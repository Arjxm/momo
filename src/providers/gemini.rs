//! Google Gemini provider implementation.

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

const MAX_RETRIES: u32 = 3;

pub struct GeminiProvider {
    client: Client,
    api_key: String,
    base_url: String,
    model: String,
    max_tokens: u32,
    temperature: Option<f32>,
}

impl GeminiProvider {
    pub fn new(config: ProviderConfig) -> Result<Self, AgentError> {
        if config.api_key.is_empty() {
            return Err(AgentError::ConfigError(
                "Google Gemini API key is required".to_string(),
            ));
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| AgentError::ConfigError(format!("Failed to create HTTP client: {}", e)))?;

        let base_url = config
            .base_url
            .unwrap_or_else(|| ProviderType::Gemini.default_base_url().to_string());

        Ok(Self {
            client,
            api_key: config.api_key,
            base_url,
            model: config.model,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
        })
    }

    fn convert_messages(
        &self,
        messages: &[Message],
        system_prompt: Option<&str>,
    ) -> (Option<GeminiSystemInstruction>, Vec<GeminiContent>) {
        let system_instruction = system_prompt.map(|s| GeminiSystemInstruction {
            parts: vec![GeminiPart::Text { text: s.to_string() }],
        });

        let mut contents = Vec::new();

        for m in messages {
            if m.role == MessageRole::System {
                continue; // Already handled
            }

            let role = match m.role {
                MessageRole::User | MessageRole::Tool => "user",
                MessageRole::Assistant => "model",
                MessageRole::System => continue,
            };

            let parts = match &m.content {
                MessageContent::Text(text) => {
                    vec![GeminiPart::Text { text: text.clone() }]
                }
                MessageContent::Blocks(blocks) => {
                    blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlockInput::Text { text } => {
                                Some(GeminiPart::Text { text: text.clone() })
                            }
                            ContentBlockInput::ToolUse { id, name, input } => {
                                Some(GeminiPart::FunctionCall {
                                    function_call: GeminiFunctionCall {
                                        name: name.clone(),
                                        args: input.clone(),
                                    },
                                })
                            }
                            ContentBlockInput::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } => {
                                // Find the function name from context (use tool_use_id as fallback)
                                Some(GeminiPart::FunctionResponse {
                                    function_response: GeminiFunctionResponse {
                                        name: tool_use_id.clone(),
                                        response: serde_json::json!({ "result": content }),
                                    },
                                })
                            }
                        })
                        .collect()
                }
            };

            if !parts.is_empty() {
                contents.push(GeminiContent {
                    role: role.to_string(),
                    parts,
                });
            }
        }

        (system_instruction, contents)
    }

    fn convert_tools(&self, tools: &[ToolDefinition]) -> Vec<GeminiTool> {
        if tools.is_empty() {
            return vec![];
        }

        let function_declarations: Vec<GeminiFunctionDeclaration> = tools
            .iter()
            .map(|t| GeminiFunctionDeclaration {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: Some(Self::clean_schema_for_gemini(&t.input_schema)),
            })
            .collect();

        vec![GeminiTool {
            function_declarations,
        }]
    }

    /// Clean JSON Schema to remove fields not supported by Gemini API
    fn clean_schema_for_gemini(schema: &serde_json::Value) -> serde_json::Value {
        match schema {
            serde_json::Value::Object(obj) => {
                let mut cleaned = serde_json::Map::new();
                for (key, value) in obj {
                    // Skip unsupported fields
                    if key == "additionalProperties"
                        || key == "$schema"
                        || key == "$id"
                        || key == "$ref"
                        || key == "default"
                    {
                        continue;
                    }
                    // Recursively clean nested objects
                    cleaned.insert(key.clone(), Self::clean_schema_for_gemini(value));
                }
                serde_json::Value::Object(cleaned)
            }
            serde_json::Value::Array(arr) => {
                serde_json::Value::Array(
                    arr.iter().map(Self::clean_schema_for_gemini).collect()
                )
            }
            other => other.clone(),
        }
    }

    async fn send_request(&self, request: &GeminiRequest) -> Result<GeminiResponse, AgentError> {
        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, self.model, self.api_key
        );

        let mut retries = 0;
        loop {
            let response = self
                .client
                .post(&url)
                .header("content-type", "application/json")
                .json(request)
                .send()
                .await?;

            let status = response.status();

            if status.is_success() {
                let body = response.text().await?;
                debug!("Gemini response: {}", &body[..body.len().min(500)]);

                let parsed: GeminiResponse = serde_json::from_str(&body).map_err(|e| {
                    AgentError::ParseError(format!("Failed to parse Gemini response: {} - {}", e, body))
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
                "Gemini API error {}: {}",
                status, error_body
            )));
        }
    }
}

#[async_trait]
impl LLMProvider for GeminiProvider {
    fn name(&self) -> &str {
        "Google Gemini"
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
        let (system_instruction, contents) = self.convert_messages(messages, system_prompt);
        let gemini_tools = self.convert_tools(tools);

        let request = GeminiRequest {
            contents,
            system_instruction,
            tools: if gemini_tools.is_empty() {
                None
            } else {
                Some(gemini_tools)
            },
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: Some(self.max_tokens),
                temperature: self.temperature,
            }),
        };

        let response = self.send_request(&request).await?;

        // Get the first candidate
        let candidate = response
            .candidates
            .into_iter()
            .next()
            .ok_or_else(|| AgentError::ApiError("No candidates in Gemini response".to_string()))?;

        // Convert content
        let mut content = Vec::new();

        for part in candidate.content.parts {
            match part {
                GeminiPart::Text { text } => {
                    content.push(ContentBlock::Text(text));
                }
                GeminiPart::FunctionCall { function_call } => {
                    let arguments: HashMap<String, serde_json::Value> = function_call
                        .args
                        .as_object()
                        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default();

                    content.push(ContentBlock::ToolCall {
                        id: uuid::Uuid::new_v4().to_string(), // Gemini doesn't provide IDs
                        name: function_call.name,
                        arguments,
                    });
                }
                GeminiPart::FunctionResponse { .. } => {
                    // Skip function responses in output
                }
            }
        }

        let stop_reason = match candidate.finish_reason.as_deref() {
            Some("STOP") => StopReason::EndTurn,
            Some("MAX_TOKENS") => StopReason::MaxTokens,
            _ => {
                if content.iter().any(|c| matches!(c, ContentBlock::ToolCall { .. })) {
                    StopReason::ToolUse
                } else {
                    StopReason::EndTurn
                }
            }
        };

        let usage = response.usage_metadata.map(|u| TokenUsage {
            input_tokens: u.prompt_token_count,
            output_tokens: u.candidates_token_count,
            total_tokens: u.total_token_count,
        }).unwrap_or_default();

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
        // Gemini pricing (per million tokens)
        if self.model.contains("flash") {
            (0.075, 0.30) // Flash is very cheap
        } else if self.model.contains("pro") {
            (1.25, 5.0)
        } else {
            (0.075, 0.30)
        }
    }
}

// Gemini API types
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiSystemInstruction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Debug, Serialize)]
struct GeminiSystemInstruction {
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum GeminiPart {
    Text {
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    FunctionCall {
        function_call: GeminiFunctionCall,
    },
    #[serde(rename_all = "camelCase")]
    FunctionResponse {
        function_response: GeminiFunctionResponse,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiFunctionResponse {
    name: String,
    response: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiTool {
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Serialize)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: GeminiContent,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    prompt_token_count: u32,
    candidates_token_count: u32,
    total_token_count: u32,
}
