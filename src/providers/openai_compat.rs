//! OpenAI-compatible provider implementation.
//!
//! Works with: OpenAI, DeepSeek, Ollama, OpenRouter, Groq, Together AI, and any
//! provider that implements the OpenAI chat completions API format.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::{
    ContentBlock, ContentBlockInput, LLMProvider, LLMResponse, Message, MessageContent,
    MessageRole, ProviderConfig, ProviderType, StopReason, TokenUsage,
};
use crate::types::{AgentError, ToolDefinition};

const MAX_RETRIES: u32 = 3;

pub struct OpenAICompatibleProvider {
    client: Client,
    api_key: String,
    base_url: String,
    model: String,
    max_tokens: u32,
    temperature: Option<f32>,
    provider_type: ProviderType,
}

impl OpenAICompatibleProvider {
    pub fn new(config: ProviderConfig) -> Result<Self, AgentError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| AgentError::ConfigError(format!("Failed to create HTTP client: {}", e)))?;

        let base_url = config
            .base_url
            .unwrap_or_else(|| config.provider_type.default_base_url().to_string());

        // Local providers don't require API key
        if config.api_key.is_empty()
            && config.provider_type != ProviderType::Ollama
            && config.provider_type != ProviderType::LMStudio
        {
            return Err(AgentError::ConfigError(format!(
                "{} API key is required",
                config.provider_type
            )));
        }

        Ok(Self {
            client,
            api_key: config.api_key,
            base_url,
            model: config.model,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            provider_type: config.provider_type,
        })
    }

    fn convert_messages(
        &self,
        messages: &[Message],
        system_prompt: Option<&str>,
    ) -> Vec<OpenAIMessage> {
        let mut result = Vec::new();

        // Add system message if provided
        if let Some(system) = system_prompt {
            result.push(OpenAIMessage {
                role: "system".to_string(),
                content: Some(serde_json::Value::String(system.to_string())),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            });
        }

        for m in messages {
            match (&m.role, &m.content) {
                (MessageRole::System, MessageContent::Text(text)) => {
                    // System messages already handled above, but handle extras
                    result.push(OpenAIMessage {
                        role: "system".to_string(),
                        content: Some(serde_json::Value::String(text.clone())),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                    });
                }
                (MessageRole::User, MessageContent::Text(text)) => {
                    result.push(OpenAIMessage {
                        role: "user".to_string(),
                        content: Some(serde_json::Value::String(text.clone())),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                    });
                }
                (MessageRole::User, MessageContent::Blocks(blocks)) => {
                    // Handle tool results
                    for block in blocks {
                        if let ContentBlockInput::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } = block
                        {
                            result.push(OpenAIMessage {
                                role: "tool".to_string(),
                                content: Some(serde_json::Value::String(content.clone())),
                                tool_calls: None,
                                tool_call_id: Some(tool_use_id.clone()),
                                name: None,
                            });
                        }
                    }
                }
                (MessageRole::Assistant, MessageContent::Text(text)) => {
                    result.push(OpenAIMessage {
                        role: "assistant".to_string(),
                        content: Some(serde_json::Value::String(text.clone())),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                    });
                }
                (MessageRole::Assistant, MessageContent::Blocks(blocks)) => {
                    // Handle assistant messages with tool calls
                    let mut text_content = String::new();
                    let mut tool_calls = Vec::new();

                    for block in blocks {
                        match block {
                            ContentBlockInput::Text { text } => {
                                text_content.push_str(text);
                            }
                            ContentBlockInput::ToolUse { id, name, input } => {
                                tool_calls.push(OpenAIToolCall {
                                    id: id.clone(),
                                    r#type: "function".to_string(),
                                    function: OpenAIFunctionCall {
                                        name: name.clone(),
                                        arguments: serde_json::to_string(input)
                                            .unwrap_or_default(),
                                    },
                                });
                            }
                            _ => {}
                        }
                    }

                    result.push(OpenAIMessage {
                        role: "assistant".to_string(),
                        content: if text_content.is_empty() {
                            None
                        } else {
                            Some(serde_json::Value::String(text_content))
                        },
                        tool_calls: if tool_calls.is_empty() {
                            None
                        } else {
                            Some(tool_calls)
                        },
                        tool_call_id: None,
                        name: None,
                    });
                }
                (MessageRole::Tool, MessageContent::Text(text)) => {
                    result.push(OpenAIMessage {
                        role: "tool".to_string(),
                        content: Some(serde_json::Value::String(text.clone())),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                    });
                }
                _ => {}
            }
        }

        result
    }

    fn convert_tools(&self, tools: &[ToolDefinition]) -> Vec<OpenAITool> {
        tools
            .iter()
            .map(|t| OpenAITool {
                r#type: "function".to_string(),
                function: OpenAIFunction {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.input_schema.clone(),
                },
            })
            .collect()
    }

    async fn send_request(&self, request: &OpenAIRequest) -> Result<OpenAIResponse, AgentError> {
        let url = format!("{}/chat/completions", self.base_url);
        debug!("🌐 [OPENAI] Sending request to: {}", url);
        debug!("🌐 [OPENAI] Model: {}, Messages: {}, Tools: {}",
            request.model,
            request.messages.len(),
            request.tools.as_ref().map(|t| t.len()).unwrap_or(0)
        );

        let mut retries = 0;
        loop {
            let mut req = self
                .client
                .post(&url)
                .header("content-type", "application/json");

            // Add authorization header based on provider
            if !self.api_key.is_empty() {
                req = match self.provider_type {
                    ProviderType::OpenRouter => {
                        req.header("Authorization", format!("Bearer {}", self.api_key))
                            .header("HTTP-Referer", "https://github.com/agent-brain")
                            .header("X-Title", "Agent Brain")
                    }
                    _ => req.header("Authorization", format!("Bearer {}", self.api_key)),
                };
            }

            debug!("🌐 [OPENAI] Sending HTTP request...");
            let response = req.json(request).send().await?;
            let status = response.status();
            debug!("🌐 [OPENAI] Response status: {}", status);

            if status.is_success() {
                let body = response.text().await?;
                debug!("OpenAI-compat response: {}", &body[..body.len().min(500)]);

                let parsed: OpenAIResponse = serde_json::from_str(&body).map_err(|e| {
                    AgentError::ParseError(format!("Failed to parse response: {} - {}", e, body))
                })?;
                return Ok(parsed);
            }

            let error_body = response.text().await.unwrap_or_default();

            // Handle rate limiting
            if status.as_u16() == 429 && retries < MAX_RETRIES {
                retries += 1;
                let delay = Duration::from_secs(2u64.pow(retries));
                warn!("Rate limited, retrying in {:?}...", delay);
                tokio::time::sleep(delay).await;
                continue;
            }

            // Handle server errors
            if status.is_server_error() && retries < MAX_RETRIES {
                retries += 1;
                let delay = Duration::from_secs(2);
                warn!("Server error {}, retrying in {:?}...", status, delay);
                tokio::time::sleep(delay).await;
                continue;
            }

            return Err(AgentError::ApiError(format!(
                "{} API error {}: {}",
                self.provider_type, status, error_body
            )));
        }
    }
}

#[async_trait]
impl LLMProvider for OpenAICompatibleProvider {
    fn name(&self) -> &str {
        match self.provider_type {
            ProviderType::OpenAI => "OpenAI",
            ProviderType::DeepSeek => "DeepSeek",
            ProviderType::Ollama => "Ollama",
            ProviderType::OpenRouter => "OpenRouter",
            ProviderType::Groq => "Groq",
            ProviderType::Together => "Together AI",
            ProviderType::LMStudio => "LM Studio",
            ProviderType::Custom => "Custom",
            _ => "OpenAI-Compatible",
        }
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_prompt: Option<&str>,
    ) -> Result<LLMResponse, AgentError> {
        let openai_messages = self.convert_messages(messages, system_prompt);
        let openai_tools = if tools.is_empty() {
            None
        } else {
            Some(self.convert_tools(tools))
        };

        let request = OpenAIRequest {
            model: self.model.clone(),
            messages: openai_messages,
            tools: openai_tools,
            max_tokens: Some(self.max_tokens),
            temperature: self.temperature,
        };

        let response = self.send_request(&request).await?;

        // Get the first choice
        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| AgentError::ApiError("No choices in response".to_string()))?;

        // Convert content
        let mut content = Vec::new();

        if let Some(text) = choice.message.content {
            if !text.is_empty() {
                content.push(ContentBlock::Text(text));
            }
        }

        if let Some(tool_calls) = choice.message.tool_calls {
            for tc in tool_calls {
                let arguments: HashMap<String, serde_json::Value> =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                content.push(ContentBlock::ToolCall {
                    id: tc.id,
                    name: tc.function.name,
                    arguments,
                });
            }
        }

        let stop_reason = match choice.finish_reason.as_deref() {
            Some("stop") => StopReason::EndTurn,
            Some("tool_calls") => StopReason::ToolUse,
            Some("length") => StopReason::MaxTokens,
            Some(other) => {
                // Check if we have tool calls even without explicit finish reason
                if content.iter().any(|c| matches!(c, ContentBlock::ToolCall { .. })) {
                    StopReason::ToolUse
                } else {
                    StopReason::Error(format!("Unknown finish reason: {}", other))
                }
            }
            None => {
                if content.iter().any(|c| matches!(c, ContentBlock::ToolCall { .. })) {
                    StopReason::ToolUse
                } else {
                    StopReason::EndTurn
                }
            }
        };

        let usage = response.usage.map(|u| TokenUsage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        }).unwrap_or_default();

        Ok(LLMResponse {
            content,
            stop_reason,
            usage,
            model: self.model.clone(),
        })
    }

    fn supports_tools(&self) -> bool {
        // Most OpenAI-compatible providers support tools
        // Ollama depends on the model
        true
    }

    fn pricing(&self) -> (f64, f64) {
        match self.provider_type {
            ProviderType::OpenAI => {
                if self.model.contains("gpt-4o-mini") {
                    (0.15, 0.60)
                } else if self.model.contains("gpt-4o") {
                    (2.50, 10.0)
                } else if self.model.contains("gpt-4-turbo") {
                    (10.0, 30.0)
                } else if self.model.contains("o1") {
                    (15.0, 60.0)
                } else {
                    (5.0, 15.0)
                }
            }
            ProviderType::DeepSeek => (0.27, 1.10), // Very cheap
            ProviderType::Ollama => (0.0, 0.0),     // Free (local)
            ProviderType::LMStudio => (0.0, 0.0),   // Free (local)
            ProviderType::Groq => (0.05, 0.08),    // Very fast and cheap
            ProviderType::Together => (0.90, 0.90),
            ProviderType::OpenRouter => (1.0, 3.0), // Varies by model
            _ => (1.0, 3.0),
        }
    }
}

// OpenAI API types
#[derive(Debug, Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIToolCall {
    id: String,
    r#type: String,
    function: OpenAIFunctionCall,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct OpenAITool {
    r#type: String,
    function: OpenAIFunction,
}

#[derive(Debug, Serialize)]
struct OpenAIFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: OpenAIResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}
