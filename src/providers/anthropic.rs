//! Anthropic Claude provider implementation.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, warn};

use super::{
    ContentBlock, ContentBlockInput, LLMProvider, LLMResponse, Message, MessageContent,
    MessageRole, ProviderConfig, ProviderType, StopReason, TokenUsage,
};
use crate::types::{AgentError, ToolDefinition};

const API_VERSION: &str = "2023-06-01";
const MAX_RETRIES: u32 = 3;

pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    base_url: String,
    model: String,
    max_tokens: u32,
    temperature: Option<f32>,
}

impl AnthropicProvider {
    pub fn new(config: ProviderConfig) -> Result<Self, AgentError> {
        if config.api_key.is_empty() {
            return Err(AgentError::ConfigError(
                "Anthropic API key is required".to_string(),
            ));
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| AgentError::ConfigError(format!("Failed to create HTTP client: {}", e)))?;

        let base_url = config
            .base_url
            .unwrap_or_else(|| ProviderType::Anthropic.default_base_url().to_string());

        Ok(Self {
            client,
            api_key: config.api_key,
            base_url,
            model: config.model,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
        })
    }

    fn convert_messages(&self, messages: &[Message]) -> Vec<AnthropicMessage> {
        messages
            .iter()
            .filter(|m| m.role != MessageRole::System) // System handled separately
            .map(|m| {
                let role = match m.role {
                    MessageRole::User | MessageRole::Tool => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::System => "user", // Won't happen due to filter
                };

                let content = match &m.content {
                    MessageContent::Text(text) => serde_json::json!(text),
                    MessageContent::Blocks(blocks) => {
                        let converted: Vec<serde_json::Value> = blocks
                            .iter()
                            .map(|b| match b {
                                ContentBlockInput::Text { text } => {
                                    serde_json::json!({"type": "text", "text": text})
                                }
                                ContentBlockInput::ToolUse { id, name, input } => {
                                    serde_json::json!({
                                        "type": "tool_use",
                                        "id": id,
                                        "name": name,
                                        "input": input
                                    })
                                }
                                ContentBlockInput::ToolResult {
                                    tool_use_id,
                                    content,
                                    is_error,
                                } => {
                                    serde_json::json!({
                                        "type": "tool_result",
                                        "tool_use_id": tool_use_id,
                                        "content": content,
                                        "is_error": is_error.unwrap_or(false)
                                    })
                                }
                            })
                            .collect();
                        serde_json::Value::Array(converted)
                    }
                };

                AnthropicMessage {
                    role: role.to_string(),
                    content,
                }
            })
            .collect()
    }

    fn convert_tools(&self, tools: &[ToolDefinition]) -> Vec<AnthropicTool> {
        tools
            .iter()
            .map(|t| AnthropicTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
            })
            .collect()
    }

    async fn send_request(
        &self,
        request: &AnthropicRequest,
    ) -> Result<AnthropicResponse, AgentError> {
        let url = format!("{}/v1/messages", self.base_url);

        let mut retries = 0;
        loop {
            let response = self
                .client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION)
                .header("content-type", "application/json")
                .json(request)
                .send()
                .await?;

            let status = response.status();

            if status.is_success() {
                let body = response.text().await?;
                debug!("Anthropic response: {}", &body[..body.len().min(500)]);

                let parsed: AnthropicResponse = serde_json::from_str(&body)?;
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
                "Anthropic API error {}: {}",
                status, error_body
            )));
        }
    }
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "Anthropic"
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
        let anthropic_messages = self.convert_messages(messages);
        let anthropic_tools = if tools.is_empty() {
            None
        } else {
            Some(self.convert_tools(tools))
        };

        let request = AnthropicRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system: system_prompt.map(|s| s.to_string()),
            messages: anthropic_messages,
            tools: anthropic_tools,
            temperature: self.temperature,
        };

        let response = self.send_request(&request).await?;

        // Convert response
        let content: Vec<ContentBlock> = response
            .content
            .into_iter()
            .map(|block| match block {
                AnthropicContentBlock::Text { text } => ContentBlock::Text(text),
                AnthropicContentBlock::ToolUse { id, name, input } => {
                    let arguments: HashMap<String, serde_json::Value> = input
                        .as_object()
                        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default();
                    ContentBlock::ToolCall { id, name, arguments }
                }
            })
            .collect();

        let stop_reason = match response.stop_reason.as_str() {
            "end_turn" => StopReason::EndTurn,
            "tool_use" => StopReason::ToolUse,
            "max_tokens" => StopReason::MaxTokens,
            "stop_sequence" => StopReason::StopSequence,
            other => StopReason::Error(format!("Unknown stop reason: {}", other)),
        };

        let usage = TokenUsage {
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
            total_tokens: response.usage.input_tokens + response.usage.output_tokens,
        };

        Ok(LLMResponse {
            content,
            stop_reason,
            usage,
            model: self.model.clone(),
        })
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn pricing(&self) -> (f64, f64) {
        // Claude Sonnet pricing
        if self.model.contains("opus") {
            (15.0, 75.0)
        } else if self.model.contains("sonnet") {
            (3.0, 15.0)
        } else if self.model.contains("haiku") {
            (0.25, 1.25)
        } else {
            (3.0, 15.0)
        }
    }
}

// Anthropic API types
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
    stop_reason: String,
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}
