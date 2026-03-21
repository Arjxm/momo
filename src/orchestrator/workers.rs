//! Worker agents for task execution

use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

use super::types::{AgentType, TaskNode, WorkerResult};
use crate::providers::{ContentBlock, ContentBlockInput, LLMProvider, Message, MessageContent, StopReason};
use crate::tools::ToolRegistry;
use crate::types::{AgentError, ToolDefinition, ToolResult};

/// A specialized worker agent that executes specific types of tasks
pub struct Worker {
    agent_type: AgentType,
    provider: Arc<dyn LLMProvider>,
    available_tools: Vec<ToolDefinition>,
}

impl Worker {
    pub fn new(
        agent_type: AgentType,
        provider: Arc<dyn LLMProvider>,
        available_tools: Vec<ToolDefinition>,
    ) -> Self {
        Self {
            agent_type,
            provider,
            available_tools,
        }
    }

    fn system_prompt(agent_type: &AgentType) -> String {
        match agent_type {
            AgentType::Research => r#"You are a Research Agent. Your specialization:
- Information gathering from web and documents
- Analyzing and summarizing findings
- Finding relevant papers, articles, and data

Be thorough but concise. Cite sources when possible. Focus on accuracy."#.to_string(),

            AgentType::Code => r#"You are a Code Agent. Your specialization:
- Writing clean, efficient code
- Analyzing and fixing bugs
- Creating and modifying files
- Following best practices

Write code that is readable and well-structured. Include brief comments for complex logic."#.to_string(),

            AgentType::Comms => r#"You are a Communications Agent. Your specialization:
- Drafting emails and messages
- Managing notifications
- Professional communication

Be clear and professional. Match the appropriate tone for the context."#.to_string(),

            AgentType::Data => r#"You are a Data Agent. Your specialization:
- Database queries and operations
- Data transformation and analysis
- Working with structured data

Be precise with queries. Validate data integrity."#.to_string(),

            AgentType::Browser => r#"You are a Browser Agent. Your specialization:
- Web automation and navigation
- Form filling and interaction
- Screenshot capture
- Data extraction from web pages

Be careful with navigation. Handle errors gracefully."#.to_string(),

            AgentType::Planner => r#"You are a Planner Agent. This should not happen - planners don't execute tasks."#.to_string(),
        }
    }

    pub fn agent_type(&self) -> &AgentType {
        &self.agent_type
    }

    /// Execute a task
    pub async fn execute(
        &self,
        task: &TaskNode,
        context: &str,
        tool_executor: &dyn ToolExecutor,
    ) -> WorkerResult {
        info!("[{}] Starting task: {}", self.agent_type.to_string().to_uppercase(), task.description);

        // Build the prompt with task details and context
        let prompt = self.build_prompt(task, context);
        let mut messages = vec![Message::user(&prompt)];
        let mut tools_used = Vec::new();
        let mut total_tokens = 0u32;
        let mut iterations = 0;
        let max_iterations = 10;
        let system_prompt = Self::system_prompt(&self.agent_type);

        loop {
            iterations += 1;
            if iterations > max_iterations {
                warn!("Worker reached max iterations for task {}", task.id);
                return WorkerResult::failure(
                    task.id.clone(),
                    "Max iterations reached".to_string(),
                );
            }

            let response = match self.provider.chat(&messages, &self.available_tools, Some(&system_prompt)).await {
                Ok(r) => r,
                Err(e) => {
                    return WorkerResult::failure(task.id.clone(), e.to_string());
                }
            };

            total_tokens += response.usage.input_tokens + response.usage.output_tokens;

            // Convert response to message for conversation history
            let assistant_blocks: Vec<ContentBlockInput> = response.content.iter().map(|block| {
                match block {
                    ContentBlock::Text(text) => ContentBlockInput::Text { text: text.clone() },
                    ContentBlock::ToolCall { id, name, arguments } => ContentBlockInput::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: serde_json::to_value(arguments).unwrap_or_default(),
                    },
                }
            }).collect();

            messages.push(Message::assistant_blocks(assistant_blocks));

            if response.stop_reason == StopReason::EndTurn {
                let output = response.text();
                info!("[{}] ✓ Completed task (used {} tools, {} tokens)",
                    self.agent_type.to_string().to_uppercase(),
                    tools_used.len(),
                    total_tokens
                );
                return WorkerResult::success(task.id.clone(), output, tools_used, total_tokens);
            }

            if response.stop_reason == StopReason::ToolUse {
                let tool_calls = response.tool_calls();
                let mut tool_results: Vec<(String, String, bool)> = Vec::new();

                for call in tool_calls {
                    info!("[{}] → Calling tool: {}", self.agent_type.to_string().to_uppercase(), call.name);
                    if !tools_used.contains(&call.name) {
                        tools_used.push(call.name.clone());
                    }

                    let result = tool_executor.execute(&call.name, &call.id, call.arguments).await;
                    if result.is_error {
                        warn!("[{}] ✗ Tool {} failed", self.agent_type.to_string().to_uppercase(), call.name);
                    }
                    tool_results.push((result.tool_use_id, result.content, result.is_error));
                }

                messages.push(Message::tool_results(tool_results));
            } else {
                warn!("Unexpected stop reason: {:?}", response.stop_reason);
                let output = response.text();
                return WorkerResult::success(task.id.clone(), output, tools_used, total_tokens);
            }
        }
    }

    fn build_prompt(&self, task: &TaskNode, context: &str) -> String {
        let mut prompt = format!(
            "TASK: {}\n\nINPUT CONTEXT:\n{}\n",
            task.description,
            task.input_context
        );

        if !context.is_empty() {
            prompt.push_str(&format!("\nPREVIOUS RESULTS:\n{}\n", context));
        }

        if let Some(ref tool) = task.tool_hint {
            prompt.push_str(&format!("\nSUGGESTED TOOL: {}\n", tool));
        }

        prompt.push_str("\nComplete this task and provide the result.");

        prompt
    }
}

/// Trait for tool execution (allows mocking in tests)
#[async_trait::async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(
        &self,
        name: &str,
        tool_use_id: &str,
        input: HashMap<String, serde_json::Value>,
    ) -> ToolResult;
}

/// Worker pool manages multiple worker agents
pub struct WorkerPool {
    workers: HashMap<AgentType, Worker>,
}

impl WorkerPool {
    pub fn new(provider: Arc<dyn LLMProvider>, registry: &ToolRegistry) -> Self {
        let mut workers = HashMap::new();

        // Create workers for each agent type with appropriate tools
        let agent_types = [
            AgentType::Research,
            AgentType::Code,
            AgentType::Comms,
            AgentType::Data,
            AgentType::Browser,
        ];

        for agent_type in agent_types {
            let tools = Self::tools_for_agent(&agent_type, registry);
            let worker = Worker::new(agent_type.clone(), provider.clone(), tools);
            workers.insert(agent_type, worker);
        }

        Self { workers }
    }

    /// Get tools appropriate for each agent type
    fn tools_for_agent(agent_type: &AgentType, registry: &ToolRegistry) -> Vec<ToolDefinition> {
        let all_tools = registry.definitions();

        match agent_type {
            AgentType::Research => {
                // Research gets: web_fetch, arxiv_search, MCP search tools
                all_tools
                    .into_iter()
                    .filter(|t| {
                        t.name.contains("search")
                            || t.name.contains("fetch")
                            || t.name.contains("arxiv")
                            || t.name.contains("brave")
                    })
                    .collect()
            }
            AgentType::Code => {
                // Code gets: file operations, calculator
                all_tools
                    .into_iter()
                    .filter(|t| {
                        t.name.contains("file")
                            || t.name.contains("write")
                            || t.name.contains("read")
                            || t.name.contains("calculator")
                            || t.name.contains("edit")
                            || t.name.contains("directory")
                    })
                    .collect()
            }
            AgentType::Browser => {
                // Browser gets: all browser and playwright tools
                all_tools
                    .into_iter()
                    .filter(|t| {
                        t.name.contains("browser")
                            || t.name.contains("navigate")
                            || t.name.contains("click")
                            || t.name.contains("screenshot")
                            || t.name.contains("playwright")
                    })
                    .collect()
            }
            AgentType::Data => {
                // Data gets: database tools
                all_tools
                    .into_iter()
                    .filter(|t| {
                        t.name.contains("sql")
                            || t.name.contains("query")
                            || t.name.contains("database")
                            || t.name.contains("sqlite")
                    })
                    .collect()
            }
            AgentType::Comms => {
                // Comms gets: email, messaging tools (may be empty initially)
                all_tools
                    .into_iter()
                    .filter(|t| {
                        t.name.contains("email")
                            || t.name.contains("message")
                            || t.name.contains("send")
                            || t.name.contains("notify")
                    })
                    .collect()
            }
            AgentType::Planner => vec![], // Planner doesn't use tools directly
        }
    }

    pub fn get_worker(&self, agent_type: &AgentType) -> Option<&Worker> {
        self.workers.get(agent_type)
    }

    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }
}
