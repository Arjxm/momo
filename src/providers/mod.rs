//! Multi-provider LLM support.
//!
//! Supports multiple LLM providers through a unified interface:
//! - Anthropic (Claude)
//! - OpenAI (GPT-4, GPT-4o, etc.)
//! - Google Gemini
//! - DeepSeek
//! - Ollama (local LLMs: Qwen, Llama, Mistral, etc.)
//! - OpenRouter (500+ models)
//! - Groq (fast inference)
//! - Together AI

pub mod anthropic;
pub mod gemini;
pub mod openai_compat;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::{AgentError, ToolDefinition};

// Re-exports
pub use anthropic::AnthropicProvider;
pub use gemini::GeminiProvider;
pub use openai_compat::OpenAICompatibleProvider;

/// Unified response from any LLM provider
#[derive(Debug, Clone)]
pub struct LLMResponse {
    pub content: Vec<ContentBlock>,
    pub stop_reason: StopReason,
    pub usage: TokenUsage,
    pub model: String,
}

/// Content block types (unified across providers)
#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        arguments: HashMap<String, serde_json::Value>,
    },
}

/// Why the model stopped generating
#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    Error(String),
}

impl StopReason {
    pub fn as_str(&self) -> &str {
        match self {
            StopReason::EndTurn => "end_turn",
            StopReason::ToolUse => "tool_use",
            StopReason::MaxTokens => "max_tokens",
            StopReason::StopSequence => "stop_sequence",
            StopReason::Error(_) => "error",
        }
    }
}

/// Token usage information
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

impl TokenUsage {
    pub fn add(&mut self, other: &TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.total_tokens += other.total_tokens;
    }
}

/// A message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlockInput>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlockInput {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

impl Message {
    pub fn system(text: &str) -> Self {
        Self {
            role: MessageRole::System,
            content: MessageContent::Text(text.to_string()),
        }
    }

    pub fn user(text: &str) -> Self {
        Self {
            role: MessageRole::User,
            content: MessageContent::Text(text.to_string()),
        }
    }

    pub fn assistant_text(text: &str) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: MessageContent::Text(text.to_string()),
        }
    }

    pub fn assistant_blocks(blocks: Vec<ContentBlockInput>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: MessageContent::Blocks(blocks),
        }
    }

    pub fn tool_result(tool_use_id: &str, content: &str, is_error: bool) -> Self {
        Self {
            role: MessageRole::User,
            content: MessageContent::Blocks(vec![ContentBlockInput::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.to_string(),
                is_error: if is_error { Some(true) } else { None },
            }]),
        }
    }

    pub fn tool_results(results: Vec<(String, String, bool)>) -> Self {
        let blocks = results
            .into_iter()
            .map(|(id, content, is_error)| ContentBlockInput::ToolResult {
                tool_use_id: id,
                content,
                is_error: if is_error { Some(true) } else { None },
            })
            .collect();
        Self {
            role: MessageRole::User,
            content: MessageContent::Blocks(blocks),
        }
    }
}

impl LLMResponse {
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

    /// Extract all tool calls
    pub fn tool_calls(&self) -> Vec<ToolCall> {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolCall { id, name, arguments } => Some(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    /// Check if response contains tool calls
    pub fn has_tool_calls(&self) -> bool {
        self.content.iter().any(|b| matches!(b, ContentBlock::ToolCall { .. }))
    }
}

/// A tool call from the model
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: HashMap<String, serde_json::Value>,
}

/// Configuration for an LLM provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: ProviderType,
    pub api_key: String,
    #[serde(default)]
    pub base_url: Option<String>,
    pub model: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub extra: HashMap<String, serde_json::Value>,
}

fn default_max_tokens() -> u32 {
    4096
}

/// Supported provider types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    Anthropic,
    OpenAI,
    Gemini,
    DeepSeek,
    Ollama,
    OpenRouter,
    Groq,
    Together,
    LMStudio,
    Custom,
}

impl ProviderType {
    pub fn default_base_url(&self) -> &str {
        match self {
            ProviderType::Anthropic => "https://api.anthropic.com",
            ProviderType::OpenAI => "https://api.openai.com/v1",
            ProviderType::Gemini => "https://generativelanguage.googleapis.com/v1beta",
            ProviderType::DeepSeek => "https://api.deepseek.com",
            ProviderType::Ollama => "http://localhost:11434/v1",
            ProviderType::OpenRouter => "https://openrouter.ai/api/v1",
            ProviderType::Groq => "https://api.groq.com/openai/v1",
            ProviderType::Together => "https://api.together.xyz/v1",
            ProviderType::LMStudio => "http://localhost:1234/v1",
            ProviderType::Custom => "",
        }
    }

    pub fn default_model(&self) -> &str {
        match self {
            ProviderType::Anthropic => "claude-sonnet-4-20250514",
            ProviderType::OpenAI => "gpt-4o",
            ProviderType::Gemini => "gemini-2.0-flash",
            ProviderType::DeepSeek => "deepseek-chat",
            ProviderType::Ollama => "qwen2.5:latest",
            ProviderType::OpenRouter => "anthropic/claude-3.5-sonnet",
            ProviderType::Groq => "llama-3.3-70b-versatile",
            ProviderType::Together => "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            ProviderType::LMStudio => "local-model",
            ProviderType::Custom => "gpt-4o",
        }
    }

    /// Check if this provider uses OpenAI-compatible API
    pub fn is_openai_compatible(&self) -> bool {
        matches!(
            self,
            ProviderType::OpenAI
                | ProviderType::DeepSeek
                | ProviderType::Ollama
                | ProviderType::OpenRouter
                | ProviderType::Groq
                | ProviderType::Together
                | ProviderType::LMStudio
                | ProviderType::Custom
        )
    }
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderType::Anthropic => write!(f, "Anthropic"),
            ProviderType::OpenAI => write!(f, "OpenAI"),
            ProviderType::Gemini => write!(f, "Google Gemini"),
            ProviderType::DeepSeek => write!(f, "DeepSeek"),
            ProviderType::Ollama => write!(f, "Ollama (Local)"),
            ProviderType::OpenRouter => write!(f, "OpenRouter"),
            ProviderType::Groq => write!(f, "Groq"),
            ProviderType::Together => write!(f, "Together AI"),
            ProviderType::LMStudio => write!(f, "LM Studio (Local)"),
            ProviderType::Custom => write!(f, "Custom"),
        }
    }
}

/// The unified LLM provider trait
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Get the provider name
    fn name(&self) -> &str;

    /// Get the current model
    fn model(&self) -> &str;

    /// Send a message and get a response
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_prompt: Option<&str>,
    ) -> Result<LLMResponse, AgentError>;

    /// Check if the provider supports tool calling
    fn supports_tools(&self) -> bool {
        true
    }

    /// Get pricing info (cost per million tokens)
    fn pricing(&self) -> (f64, f64) {
        // Default: (input_cost, output_cost) per million tokens
        (1.0, 3.0)
    }
}

/// Create a provider from configuration
pub fn create_provider(config: ProviderConfig) -> Result<Box<dyn LLMProvider>, AgentError> {
    match config.provider_type {
        ProviderType::Anthropic => {
            Ok(Box::new(AnthropicProvider::new(config)?))
        }
        ProviderType::Gemini => {
            Ok(Box::new(GeminiProvider::new(config)?))
        }
        _ if config.provider_type.is_openai_compatible() => {
            Ok(Box::new(OpenAICompatibleProvider::new(config)?))
        }
        _ => Err(AgentError::ConfigError(format!(
            "Unknown provider type: {:?}",
            config.provider_type
        ))),
    }
}

/// List of popular models by provider
pub fn available_models(provider: &ProviderType) -> Vec<&'static str> {
    match provider {
        ProviderType::Anthropic => vec![
            "claude-sonnet-4-20250514",
            "claude-opus-4-20250514",
            "claude-3-5-sonnet-20241022",
            "claude-3-5-haiku-20241022",
            "claude-3-opus-20240229",
        ],
        ProviderType::OpenAI => vec![
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4-turbo",
            "gpt-4",
            "o1-preview",
            "o1-mini",
        ],
        ProviderType::Gemini => vec![
            "gemini-2.0-flash",
            "gemini-2.0-flash-lite",
            "gemini-1.5-pro",
            "gemini-1.5-flash",
        ],
        ProviderType::DeepSeek => vec![
            "deepseek-chat",
            "deepseek-reasoner",
        ],
        ProviderType::Ollama => vec![
            "qwen2.5:latest",
            "qwen2.5:72b",
            "llama3.3:latest",
            "llama3.2:latest",
            "mistral:latest",
            "codellama:latest",
            "deepseek-r1:latest",
        ],
        ProviderType::OpenRouter => vec![
            "anthropic/claude-3.5-sonnet",
            "openai/gpt-4o",
            "google/gemini-2.0-flash-exp:free",
            "meta-llama/llama-3.3-70b-instruct",
            "deepseek/deepseek-r1",
        ],
        ProviderType::Groq => vec![
            "llama-3.3-70b-versatile",
            "llama-3.1-8b-instant",
            "mixtral-8x7b-32768",
            "gemma2-9b-it",
        ],
        ProviderType::Together => vec![
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            "meta-llama/Meta-Llama-3.1-405B-Instruct-Turbo",
            "Qwen/Qwen2.5-72B-Instruct-Turbo",
            "deepseek-ai/DeepSeek-R1",
        ],
        ProviderType::LMStudio => vec![
            // LM Studio uses whatever model you load locally
            // Common models: Llama, Mistral, Qwen, Phi, etc.
        ],
        ProviderType::Custom => vec![],
    }
}
