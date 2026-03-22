use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Agent specialization types
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    /// Master planner - decomposes tasks and coordinates workers
    Planner,
    /// Research agent - web search, document analysis
    Research,
    /// Code agent - writes and analyzes code
    Code,
    /// Communications agent - email, messaging, notifications
    Comms,
    /// Data agent - database queries, data transformation
    Data,
    /// Browser agent - web automation, scraping
    Browser,
}

impl std::fmt::Display for AgentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentType::Planner => write!(f, "planner"),
            AgentType::Research => write!(f, "research"),
            AgentType::Code => write!(f, "code"),
            AgentType::Comms => write!(f, "comms"),
            AgentType::Data => write!(f, "data"),
            AgentType::Browser => write!(f, "browser"),
        }
    }
}

/// Task status in the queue
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Waiting for dependencies
    Pending,
    /// Ready to execute (dependencies satisfied)
    Ready,
    /// Currently being worked on
    InProgress,
    /// Successfully completed
    Completed,
    /// Failed with error
    Failed,
    /// Cancelled by user or system
    Cancelled,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Pending => write!(f, "pending"),
            TaskStatus::Ready => write!(f, "ready"),
            TaskStatus::InProgress => write!(f, "in_progress"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::Failed => write!(f, "failed"),
            TaskStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Priority levels for tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Urgent = 3,
}

impl Default for TaskPriority {
    fn default() -> Self {
        TaskPriority::Normal
    }
}

/// A task node in the dependency graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNode {
    pub id: String,
    pub description: String,
    pub agent_type: AgentType,
    pub status: TaskStatus,
    pub priority: TaskPriority,

    // ═══════════════════════════════════════════════════════════════════
    // TASK HIERARCHY (for autonomous decomposition)
    // ═══════════════════════════════════════════════════════════════════

    /// If this is a subtask, the ID of the parent task that created it
    pub parent_id: Option<String>,
    /// The original root task (user's request) - always set
    pub root_id: Option<String>,

    // ═══════════════════════════════════════════════════════════════════
    // DEPENDENCIES & EXECUTION
    // ═══════════════════════════════════════════════════════════════════

    /// IDs of tasks this task depends on (must complete before this starts)
    pub dependencies: Vec<String>,
    /// Tool hint - suggested tool for this task
    pub tool_hint: Option<String>,
    /// Input context from parent task or user
    pub input_context: String,
    /// Output result when completed
    pub output: Option<String>,
    /// Error message if failed
    pub error: Option<String>,

    // ═══════════════════════════════════════════════════════════════════
    // OPERATION & MEMORY TRACKING
    // ═══════════════════════════════════════════════════════════════════

    /// Operation IDs performed during this task
    pub operations: Vec<String>,
    /// Memory IDs that were recalled/used during this task
    pub memories_used: Vec<String>,
    /// Memory IDs that were created/learned from this task
    pub memories_created: Vec<String>,

    // ═══════════════════════════════════════════════════════════════════
    // RETRY & TIMING
    // ═══════════════════════════════════════════════════════════════════

    /// Number of retry attempts
    pub retries: u32,
    /// Maximum retries allowed
    pub max_retries: u32,
    /// Created timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Started timestamp
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Completed timestamp
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl TaskNode {
    pub fn new(
        description: String,
        agent_type: AgentType,
        dependencies: Vec<String>,
        input_context: String,
    ) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        Self {
            id: id.clone(),
            description,
            agent_type,
            status: TaskStatus::Pending,
            priority: TaskPriority::Normal,
            parent_id: None,
            root_id: Some(id), // Default: self is root
            dependencies,
            tool_hint: None,
            input_context,
            output: None,
            error: None,
            operations: Vec::new(),
            memories_used: Vec::new(),
            memories_created: Vec::new(),
            retries: 0,
            max_retries: 3,
            created_at: chrono::Utc::now(),
            started_at: None,
            completed_at: None,
        }
    }

    /// Create a subtask under a parent task
    pub fn subtask(
        description: String,
        agent_type: AgentType,
        parent: &TaskNode,
        dependencies: Vec<String>,
        input_context: String,
    ) -> Self {
        let mut task = Self::new(description, agent_type, dependencies, input_context);
        task.parent_id = Some(parent.id.clone());
        task.root_id = parent.root_id.clone();
        task
    }

    /// Create a root task (user's original request)
    pub fn root(description: String, agent_type: AgentType, input_context: String) -> Self {
        let mut task = Self::new(description, agent_type, vec![], input_context);
        task.root_id = Some(task.id.clone());
        task
    }

    pub fn with_priority(mut self, priority: TaskPriority) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_tool_hint(mut self, tool: String) -> Self {
        self.tool_hint = Some(tool);
        self
    }

    /// Record an operation performed during this task
    pub fn record_operation(&mut self, operation_id: String) {
        self.operations.push(operation_id);
    }

    /// Record a memory that was used/recalled during this task
    pub fn record_memory_used(&mut self, memory_id: String) {
        if !self.memories_used.contains(&memory_id) {
            self.memories_used.push(memory_id);
        }
    }

    /// Record a memory that was created/learned from this task
    pub fn record_memory_created(&mut self, memory_id: String) {
        self.memories_created.push(memory_id);
    }

    /// Check if this is a root task (not a subtask)
    pub fn is_root(&self) -> bool {
        self.parent_id.is_none()
    }

    /// Check if this is a subtask
    pub fn is_subtask(&self) -> bool {
        self.parent_id.is_some()
    }

    /// Check if all dependencies are satisfied
    pub fn dependencies_satisfied(&self, completed_tasks: &[String]) -> bool {
        self.dependencies.iter().all(|dep| completed_tasks.contains(dep))
    }

    /// Mark as in progress
    pub fn start(&mut self) {
        self.status = TaskStatus::InProgress;
        self.started_at = Some(chrono::Utc::now());
    }

    /// Mark as completed with output
    pub fn complete(&mut self, output: String) {
        self.status = TaskStatus::Completed;
        self.output = Some(output);
        self.completed_at = Some(chrono::Utc::now());
    }

    /// Mark as failed with error
    pub fn fail(&mut self, error: String) {
        self.status = TaskStatus::Failed;
        self.error = Some(error);
        self.completed_at = Some(chrono::Utc::now());
    }

    /// Check if can retry
    pub fn can_retry(&self) -> bool {
        self.retries < self.max_retries
    }

    /// Increment retry count and reset for retry
    pub fn retry(&mut self) {
        self.retries += 1;
        self.status = TaskStatus::Pending;
        self.error = None;
        self.started_at = None;
        self.completed_at = None;
    }
}

/// Plan result from the planner
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub original_request: String,
    pub tasks: Vec<TaskNode>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl Plan {
    pub fn new(original_request: String, tasks: Vec<TaskNode>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            original_request,
            tasks,
            created_at: chrono::Utc::now(),
        }
    }

    /// Get task IDs in topological order
    pub fn execution_order(&self) -> Vec<String> {
        let mut result = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let task_map: HashMap<String, &TaskNode> =
            self.tasks.iter().map(|t| (t.id.clone(), t)).collect();

        fn visit(
            task_id: &str,
            task_map: &HashMap<String, &TaskNode>,
            visited: &mut std::collections::HashSet<String>,
            result: &mut Vec<String>,
        ) {
            if visited.contains(task_id) {
                return;
            }
            if let Some(task) = task_map.get(task_id) {
                for dep in &task.dependencies {
                    visit(dep, task_map, visited, result);
                }
            }
            visited.insert(task_id.to_string());
            result.push(task_id.to_string());
        }

        for task in &self.tasks {
            visit(&task.id, &task_map, &mut visited, &mut result);
        }

        result
    }
}

/// Worker execution result
#[derive(Debug, Clone)]
pub struct WorkerResult {
    pub task_id: String,
    pub success: bool,
    pub output: String,
    pub tools_used: Vec<String>,
    pub tokens_used: u32,
}

impl WorkerResult {
    pub fn success(task_id: String, output: String, tools_used: Vec<String>, tokens_used: u32) -> Self {
        Self {
            task_id,
            success: true,
            output,
            tools_used,
            tokens_used,
        }
    }

    pub fn failure(task_id: String, error: String) -> Self {
        Self {
            task_id,
            success: false,
            output: error,
            tools_used: vec![],
            tokens_used: 0,
        }
    }
}

/// Skill template for SkillFactory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillTemplate {
    pub name: String,
    pub description: String,
    pub language: String, // "python", "javascript", "wasm"
    pub code: String,
    pub input_schema: serde_json::Value,
    pub dependencies: Vec<String>,
}

impl SkillTemplate {
    pub fn new(
        name: String,
        description: String,
        language: String,
        code: String,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name,
            description,
            language,
            code,
            input_schema,
            dependencies: vec![],
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// SELF-IMPROVEMENT: Task Specification & Validation
// ═══════════════════════════════════════════════════════════════════

/// A numeric requirement extracted from a task description
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumericRequirement {
    /// What is being counted (e.g., "sites", "products")
    pub entity: String,
    /// The expected count
    pub expected_count: u32,
    /// Operator for comparison (exactly, at_least, at_most)
    pub comparison: ComparisonOp,
}

/// Comparison operator for numeric requirements
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonOp {
    Exactly,
    AtLeast,
    AtMost,
}

impl Default for ComparisonOp {
    fn default() -> Self {
        ComparisonOp::AtLeast
    }
}

/// An expected output from task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedOutput {
    /// Name or pattern of the expected output
    pub name: String,
    /// Type of output (file, data, message)
    pub output_type: OutputType,
    /// Whether this output is required or optional
    pub required: bool,
}

/// Type of expected output
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputType {
    File,
    Data,
    Message,
    Artifact,
}

impl Default for OutputType {
    fn default() -> Self {
        OutputType::Data
    }
}

/// Specification extracted from a task description
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpecification {
    /// Numeric requirements (e.g., "3 sites", "10 products each")
    pub numeric_requirements: Vec<NumericRequirement>,
    /// Expected outputs to be produced
    pub expected_outputs: Vec<ExpectedOutput>,
    /// Qualitative requirements (free-form text)
    pub qualitative_requirements: Vec<String>,
    /// Keywords for similarity matching
    pub keywords: Vec<String>,
    /// Original task description
    pub original_description: String,
}

impl TaskSpecification {
    pub fn new(description: String) -> Self {
        Self {
            numeric_requirements: Vec::new(),
            expected_outputs: Vec::new(),
            qualitative_requirements: Vec::new(),
            keywords: Vec::new(),
            original_description: description,
        }
    }

    /// Generate a fingerprint for task matching
    pub fn fingerprint(&self) -> String {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();

        // Hash based on keywords and requirement types
        let mut parts: Vec<String> = self.keywords.clone();
        for req in &self.numeric_requirements {
            parts.push(format!("num:{}", req.entity));
        }
        for out in &self.expected_outputs {
            parts.push(format!("out:{}", out.name));
        }
        parts.sort();
        hasher.update(parts.join(",").as_bytes());

        format!("{:x}", hasher.finalize())[..16].to_string()
    }

    /// Check if this spec has any requirements to validate
    pub fn has_requirements(&self) -> bool {
        !self.numeric_requirements.is_empty()
            || !self.expected_outputs.is_empty()
            || !self.qualitative_requirements.is_empty()
    }
}

/// Result of validating a single requirement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementResult {
    /// The requirement description
    pub requirement: String,
    /// Whether it passed
    pub passed: bool,
    /// Actual value observed (if applicable)
    pub actual_value: Option<String>,
    /// Expected value (if applicable)
    pub expected_value: Option<String>,
    /// Explanation of the result
    pub explanation: String,
}

/// Result of validating a single output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputResult {
    /// The expected output
    pub expected: ExpectedOutput,
    /// Whether it was found
    pub found: bool,
    /// Location or details of found output
    pub location: Option<String>,
}

/// Element that was missing or incorrect
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingElement {
    /// What's missing
    pub element: String,
    /// Category (numeric, output, qualitative)
    pub category: String,
    /// Detailed explanation
    pub details: String,
}

/// Overall result of validating task output against specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Whether all requirements were met
    pub overall_success: bool,
    /// Results for each numeric/qualitative requirement
    pub requirement_results: Vec<RequirementResult>,
    /// Results for each expected output
    pub output_results: Vec<OutputResult>,
    /// Summary of what's missing
    pub missing_elements: Vec<MissingElement>,
    /// Overall confidence score (0.0 - 1.0)
    pub confidence: f64,
    /// Human-readable summary
    pub summary: String,
}

impl ValidationResult {
    /// Create a successful validation result
    pub fn success() -> Self {
        Self {
            overall_success: true,
            requirement_results: Vec::new(),
            output_results: Vec::new(),
            missing_elements: Vec::new(),
            confidence: 1.0,
            summary: "All requirements satisfied".to_string(),
        }
    }

    /// Create a failed validation result
    pub fn failure(summary: String) -> Self {
        Self {
            overall_success: false,
            requirement_results: Vec::new(),
            output_results: Vec::new(),
            missing_elements: Vec::new(),
            confidence: 0.0,
            summary,
        }
    }

    /// Add a requirement result
    pub fn with_requirement(mut self, result: RequirementResult) -> Self {
        if !result.passed {
            self.overall_success = false;
        }
        self.requirement_results.push(result);
        self
    }

    /// Add an output result
    pub fn with_output(mut self, result: OutputResult) -> Self {
        if !result.found && result.expected.required {
            self.overall_success = false;
        }
        self.output_results.push(result);
        self
    }

    /// Add a missing element
    pub fn with_missing(mut self, element: MissingElement) -> Self {
        self.overall_success = false;
        self.missing_elements.push(element);
        self
    }

    /// Count the number of failures
    pub fn failure_count(&self) -> usize {
        let req_failures = self.requirement_results.iter().filter(|r| !r.passed).count();
        let output_failures = self.output_results.iter().filter(|r| !r.found && r.expected.required).count();
        req_failures + output_failures + self.missing_elements.len()
    }
}
