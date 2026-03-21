use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::types::{AgentType, Plan, TaskNode, TaskPriority, TaskStatus};
use crate::types::AgentError;

/// Task queue with dependency-aware scheduling
pub struct TaskQueue {
    /// All tasks by ID
    tasks: HashMap<String, TaskNode>,
    /// Tasks ready for execution (dependencies satisfied)
    ready_queue: VecDeque<String>,
    /// Completed task IDs
    completed: HashSet<String>,
    /// Failed task IDs (for dependency checking)
    failed: HashSet<String>,
    /// Current plan ID
    plan_id: Option<String>,
}

impl TaskQueue {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            ready_queue: VecDeque::new(),
            completed: HashSet::new(),
            failed: HashSet::new(),
            plan_id: None,
        }
    }

    /// Load a plan into the queue
    pub fn load_plan(&mut self, plan: Plan) {
        self.clear();
        self.plan_id = Some(plan.id);

        // Add all tasks
        for task in plan.tasks {
            let task_id = task.id.clone();
            self.tasks.insert(task_id.clone(), task);
        }

        // Find initially ready tasks (no dependencies)
        self.update_ready_queue();

        info!(
            "Loaded plan with {} tasks, {} ready",
            self.tasks.len(),
            self.ready_queue.len()
        );
    }

    /// Clear the queue
    pub fn clear(&mut self) {
        self.tasks.clear();
        self.ready_queue.clear();
        self.completed.clear();
        self.failed.clear();
        self.plan_id = None;
    }

    /// Add a single task
    pub fn add_task(&mut self, task: TaskNode) {
        let task_id = task.id.clone();
        self.tasks.insert(task_id.clone(), task);
        self.update_ready_queue();
    }

    /// Get the next ready task, prioritized by priority level
    pub fn pop_ready(&mut self) -> Option<TaskNode> {
        // Sort ready queue by priority (highest first)
        let mut ready_tasks: Vec<(String, TaskPriority)> = self
            .ready_queue
            .iter()
            .filter_map(|id| {
                self.tasks.get(id).map(|t| (id.clone(), t.priority))
            })
            .collect();

        ready_tasks.sort_by(|a, b| b.1.cmp(&a.1));

        if let Some((task_id, _)) = ready_tasks.first() {
            // Remove from ready queue
            self.ready_queue.retain(|id| id != task_id);

            // Mark as in progress
            if let Some(task) = self.tasks.get_mut(task_id) {
                task.start();
                return Some(task.clone());
            }
        }

        None
    }

    /// Get the next ready task for a specific agent type
    pub fn pop_ready_for_agent(&mut self, agent_type: &AgentType) -> Option<TaskNode> {
        let matching: Vec<String> = self
            .ready_queue
            .iter()
            .filter(|id| {
                self.tasks
                    .get(*id)
                    .map(|t| &t.agent_type == agent_type)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        // Sort by priority
        let mut with_priority: Vec<(String, TaskPriority)> = matching
            .into_iter()
            .filter_map(|id| {
                self.tasks.get(&id).map(|t| (id, t.priority))
            })
            .collect();

        with_priority.sort_by(|a, b| b.1.cmp(&a.1));

        if let Some((task_id, _)) = with_priority.first() {
            self.ready_queue.retain(|id| id != task_id);

            if let Some(task) = self.tasks.get_mut(task_id) {
                task.start();
                return Some(task.clone());
            }
        }

        None
    }

    /// Mark a task as completed
    pub fn complete_task(&mut self, task_id: &str, output: String) {
        if let Some(task) = self.tasks.get_mut(task_id) {
            task.complete(output);
            self.completed.insert(task_id.to_string());
            debug!("Task {} completed", task_id);
        }

        // Update ready queue - some tasks may now be unblocked
        self.update_ready_queue();
    }

    /// Mark a task as failed
    pub fn fail_task(&mut self, task_id: &str, error: String) {
        if let Some(task) = self.tasks.get_mut(task_id) {
            if task.can_retry() {
                task.retry();
                info!("Task {} failed, retrying (attempt {})", task_id, task.retries);
                self.update_ready_queue();
            } else {
                task.fail(error);
                self.failed.insert(task_id.to_string());
                warn!("Task {} failed permanently", task_id);

                // Cancel dependent tasks
                self.cancel_dependents(task_id);
            }
        }
    }

    /// Cancel all tasks that depend on a failed task
    fn cancel_dependents(&mut self, failed_task_id: &str) {
        let dependents: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, t)| t.dependencies.contains(&failed_task_id.to_string()))
            .map(|(id, _)| id.clone())
            .collect();

        for dep_id in dependents {
            if let Some(task) = self.tasks.get_mut(&dep_id) {
                if task.status == TaskStatus::Pending || task.status == TaskStatus::Ready {
                    task.status = TaskStatus::Cancelled;
                    task.error = Some(format!("Dependency {} failed", failed_task_id));
                    self.failed.insert(dep_id.clone());
                    warn!("Cancelled task {} due to failed dependency", dep_id);
                    // Recursively cancel
                    self.cancel_dependents(&dep_id);
                }
            }
        }

        // Remove cancelled tasks from ready queue
        self.ready_queue.retain(|id| !self.failed.contains(id));
    }

    /// Update the ready queue based on completed dependencies
    fn update_ready_queue(&mut self) {
        let completed_list: Vec<String> = self.completed.iter().cloned().collect();

        for (task_id, task) in &self.tasks {
            // Skip if already in ready queue or not pending
            if self.ready_queue.contains(task_id) || task.status != TaskStatus::Pending {
                continue;
            }

            // Check if dependencies are satisfied
            if task.dependencies_satisfied(&completed_list) {
                self.ready_queue.push_back(task_id.clone());
                debug!("Task {} is now ready", task_id);
            }
        }
    }

    /// Get all tasks matching a status
    pub fn tasks_with_status(&self, status: &TaskStatus) -> Vec<&TaskNode> {
        self.tasks
            .values()
            .filter(|t| &t.status == status)
            .collect()
    }

    /// Check if all tasks are complete (or failed/cancelled)
    pub fn is_complete(&self) -> bool {
        self.tasks.values().all(|t| {
            matches!(
                t.status,
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
            )
        })
    }

    /// Get completion stats
    pub fn stats(&self) -> QueueStats {
        let mut stats = QueueStats::default();
        for task in self.tasks.values() {
            stats.total += 1;
            match task.status {
                TaskStatus::Pending => stats.pending += 1,
                TaskStatus::Ready => stats.ready += 1,
                TaskStatus::InProgress => stats.in_progress += 1,
                TaskStatus::Completed => stats.completed += 1,
                TaskStatus::Failed => stats.failed += 1,
                TaskStatus::Cancelled => stats.cancelled += 1,
            }
        }
        stats
    }

    /// Get a task by ID
    pub fn get_task(&self, task_id: &str) -> Option<&TaskNode> {
        self.tasks.get(task_id)
    }

    /// Get all tasks
    pub fn all_tasks(&self) -> Vec<&TaskNode> {
        self.tasks.values().collect()
    }

    /// Get completed task outputs for context
    pub fn completed_outputs(&self) -> HashMap<String, String> {
        self.tasks
            .iter()
            .filter(|(_, t)| t.status == TaskStatus::Completed)
            .filter_map(|(id, t)| {
                t.output.as_ref().map(|o| (id.clone(), o.clone()))
            })
            .collect()
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Queue statistics
#[derive(Debug, Clone, Default)]
pub struct QueueStats {
    pub total: usize,
    pub pending: usize,
    pub ready: usize,
    pub in_progress: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
}

impl std::fmt::Display for QueueStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Tasks: {}/{} done, {} ready, {} in-progress, {} failed",
            self.completed, self.total, self.ready, self.in_progress, self.failed
        )
    }
}

/// Thread-safe task queue wrapper
pub type SharedTaskQueue = Arc<Mutex<TaskQueue>>;

pub fn create_shared_queue() -> SharedTaskQueue {
    Arc::new(Mutex::new(TaskQueue::new()))
}
