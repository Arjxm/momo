use std::sync::Arc;
use std::time::Instant;

use tracing::{debug, info, warn};

use crate::claude::ClaudeClient;
use crate::graph::memory::{record_tool_compositions, MemoryExtractor};
use crate::graph::GraphBrain;
use crate::skills::SkillManager;
use crate::tools::mcp_bridge::MCPBridge;
use crate::tools::BrowserTool;
use crate::tools::ToolRegistry;
use crate::types::{
    AgentConfig, AgentError, ConversationMessage, EpisodeNode, MemoryNode, TokenUsage, ToolResult,
    ToolType,
};

/// Maximum length for tool result content (in characters)
const MAX_TOOL_RESULT_LENGTH: usize = 8000;

/// Maximum number of messages to keep in conversation history
const MAX_CONVERSATION_MESSAGES: usize = 20;

/// Truncate a tool result to prevent token overflow
fn truncate_tool_result(mut result: ToolResult) -> ToolResult {
    if result.content.len() > MAX_TOOL_RESULT_LENGTH {
        // Check if it looks like base64 image data
        if result.content.starts_with("data:image") || result.content.contains("base64") {
            result.content = "[Image data truncated - screenshot captured successfully]".to_string();
        } else {
            result.content = format!(
                "{}...\n\n[Output truncated from {} to {} chars]",
                &result.content[..MAX_TOOL_RESULT_LENGTH - 100],
                result.content.len(),
                MAX_TOOL_RESULT_LENGTH
            );
        }
    }
    result
}

/// Prune conversation history to keep token count manageable
/// Important: Must keep tool_use and tool_result pairs together
fn prune_conversation(messages: &mut Vec<ConversationMessage>) {
    if messages.len() <= MAX_CONVERSATION_MESSAGES {
        return;
    }

    // Keep the first message (user's original request)
    let first = messages.remove(0);

    // Find safe pruning points - we can only prune BEFORE a user message
    // that doesn't contain tool_results (i.e., a fresh user input)
    // For simplicity, keep only the last N messages but ensure we don't
    // break tool_use/tool_result pairs

    // Keep the last MAX_CONVERSATION_MESSAGES - 2 messages (leave room for first + summary)
    let keep_count = MAX_CONVERSATION_MESSAGES - 2;

    if messages.len() > keep_count {
        // Find a safe cut point - look for an assistant message followed by user message
        // that starts fresh (not a tool_result)
        let mut cut_index = messages.len() - keep_count;

        // Adjust cut point to not break tool pairs
        // A tool_result user message must keep its preceding assistant tool_use message
        while cut_index > 0 && cut_index < messages.len() {
            let msg = &messages[cut_index];
            // Check if this message contains tool_result (it's a user message with tool results)
            if msg.role == "user" {
                if let Some(arr) = msg.content.as_array() {
                    let has_tool_result = arr.iter().any(|item| {
                        item.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                    });
                    if has_tool_result {
                        // Can't cut here - move back one more message
                        cut_index = cut_index.saturating_sub(1);
                        continue;
                    }
                }
            }
            break;
        }

        // Remove messages before cut point
        messages.drain(0..cut_index);
    }

    // Add summary message and restore first
    let summary = ConversationMessage::user(
        "[Earlier conversation pruned to manage context length. Continuing task...]"
    );
    messages.insert(0, summary);
    messages.insert(0, first);

    info!("Pruned conversation to {} messages", messages.len());
}

/// The main agent that orchestrates the ReAct loop
pub struct Agent {
    client: ClaudeClient,
    registry: ToolRegistry,
    config: AgentConfig,
    brain: Arc<GraphBrain>,
    user_id: String,
    memory_extractor: Option<MemoryExtractor>,
    skill_manager: Option<Arc<tokio::sync::Mutex<SkillManager>>>,
    mcp_bridge: Option<Arc<tokio::sync::Mutex<MCPBridge>>>,
    browser_tool: Option<Arc<BrowserTool>>,
}

/// Result of running the agent
pub struct AgentResult {
    pub response: String,
    pub usage: TokenUsage,
    pub tools_used: Vec<String>,
    pub episode_id: Option<String>,
}

impl Agent {
    /// Create a new agent with graph integration
    pub fn new(
        client: ClaudeClient,
        registry: ToolRegistry,
        config: AgentConfig,
        brain: Arc<GraphBrain>,
        user_id: String,
    ) -> Self {
        Self {
            client,
            registry,
            config,
            brain,
            user_id,
            memory_extractor: None,
            skill_manager: None,
            mcp_bridge: None,
            browser_tool: None,
        }
    }

    /// Set up memory extraction with a separate API key (uses Haiku)
    pub fn with_memory_extractor(mut self, api_key: String) -> Self {
        self.memory_extractor = Some(MemoryExtractor::new(api_key));
        self
    }

    /// Set up skill manager for user skills
    pub fn with_skill_manager(mut self, skill_manager: Arc<tokio::sync::Mutex<SkillManager>>) -> Self {
        self.skill_manager = Some(skill_manager);
        self
    }

    /// Set up MCP bridge for MCP tools
    pub fn with_mcp_bridge(mut self, mcp_bridge: Arc<tokio::sync::Mutex<MCPBridge>>) -> Self {
        self.mcp_bridge = Some(mcp_bridge);
        self
    }

    /// Set up browser tool
    pub fn with_browser(mut self, browser: Arc<BrowserTool>) -> Self {
        self.browser_tool = Some(browser);
        self
    }

    /// Build the dynamic system prompt with memories and context
    fn build_system_prompt(&self, memories: &[MemoryNode], prefs: &[MemoryNode]) -> String {
        let tool_counts = self.registry.count_by_type();
        let native_count = tool_counts.get(&ToolType::Native).unwrap_or(&0);
        let mcp_count = tool_counts.get(&ToolType::Mcp).unwrap_or(&0);
        let skill_count = tool_counts.get(&ToolType::Skill).unwrap_or(&0);

        let native_tools = self.registry.tool_names_by_type(&ToolType::Native).join(", ");
        let mcp_tools = self.registry.tool_names_by_type(&ToolType::Mcp).join(", ");
        let skill_tools = self.registry.tool_names_by_type(&ToolType::Skill).join(", ");

        let memories_text = if memories.is_empty() {
            "No relevant memories.".to_string()
        } else {
            memories
                .iter()
                .map(|m| format!("- {}", m.content))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let prefs_text = if prefs.is_empty() {
            "No known preferences.".to_string()
        } else {
            prefs
                .iter()
                .map(|p| format!("- {}", p.content))
                .collect::<Vec<_>>()
                .join("\n")
        };

        format!(
            r#"You are Agent Brain, an autonomous AI assistant with persistent memory and extensible tools.

AVAILABLE TOOLS ({} total):
- Native ({}): {}
- MCP ({}): {}
- Skills ({}): {}
- Browser: navigate, extract_text, extract_links, click, fill, screenshot, run_js, get_html

WHAT I KNOW:
{}

YOUR PREFERENCES:
{}

You can manage MCP servers (connect, disconnect, list, refresh) and skills.
When you need a tool that doesn't exist, tell the user - they can add custom skills.

Be concise and helpful. Use tools when they can provide accurate information."#,
            self.registry.len(),
            native_count,
            if native_tools.is_empty() { "none" } else { &native_tools },
            mcp_count,
            if mcp_tools.is_empty() { "none" } else { &mcp_tools },
            skill_count,
            if skill_tools.is_empty() { "none" } else { &skill_tools },
            memories_text,
            prefs_text
        )
    }

    /// Extract keywords from user input for memory recall
    fn extract_keywords(input: &str) -> Vec<String> {
        // Simple keyword extraction - split on whitespace and filter short words
        input
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .map(|w| w.to_lowercase())
            .take(5)
            .collect()
    }

    /// Run the agent with a user message
    pub async fn run(&self, user_message: &str) -> Result<AgentResult, AgentError> {
        let start_time = Instant::now();
        let mut tools_used = Vec::new();

        // Recall relevant memories
        let keywords = Self::extract_keywords(user_message);
        let memories = self.brain.recall(&keywords, 5).unwrap_or_default();
        let prefs = self.brain.recall_user_prefs(&self.user_id).unwrap_or_default();

        // Build dynamic system prompt
        let system_prompt = self.build_system_prompt(&memories, &prefs);

        // Create config with system prompt
        let mut run_config = self.config.clone();
        run_config.system_prompt = Some(system_prompt);

        // Create a client for this run with the system prompt
        let run_client = crate::claude::ClaudeClient::new(
            self.client.api_key().to_string(),
            run_config.clone(),
        );

        let mut messages = vec![ConversationMessage::user(user_message)];
        let tools = self.registry.definitions();
        let mut total_usage = TokenUsage::default();
        let mut iterations = 0;

        info!("Starting agent run with message: {}", user_message);

        let response_text;

        loop {
            iterations += 1;

            // Progress warnings instead of hard stops
            if iterations == 15 {
                info!("Iteration 15 - task still in progress...");
            } else if iterations == 30 {
                warn!("Iteration 30 - long-running task, consider breaking into smaller steps");
            } else if iterations > self.config.max_iterations as usize {
                warn!("Max iterations ({}) reached - stopping to prevent runaway", self.config.max_iterations);
                return Err(AgentError::MaxIterationsReached);
            }

            debug!("Iteration {}: Sending request to Claude", iterations);

            let response = run_client.send_message(&messages, &tools).await?;
            total_usage.add(&response.usage);

            debug!(
                "Received response with stop_reason: {}",
                response.stop_reason
            );

            // Add assistant response to conversation
            messages.push(ConversationMessage::assistant(response.content_to_json()));

            // Check if we should stop
            if response.stop_reason == "end_turn" {
                info!(
                    "Agent completed in {} iterations, tokens: in={}, out={}",
                    iterations, total_usage.input_tokens, total_usage.output_tokens
                );
                response_text = response.text();
                break;
            }

            // Handle tool use
            if response.stop_reason == "tool_use" {
                let tool_calls = response.tool_calls();
                info!("Executing {} tool(s)", tool_calls.len());

                let mut results = Vec::new();
                for call in tool_calls {
                    debug!("Executing tool: {} (id: {})", call.name, call.id);

                    // Track tool usage
                    if !tools_used.contains(&call.name) {
                        tools_used.push(call.name.clone());
                    }

                    // Dispatch based on tool type
                    let result = self.execute_tool(&call.name, &call.id, call.input).await;

                    debug!(
                        "Tool {} result: {} (is_error: {})",
                        call.name,
                        if result.content.len() > 100 {
                            format!("{}...", &result.content[..100])
                        } else {
                            result.content.clone()
                        },
                        result.is_error
                    );

                    // Update tool stats in graph
                    self.brain
                        .update_tool_stats(&call.name, !result.is_error)
                        .ok();

                    // Truncate result to manage tokens
                    results.push(truncate_tool_result(result));
                }

                // Add tool results to conversation
                messages.push(ConversationMessage::tool_results(results));

                // Prune conversation if getting too long
                prune_conversation(&mut messages);
            } else {
                // Unexpected stop reason
                warn!("Unexpected stop_reason: {}", response.stop_reason);
                response_text = response.text();
                break;
            }
        }

        let duration_ms = start_time.elapsed().as_millis() as i64;
        let tokens_used = (total_usage.input_tokens + total_usage.output_tokens) as i64;

        // Record tool compositions
        if tools_used.len() >= 2 {
            record_tool_compositions(&self.brain, &tools_used).ok();
        }

        // Record episode in graph
        let episode = EpisodeNode::new(
            user_message.to_string(),
            response_text.clone(),
            tools_used.clone(),
            true,
            duration_ms,
            tokens_used,
            estimate_cost(total_usage.input_tokens, total_usage.output_tokens),
        );

        let episode_id = self
            .brain
            .record_episode(&episode, &self.user_id)
            .ok();

        // Link tools to episode
        if let Some(ref ep_id) = episode_id {
            for tool_name in &tools_used {
                self.brain.link_tool_to_episode(tool_name, ep_id).ok();
            }
        }

        // Extract and store memories (async, don't block response)
        if let (Some(ref extractor), Some(ref ep_id)) = (&self.memory_extractor, &episode_id) {
            let brain = self.brain.clone();
            let user_id = self.user_id.clone();
            let user_input = user_message.to_string();
            let agent_response = response_text.clone();
            let tools = tools_used.clone();
            let episode_id = ep_id.clone();
            let extractor_clone = extractor.clone();

            // Spawn memory extraction as background task
            tokio::spawn(async move {
                if let Err(e) = extractor_clone
                    .extract_and_store(
                        &brain,
                        &user_id,
                        &user_input,
                        &agent_response,
                        &tools,
                        &episode_id,
                    )
                    .await
                {
                    warn!("Memory extraction failed: {}", e);
                }
            });
        }

        Ok(AgentResult {
            response: response_text,
            usage: total_usage,
            tools_used,
            episode_id,
        })
    }

    /// Execute a tool by dispatching to the appropriate handler
    async fn execute_tool(
        &self,
        name: &str,
        tool_use_id: &str,
        input: std::collections::HashMap<String, serde_json::Value>,
    ) -> crate::types::ToolResult {
        // Get the tool type
        let tool_type = self.registry.get_tool_type(name);

        match tool_type {
            Some(ToolType::Native) | Some(ToolType::Browser) => {
                // Execute via registry (includes browser)
                self.registry.execute(tool_use_id, name, input).await
            }
            Some(ToolType::Mcp) => {
                // Execute via MCP bridge
                if let Some(ref bridge) = self.mcp_bridge {
                    let input_value = serde_json::to_value(&input).unwrap_or_default();
                    match bridge.lock().await.call_tool(name, &input_value).await {
                        Ok(content) => {
                            crate::types::ToolResult::success(tool_use_id.to_string(), content)
                        }
                        Err(e) => {
                            crate::types::ToolResult::error(tool_use_id.to_string(), e.to_string())
                        }
                    }
                } else {
                    crate::types::ToolResult::error(
                        tool_use_id.to_string(),
                        "MCP bridge not configured".to_string(),
                    )
                }
            }
            Some(ToolType::Skill) => {
                // Execute via skill manager
                if let Some(ref manager) = self.skill_manager {
                    let input_value = serde_json::to_value(&input).unwrap_or_default();
                    match manager.lock().await.execute(name, &input_value).await {
                        Ok(content) => {
                            crate::types::ToolResult::success(tool_use_id.to_string(), content)
                        }
                        Err(e) => {
                            crate::types::ToolResult::error(tool_use_id.to_string(), e.to_string())
                        }
                    }
                } else {
                    crate::types::ToolResult::error(
                        tool_use_id.to_string(),
                        "Skill manager not configured".to_string(),
                    )
                }
            }
            None => {
                // Unknown tool - try registry anyway
                self.registry.execute(tool_use_id, name, input).await
            }
        }
    }

    /// Get the tool registry
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// Get a mutable reference to the tool registry
    pub fn registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.registry
    }

    /// Get the graph brain
    pub fn brain(&self) -> &Arc<GraphBrain> {
        &self.brain
    }
}

/// Estimate cost in USD based on token usage
fn estimate_cost(input_tokens: u32, output_tokens: u32) -> f64 {
    // Claude Sonnet pricing (approximate)
    const INPUT_COST_PER_MILLION: f64 = 3.0;
    const OUTPUT_COST_PER_MILLION: f64 = 15.0;

    let input_cost = (input_tokens as f64 / 1_000_000.0) * INPUT_COST_PER_MILLION;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * OUTPUT_COST_PER_MILLION;
    input_cost + output_cost
}

