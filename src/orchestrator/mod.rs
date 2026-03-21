pub mod planner;
pub mod skill_factory;
pub mod task_queue;
pub mod types;
pub mod workers;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use planner::Planner;
use skill_factory::SkillFactory;
use task_queue::SharedTaskQueue;
use types::{AgentType, TaskNode};
use workers::{ToolExecutor, WorkerPool};

use crate::graph::GraphBrain;
use crate::providers::LLMProvider;
use crate::skills::SkillManager;
use crate::tools::mcp_bridge::MCPBridge;
use crate::tools::{BrowserTool, ToolRegistry};
use crate::types::{AgentError, ToolResult, ToolType};

/// The Orchestrator coordinates multi-agent task execution
pub struct Orchestrator {
    planner: Planner,
    worker_pool: WorkerPool,
    task_queue: SharedTaskQueue,
    skill_factory: SkillFactory,
    registry: Arc<Mutex<ToolRegistry>>,
    brain: Arc<GraphBrain>,
    mcp_bridge: Option<Arc<Mutex<MCPBridge>>>,
    skill_manager: Option<Arc<Mutex<SkillManager>>>,
    browser_tool: Option<Arc<BrowserTool>>,
}

impl Orchestrator {
    pub fn new(
        provider: Arc<dyn LLMProvider>,
        registry: ToolRegistry,
        brain: Arc<GraphBrain>,
        skills_dir: std::path::PathBuf,
    ) -> Self {
        let available_tools: Vec<String> = registry.definitions().iter().map(|t| t.name.clone()).collect();

        let planner = Planner::new(provider.clone(), available_tools);
        let worker_pool = WorkerPool::new(provider, &registry);
        let task_queue = task_queue::create_shared_queue();
        let skill_factory = SkillFactory::new(skills_dir);

        Self {
            planner,
            worker_pool,
            task_queue,
            skill_factory,
            registry: Arc::new(Mutex::new(registry)),
            brain,
            mcp_bridge: None,
            skill_manager: None,
            browser_tool: None,
        }
    }

    pub fn with_mcp_bridge(mut self, bridge: Arc<Mutex<MCPBridge>>) -> Self {
        self.mcp_bridge = Some(bridge);
        self
    }

    pub fn with_skill_manager(mut self, manager: Arc<Mutex<SkillManager>>) -> Self {
        self.skill_manager = Some(manager);
        self
    }

    pub fn with_browser(mut self, browser: Arc<BrowserTool>) -> Self {
        self.browser_tool = Some(browser);
        self
    }

    /// Execute a complex task using multi-agent orchestration
    pub async fn execute(&self, request: &str) -> Result<OrchestratorResult, AgentError> {
        info!("Orchestrator executing: {}", request);

        // Check if we should skip planning for simple tasks
        if Planner::should_skip_planning(request) {
            debug!("Skipping planning for simple request");
            return self.execute_simple(request).await;
        }

        // Phase 1: Plan decomposition
        info!("[ORCHESTRATOR] Planning task decomposition...");
        let plan = self.planner.decompose(request).await?;
        info!("[ORCHESTRATOR] Created plan with {} tasks:", plan.tasks.len());
        for (i, task) in plan.tasks.iter().enumerate() {
            info!("  {}. [{}] {}", i + 1, task.agent_type, task.description);
        }

        // Load plan into queue
        {
            let mut queue = self.task_queue.lock().await;
            queue.load_plan(plan.clone());
        }

        // Phase 2: Execute tasks
        let mut results: HashMap<String, String> = HashMap::new();
        let mut total_tokens = 0u32;
        let mut all_tools_used: Vec<String> = Vec::new();

        // Create tool executor
        let executor = OrchestratorToolExecutor {
            registry: self.registry.clone(),
            mcp_bridge: self.mcp_bridge.clone(),
            skill_manager: self.skill_manager.clone(),
            browser_tool: self.browser_tool.clone(),
        };

        loop {
            // Get next ready task
            let task = {
                let mut queue = self.task_queue.lock().await;

                // Check if complete
                if queue.is_complete() {
                    let stats = queue.stats();
                    info!("Orchestration complete: {}", stats);
                    break;
                }

                queue.pop_ready()
            };

            let task = match task {
                Some(t) => t,
                None => {
                    // No ready tasks but not complete - might be waiting for dependencies
                    // or all tasks are in progress
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    continue;
                }
            };

            let stats = self.task_queue.lock().await.stats();
            info!("[ORCHESTRATOR] Task {}/{}: {} (agent: {})",
                stats.completed + stats.in_progress,
                stats.total,
                task.description,
                task.agent_type
            );

            // Build context from completed dependencies
            let context = self.build_task_context(&task, &results);

            // Get appropriate worker
            let worker = match self.worker_pool.get_worker(&task.agent_type) {
                Some(w) => w,
                None => {
                    warn!("No worker for agent type {}", task.agent_type);
                    let mut queue = self.task_queue.lock().await;
                    queue.fail_task(&task.id, format!("No worker for {}", task.agent_type));
                    continue;
                }
            };

            // Execute task
            let result = worker.execute(&task, &context, &executor).await;
            total_tokens += result.tokens_used;

            for tool in &result.tools_used {
                if !all_tools_used.contains(tool) {
                    all_tools_used.push(tool.clone());
                }
            }

            // Update queue based on result
            {
                let mut queue = self.task_queue.lock().await;
                if result.success {
                    queue.complete_task(&task.id, result.output.clone());
                    results.insert(task.id.clone(), result.output);
                } else {
                    queue.fail_task(&task.id, result.output);
                }
            }
        }

        // Phase 3: Synthesize final result
        let final_result = self.synthesize_result(request, &results).await?;

        Ok(OrchestratorResult {
            response: final_result,
            tasks_completed: results.len(),
            total_tokens,
            tools_used: all_tools_used,
        })
    }

    /// Execute a simple task without multi-agent orchestration
    async fn execute_simple(&self, request: &str) -> Result<OrchestratorResult, AgentError> {
        // Create a single task and execute it
        let task = TaskNode::new(
            request.to_string(),
            AgentType::Code, // Default to code agent
            vec![],
            request.to_string(),
        );

        let executor = OrchestratorToolExecutor {
            registry: self.registry.clone(),
            mcp_bridge: self.mcp_bridge.clone(),
            skill_manager: self.skill_manager.clone(),
            browser_tool: self.browser_tool.clone(),
        };

        let worker = self.worker_pool.get_worker(&AgentType::Code)
            .ok_or_else(|| AgentError::ConfigError("No code worker".to_string()))?;

        let result = worker.execute(&task, "", &executor).await;

        Ok(OrchestratorResult {
            response: result.output,
            tasks_completed: 1,
            total_tokens: result.tokens_used,
            tools_used: result.tools_used,
        })
    }

    /// Build context for a task from its completed dependencies
    fn build_task_context(&self, task: &TaskNode, results: &HashMap<String, String>) -> String {
        let dep_results: Vec<String> = task
            .dependencies
            .iter()
            .filter_map(|dep_id| {
                results.get(dep_id).map(|r| format!("From task {}:\n{}", dep_id, r))
            })
            .collect();

        dep_results.join("\n\n---\n\n")
    }

    /// Synthesize the final result from all task outputs
    async fn synthesize_result(
        &self,
        original_request: &str,
        results: &HashMap<String, String>,
    ) -> Result<String, AgentError> {
        if results.is_empty() {
            return Ok("No tasks completed.".to_string());
        }

        if results.len() == 1 {
            return Ok(results.values().next().unwrap().clone());
        }

        // For multiple results, combine them
        let combined: Vec<String> = results
            .iter()
            .map(|(id, result)| format!("## Result from task {}\n{}", id, result))
            .collect();

        Ok(format!(
            "# Results for: {}\n\n{}",
            original_request,
            combined.join("\n\n")
        ))
    }

    /// Create a new skill on-the-fly
    /// Note: Skill generation is planned for future implementation
    pub async fn create_skill(&self, _requirement: &str) -> Result<String, AgentError> {
        Err(AgentError::ConfigError(
            "Dynamic skill generation is not yet implemented".to_string()
        ))
    }

    /// Get queue statistics
    pub async fn stats(&self) -> task_queue::QueueStats {
        self.task_queue.lock().await.stats()
    }
}

/// Tool executor for orchestrator workers
struct OrchestratorToolExecutor {
    registry: Arc<Mutex<ToolRegistry>>,
    mcp_bridge: Option<Arc<Mutex<MCPBridge>>>,
    skill_manager: Option<Arc<Mutex<SkillManager>>>,
    browser_tool: Option<Arc<BrowserTool>>,
}

#[async_trait::async_trait]
impl ToolExecutor for OrchestratorToolExecutor {
    async fn execute(
        &self,
        name: &str,
        tool_use_id: &str,
        input: HashMap<String, serde_json::Value>,
    ) -> ToolResult {
        let registry = self.registry.lock().await;
        let tool_type = registry.get_tool_type(name);

        match tool_type {
            Some(ToolType::Native) | Some(ToolType::Browser) => {
                registry.execute(tool_use_id, name, input).await
            }
            Some(ToolType::Mcp) => {
                if let Some(ref bridge) = self.mcp_bridge {
                    let input_value = serde_json::to_value(&input).unwrap_or_default();
                    match bridge.lock().await.call_tool(name, &input_value).await {
                        Ok(content) => ToolResult::success(tool_use_id.to_string(), content),
                        Err(e) => ToolResult::error(tool_use_id.to_string(), e.to_string()),
                    }
                } else {
                    ToolResult::error(tool_use_id.to_string(), "MCP bridge not configured".to_string())
                }
            }
            Some(ToolType::Skill) => {
                if let Some(ref manager) = self.skill_manager {
                    let input_value = serde_json::to_value(&input).unwrap_or_default();
                    match manager.lock().await.execute(name, &input_value).await {
                        Ok(content) => ToolResult::success(tool_use_id.to_string(), content),
                        Err(e) => ToolResult::error(tool_use_id.to_string(), e.to_string()),
                    }
                } else {
                    ToolResult::error(tool_use_id.to_string(), "Skill manager not configured".to_string())
                }
            }
            None => {
                // Try registry anyway
                registry.execute(tool_use_id, name, input).await
            }
        }
    }
}

/// Result of orchestrated execution
#[derive(Debug)]
pub struct OrchestratorResult {
    pub response: String,
    pub tasks_completed: usize,
    pub total_tokens: u32,
    pub tools_used: Vec<String>,
}
