use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, warn};

use crate::types::{
    AgentConfig, AgentError, ClaudeResponse, ContentBlock, ConversationMessage, TokenUsage,
    ToolDefinition,
};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

/// Client for interacting with the Anthropic Claude API
pub struct ClaudeClient {
    client: reqwest::Client,
    api_key: String,
    config: AgentConfig,
}

impl ClaudeClient {
    pub fn new(api_key: String, config: AgentConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            config,
        }
    }

    /// Get the API key
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Send a message to Claude and get a response
    pub async fn send_message(
        &self,
        messages: &[ConversationMessage],
        tools: &[ToolDefinition],
    ) -> Result<ClaudeResponse, AgentError> {
        let mut request_body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "messages": messages,
        });

        if let Some(ref system) = self.config.system_prompt {
            request_body["system"] = serde_json::Value::String(system.clone());
        }

        if !tools.is_empty() {
            request_body["tools"] = serde_json::to_value(tools)?;
        }

        debug!("Sending request to Claude API");

        // Retry logic
        let mut retries = 0;
        let max_retries = 3;

        loop {
            let response = self
                .client
                .post(API_URL)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION)
                .header("content-type", "application/json")
                .json(&request_body)
                .send()
                .await?;

            let status = response.status();

            if status.is_success() {
                let response_body: serde_json::Value = response.json().await?;
                return parse_response(response_body);
            }

            // Handle retryable errors
            if status.as_u16() == 429 {
                // Rate limited - exponential backoff
                if retries < max_retries {
                    let delay = Duration::from_secs(1 << retries);
                    warn!("Rate limited, retrying in {:?}", delay);
                    tokio::time::sleep(delay).await;
                    retries += 1;
                    continue;
                }
            } else if status.is_server_error() {
                // 5xx error - retry once after 2s
                if retries < 1 {
                    warn!("Server error {}, retrying in 2s", status);
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    retries += 1;
                    continue;
                }
            }

            // Non-retryable error or max retries reached
            let error_body = response.text().await.unwrap_or_default();
            return Err(AgentError::ApiError(format!(
                "API error {}: {}",
                status, error_body
            )));
        }
    }
}

fn parse_response(body: serde_json::Value) -> Result<ClaudeResponse, AgentError> {
    let content_array = body["content"]
        .as_array()
        .ok_or_else(|| AgentError::ParseError("Missing content array".to_string()))?;

    let mut content = Vec::new();

    for block in content_array {
        let block_type = block["type"].as_str().unwrap_or("");

        match block_type {
            "text" => {
                let text = block["text"].as_str().unwrap_or("").to_string();
                content.push(ContentBlock::Text(text));
            }
            "tool_use" => {
                let id = block["id"].as_str().unwrap_or("").to_string();
                let name = block["name"].as_str().unwrap_or("").to_string();
                let input: HashMap<String, serde_json::Value> = block["input"]
                    .as_object()
                    .map(|obj| {
                        obj.iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect()
                    })
                    .unwrap_or_default();

                content.push(ContentBlock::ToolUse { id, name, input });
            }
            _ => {
                debug!("Unknown content block type: {}", block_type);
            }
        }
    }

    let stop_reason = body["stop_reason"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    let usage = TokenUsage {
        input_tokens: body["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32,
        output_tokens: body["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32,
    };

    Ok(ClaudeResponse {
        content,
        stop_reason,
        usage,
    })
}
