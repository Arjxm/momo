use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Error types for the agent
#[derive(Error, Debug)]
pub enum AgentError {
    #[error("API error: {0}")]
    ApiError(String),

    #[error("Tool error: {0}")]
    ToolError(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Max iterations reached")]
    MaxIterationsReached,
}

/// Runtime configuration for the agent
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub max_tokens: u32,
    pub max_iterations: u32,
    pub system_prompt: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        // Allow override via environment variable, default to 50 for complex tasks
        let max_iterations = std::env::var("AGENT_MAX_ITERATIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50);

        Self {
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 4096,
            max_iterations,
            system_prompt: None,
        }
    }
}

/// A message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub role: String,
    pub content: serde_json::Value,
}

impl ConversationMessage {
    pub fn user(text: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: serde_json::Value::String(text.to_string()),
        }
    }

    pub fn assistant(content: serde_json::Value) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
        }
    }

    pub fn tool_result(tool_use_id: &str, content: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: serde_json::json!([{
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content
            }]),
        }
    }

    pub fn tool_results(results: Vec<ToolResult>) -> Self {
        let content: Vec<serde_json::Value> = results
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": r.tool_use_id,
                    "content": r.content,
                    "is_error": r.is_error
                })
            })
            .collect();
        Self {
            role: "user".to_string(),
            content: serde_json::Value::Array(content),
        }
    }
}

/// Tool definition for the Claude API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// A tool call request from Claude
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: HashMap<String, serde_json::Value>,
}

/// Result of executing a tool
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn success(tool_use_id: String, content: String) -> Self {
        Self {
            tool_use_id,
            content,
            is_error: false,
        }
    }

    pub fn error(tool_use_id: String, content: String) -> Self {
        Self {
            tool_use_id,
            content,
            is_error: true,
        }
    }
}

/// Token usage from Claude API response
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl TokenUsage {
    pub fn add(&mut self, other: &TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

/// Response from the Claude API
#[derive(Debug)]
pub struct ClaudeResponse {
    pub content: Vec<ContentBlock>,
    pub stop_reason: String,
    pub usage: TokenUsage,
}

/// A content block in Claude's response
#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: HashMap<String, serde_json::Value>,
    },
}

impl ClaudeResponse {
    /// Extract text from all text blocks
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Extract all tool use requests
    pub fn tool_calls(&self) -> Vec<ToolCall> {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse { id, name, input } => Some(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    /// Convert content blocks to JSON for conversation history
    pub fn content_to_json(&self) -> serde_json::Value {
        let blocks: Vec<serde_json::Value> = self
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::Text(text) => serde_json::json!({
                    "type": "text",
                    "text": text
                }),
                ContentBlock::ToolUse { id, name, input } => serde_json::json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input
                }),
            })
            .collect();
        serde_json::Value::Array(blocks)
    }
}

// ═══════════════════════════════════════════════════════════════════
// GRAPH NODE TYPES
// ═══════════════════════════════════════════════════════════════════

/// Tool type enum for the graph
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    Native,
    Mcp,
    Skill,
    Browser,
}

impl std::fmt::Display for ToolType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolType::Native => write!(f, "native"),
            ToolType::Mcp => write!(f, "mcp"),
            ToolType::Skill => write!(f, "skill"),
            ToolType::Browser => write!(f, "browser"),
        }
    }
}

impl std::str::FromStr for ToolType {
    type Err = AgentError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "native" => Ok(ToolType::Native),
            "mcp" => Ok(ToolType::Mcp),
            "skill" => Ok(ToolType::Skill),
            "browser" => Ok(ToolType::Browser),
            _ => Err(AgentError::ParseError(format!("Unknown tool type: {}", s))),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// OPERATION NODE (Tool Execution Record)
// ═══════════════════════════════════════════════════════════════════

/// An operation represents a single tool execution within a task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationNode {
    pub id: String,
    /// Which task triggered this operation
    pub task_id: String,
    /// Order within task (1st, 2nd, etc.)
    pub sequence: u32,
    /// Name of the tool executed
    pub tool_name: String,
    /// Type of tool (Native, MCP, Skill, Browser)
    pub tool_type: ToolType,
    /// Input arguments as JSON
    pub inputs: serde_json::Value,
    /// Output from tool execution
    pub output: String,
    /// Whether output was truncated
    pub output_truncated: bool,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
    /// Whether execution succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
    /// Previous operation ID if this was chained
    pub previous_op_id: Option<String>,
    /// Timestamp when operation was created/executed
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl OperationNode {
    pub fn new(
        task_id: String,
        sequence: u32,
        tool_name: String,
        tool_type: ToolType,
        inputs: serde_json::Value,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            task_id,
            sequence,
            tool_name,
            tool_type,
            inputs,
            output: String::new(),
            output_truncated: false,
            duration_ms: 0,
            success: false,
            error: None,
            previous_op_id: None,
            created_at: chrono::Utc::now(),
        }
    }

    /// Complete the operation with results
    pub fn complete(&mut self, output: String, duration_ms: u64, truncated: bool) {
        self.output = output;
        self.duration_ms = duration_ms;
        self.output_truncated = truncated;
        self.success = true;
    }

    /// Mark the operation as failed
    pub fn fail(&mut self, error: String, duration_ms: u64) {
        self.error = Some(error);
        self.duration_ms = duration_ms;
        self.success = false;
    }

    /// Chain this operation to a previous one
    pub fn chain_from(mut self, previous_op_id: String) -> Self {
        self.previous_op_id = Some(previous_op_id);
        self
    }
}

/// A tool node in the graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolNode {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tool_type: ToolType,
    pub input_schema: serde_json::Value,
    pub source: String,
    pub enabled: bool,
    pub usage_count: i64,
    pub success_rate: f64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl ToolNode {
    pub fn new(
        name: String,
        description: String,
        tool_type: ToolType,
        input_schema: serde_json::Value,
        source: String,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            description,
            tool_type,
            input_schema,
            source,
            enabled: true,
            usage_count: 0,
            success_rate: 1.0,
            created_at: chrono::Utc::now(),
            last_used_at: None,
        }
    }

    /// Convert to ToolDefinition for Claude API
    pub fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
        }
    }
}

/// MCP server status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MCPStatus {
    Connected,
    Disconnected,
    Error,
}

impl std::fmt::Display for MCPStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MCPStatus::Connected => write!(f, "connected"),
            MCPStatus::Disconnected => write!(f, "disconnected"),
            MCPStatus::Error => write!(f, "error"),
        }
    }
}

impl std::str::FromStr for MCPStatus {
    type Err = AgentError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "connected" => Ok(MCPStatus::Connected),
            "disconnected" => Ok(MCPStatus::Disconnected),
            "error" => Ok(MCPStatus::Error),
            _ => Err(AgentError::ParseError(format!("Unknown MCP status: {}", s))),
        }
    }
}

/// An MCP server node in the graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPServerNode {
    pub id: String,
    pub name: String,
    pub url: Option<String>,
    pub transport: String,
    pub status: MCPStatus,
    pub auto_connect: bool,
    pub last_connected_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl MCPServerNode {
    pub fn new(name: String, url: Option<String>, transport: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            url,
            transport,
            status: MCPStatus::Disconnected,
            auto_connect: true,
            last_connected_at: None,
        }
    }
}

/// Memory type enum
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Fact,
    Preference,
    EpisodeSummary,
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryType::Fact => write!(f, "fact"),
            MemoryType::Preference => write!(f, "preference"),
            MemoryType::EpisodeSummary => write!(f, "episode_summary"),
        }
    }
}

/// A memory node in the graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryNode {
    pub id: String,
    pub content: String,
    /// SHA256 fingerprint for deduplication
    pub fingerprint: String,
    pub memory_type: MemoryType,
    pub importance: f64,
    /// Provenance: which task learned this memory
    pub source_task_id: Option<String>,
    /// Provenance: which operation produced this memory
    pub source_operation_id: Option<String>,
    /// Last time this memory was accessed/retrieved
    pub last_accessed: chrono::DateTime<chrono::Utc>,
    /// Number of times this memory has been accessed
    pub access_count: u32,
    /// Task IDs that have recalled/used this memory
    pub tasks_used_in: Vec<String>,
    pub valid_from: chrono::DateTime<chrono::Utc>,
    pub valid_until: Option<chrono::DateTime<chrono::Utc>>,
    /// ID of memory that supersedes this one (if invalidated)
    pub superseded_by: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl MemoryNode {
    pub fn new(content: String, memory_type: MemoryType, importance: f64) -> Self {
        let now = chrono::Utc::now();
        let fingerprint = Self::compute_fingerprint(&content);
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            content,
            fingerprint,
            memory_type,
            importance: importance.clamp(0.0, 1.0),
            source_task_id: None,
            source_operation_id: None,
            last_accessed: now,
            access_count: 0,
            tasks_used_in: Vec::new(),
            valid_from: now,
            valid_until: None,
            superseded_by: None,
            created_at: now,
        }
    }

    /// Create a memory with full provenance
    pub fn with_provenance(
        content: String,
        memory_type: MemoryType,
        importance: f64,
        source_task_id: Option<String>,
        source_operation_id: Option<String>,
    ) -> Self {
        let mut memory = Self::new(content, memory_type, importance);
        memory.source_task_id = source_task_id;
        memory.source_operation_id = source_operation_id;
        memory
    }

    /// Compute SHA256 fingerprint for content deduplication
    pub fn compute_fingerprint(content: &str) -> String {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        // Normalize: lowercase, trim whitespace, collapse spaces
        let normalized = content
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        hasher.update(normalized.as_bytes());
        let result = hasher.finalize();
        format!("{:x}", result)
    }

    /// Record that this memory was used in a task
    pub fn record_usage(&mut self, task_id: &str) {
        if !self.tasks_used_in.contains(&task_id.to_string()) {
            self.tasks_used_in.push(task_id.to_string());
        }
        self.access_count += 1;
        self.last_accessed = chrono::Utc::now();
    }

    pub fn is_valid(&self) -> bool {
        self.valid_until.is_none()
    }

    /// Calculate recency score (1.0 for recent, decays over time)
    /// Half-life of ~7 days
    pub fn recency_score(&self) -> f64 {
        let now = chrono::Utc::now();
        let hours_since_access = (now - self.last_accessed).num_hours() as f64;
        let days_since_access = hours_since_access / 24.0;

        // Exponential decay with 7-day half-life
        // score = e^(-0.099 * days) ≈ 0.5 after 7 days
        (-0.099 * days_since_access).exp()
    }

    /// Calculate access frequency score (logarithmic scale)
    pub fn frequency_score(&self) -> f64 {
        // Log scale: 0 accesses = 0.1, 1 = 0.3, 10 = 0.5, 100 = 0.7
        (1.0 + self.access_count as f64).ln() / 10.0 + 0.1
    }

    /// Calculate overall relevance score for a query
    /// Combines: importance, recency, frequency, and keyword match
    pub fn relevance_score(&self, keyword_match_score: f64) -> f64 {
        let weights = MemoryScoreWeights::default();

        let score = (self.importance * weights.importance)
            + (self.recency_score() * weights.recency)
            + (self.frequency_score().min(1.0) * weights.frequency)
            + (keyword_match_score * weights.keyword_match);

        score.clamp(0.0, 1.0)
    }
}

/// Weights for memory scoring
#[derive(Debug, Clone)]
pub struct MemoryScoreWeights {
    pub importance: f64,
    pub recency: f64,
    pub frequency: f64,
    pub keyword_match: f64,
}

impl Default for MemoryScoreWeights {
    fn default() -> Self {
        Self {
            importance: 0.25,    // Base importance from extraction
            recency: 0.30,       // How recently accessed
            frequency: 0.15,     // How often accessed
            keyword_match: 0.30, // How well it matches query
        }
    }
}

/// An episode (interaction) node in the graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeNode {
    pub id: String,
    pub user_input: String,
    pub agent_response: String,
    pub tools_used: Vec<String>,
    pub success: bool,
    pub duration_ms: i64,
    pub tokens_used: i64,
    pub cost_usd: f64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl EpisodeNode {
    pub fn new(
        user_input: String,
        agent_response: String,
        tools_used: Vec<String>,
        success: bool,
        duration_ms: i64,
        tokens_used: i64,
        cost_usd: f64,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            user_input,
            agent_response,
            tools_used,
            success,
            duration_ms,
            tokens_used,
            cost_usd,
            created_at: chrono::Utc::now(),
        }
    }
}

/// A user node in the graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserNode {
    pub id: String,
    pub name: String,
    pub telegram_chat_id: Option<String>,
    pub timezone: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl UserNode {
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            telegram_chat_id: None,
            timezone: "UTC".to_string(),
            created_at: chrono::Utc::now(),
        }
    }
}

/// A topic node in the graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicNode {
    pub id: String,
    pub name: String,
}

impl TopicNode {
    pub fn new(name: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
        }
    }
}

/// Graph statistics
#[derive(Debug, Clone, Default)]
pub struct GraphStats {
    pub tools: usize,
    pub native_tools: usize,
    pub mcp_tools: usize,
    pub skill_tools: usize,
    pub mcp_servers: usize,
    pub memories: usize,
    pub episodes: usize,
    pub topics: usize,
}

impl std::fmt::Display for GraphStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Graph: {} tools ({} native, {} MCP, {} skills), {} memories, {} episodes, {} topics",
            self.tools,
            self.native_tools,
            self.mcp_tools,
            self.skill_tools,
            self.memories,
            self.episodes,
            self.topics
        )
    }
}
