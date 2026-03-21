use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::types::AgentError;

/// Configuration for MCP servers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPServersConfig {
    pub servers: Vec<MCPServerConfig>,
}

impl Default for MCPServersConfig {
    fn default() -> Self {
        Self { servers: vec![] }
    }
}

/// Configuration for a single MCP server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPServerConfig {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    pub transport: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_true")]
    pub auto_connect: bool,
}

fn default_true() -> bool {
    true
}

impl MCPServerConfig {
    /// Create a new stdio-based MCP server config
    pub fn stdio(name: &str, description: &str, command: &str, args: Vec<&str>, auto_connect: bool) -> Self {
        Self {
            name: name.to_string(),
            description: Some(description.to_string()),
            command: Some(command.to_string()),
            args: args.into_iter().map(|s| s.to_string()).collect(),
            url: None,
            transport: "stdio".to_string(),
            env: HashMap::new(),
            auto_connect,
        }
    }

    /// Add environment variable to config
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.insert(key.to_string(), value.to_string());
        self
    }
}

impl MCPServersConfig {
    /// Load MCP server configuration from a JSON file
    /// If file doesn't exist, returns default servers that ship with agent-brain
    pub fn load(path: &str) -> Result<Self, AgentError> {
        let path = Path::new(path);
        if !path.exists() {
            // Return default servers if no config file exists
            return Ok(Self::default_servers());
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| AgentError::ConfigError(format!("Failed to read MCP config: {}", e)))?;

        serde_json::from_str(&content)
            .map_err(|e| AgentError::ConfigError(format!("Failed to parse MCP config: {}", e)))
    }

    /// Default MCP servers that ship with agent-brain (zero API key required)
    /// These are verified working servers from the MCP ecosystem
    pub fn default_servers() -> Self {
        // Get home directory for filesystem access
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());

        Self {
            servers: vec![
                // Core utilities - verified working
                MCPServerConfig::stdio(
                    "memory",
                    "Persistent entity-relationship memory. Create entities, relations, observations.",
                    "npx",
                    vec!["-y", "@modelcontextprotocol/server-memory"],
                    true,
                ),
                MCPServerConfig::stdio(
                    "sequential-thinking",
                    "Structured step-by-step reasoning for complex problems.",
                    "npx",
                    vec!["-y", "@modelcontextprotocol/server-sequential-thinking"],
                    true,
                ),
                MCPServerConfig {
                    name: "filesystem".to_string(),
                    description: Some("Read, write, search files. Access restricted to home directory.".to_string()),
                    command: Some("npx".to_string()),
                    args: vec!["-y".to_string(), "@modelcontextprotocol/server-filesystem".to_string(), home],
                    url: None,
                    transport: "stdio".to_string(),
                    env: HashMap::new(),
                    auto_connect: true,
                },
                // Web search
                MCPServerConfig::stdio(
                    "duckduckgo",
                    "Privacy-focused web search. No API key needed.",
                    "npx",
                    vec!["-y", "duckduckgo-mcp-server"],
                    true,
                ),
                // Library docs
                MCPServerConfig::stdio(
                    "context7",
                    "Live docs for 9,000+ libraries. Eliminates hallucinated APIs.",
                    "npx",
                    vec!["-y", "@upstash/context7-mcp"],
                    true,
                ),
            ],
        }
    }

    /// Save MCP server configuration to a JSON file
    pub fn save(&self, path: &str) -> Result<(), AgentError> {
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| AgentError::ConfigError(format!("Failed to serialize MCP config: {}", e)))?;

        std::fs::write(path, content)
            .map_err(|e| AgentError::ConfigError(format!("Failed to write MCP config: {}", e)))
    }

    /// Add a new server to the configuration
    pub fn add_server(&mut self, server: MCPServerConfig) {
        // Remove existing server with same name
        self.servers.retain(|s| s.name != server.name);
        self.servers.push(server);
    }

    /// Get a server by name
    pub fn get_server(&self, name: &str) -> Option<&MCPServerConfig> {
        self.servers.iter().find(|s| s.name == name)
    }

    /// Get all servers configured for auto-connect
    pub fn auto_connect_servers(&self) -> Vec<&MCPServerConfig> {
        self.servers.iter().filter(|s| s.auto_connect).collect()
    }
}

/// Application-wide configuration
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub db_path: String,
    pub skills_dir: String,
    pub mcp_config_path: String,
    pub provider_config_path: String,
    pub default_user_id: String,
    pub provider: ProviderSettings,
}

/// LLM Provider settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSettings {
    pub provider_type: String,
    pub api_key: String,
    #[serde(default)]
    pub base_url: Option<String>,
    pub model: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub temperature: Option<f32>,
}

fn default_max_tokens() -> u32 {
    4096
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            provider_type: "anthropic".to_string(),
            api_key: String::new(),
            base_url: None,
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 4096,
            temperature: None,
        }
    }
}

impl ProviderSettings {
    /// Load provider settings from JSON file
    pub fn load(path: &str) -> Result<Self, AgentError> {
        let path = Path::new(path);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| AgentError::ConfigError(format!("Failed to read provider config: {}", e)))?;

        serde_json::from_str(&content)
            .map_err(|e| AgentError::ConfigError(format!("Failed to parse provider config: {}", e)))
    }

    /// Save provider settings to JSON file
    pub fn save(&self, path: &str) -> Result<(), AgentError> {
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| AgentError::ConfigError(format!("Failed to serialize provider config: {}", e)))?;

        std::fs::write(path, content)
            .map_err(|e| AgentError::ConfigError(format!("Failed to write provider config: {}", e)))
    }
}

impl AppConfig {
    pub fn from_env() -> Result<Self, AgentError> {
        let provider_config_path = std::env::var("PROVIDER_CONFIG_PATH")
            .unwrap_or_else(|_| "./provider_config.json".to_string());

        // Try to load provider settings from file first
        let mut provider = ProviderSettings::load(&provider_config_path).unwrap_or_default();

        // Environment variables override file settings
        if let Ok(provider_type) = std::env::var("LLM_PROVIDER") {
            provider.provider_type = provider_type;
        }

        if let Ok(model) = std::env::var("LLM_MODEL") {
            provider.model = model;
        }

        if let Ok(base_url) = std::env::var("LLM_BASE_URL") {
            provider.base_url = Some(base_url);
        }

        if let Ok(max_tokens) = std::env::var("LLM_MAX_TOKENS") {
            if let Ok(tokens) = max_tokens.parse::<u32>() {
                provider.max_tokens = tokens;
            }
        }

        // API key from environment (supports multiple provider key names)
        if provider.api_key.is_empty() {
            provider.api_key = match provider.provider_type.as_str() {
                "anthropic" => std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
                "openai" => std::env::var("OPENAI_API_KEY").unwrap_or_default(),
                "gemini" => std::env::var("GOOGLE_API_KEY")
                    .or_else(|_| std::env::var("GEMINI_API_KEY"))
                    .unwrap_or_default(),
                "deepseek" => std::env::var("DEEPSEEK_API_KEY").unwrap_or_default(),
                "openrouter" => std::env::var("OPENROUTER_API_KEY").unwrap_or_default(),
                "groq" => std::env::var("GROQ_API_KEY").unwrap_or_default(),
                "together" => std::env::var("TOGETHER_API_KEY").unwrap_or_default(),
                "ollama" | "lmstudio" => String::new(), // Local providers don't need API key
                _ => std::env::var("LLM_API_KEY").unwrap_or_default(),
            };
        }

        // Validate API key is set (except for local providers)
        if provider.api_key.is_empty() && !matches!(provider.provider_type.as_str(), "ollama" | "lmstudio") {
            return Err(AgentError::ConfigError(format!(
                "API key not set for provider '{}'. Set via environment variable or provider_config.json",
                provider.provider_type
            )));
        }

        Ok(Self {
            db_path: std::env::var("AGENT_DB_PATH").unwrap_or_else(|_| "./agent_brain".to_string()),
            skills_dir: std::env::var("SKILLS_DIR").unwrap_or_else(|_| "./skills".to_string()),
            mcp_config_path: std::env::var("MCP_CONFIG_PATH")
                .unwrap_or_else(|_| "./mcp_servers.json".to_string()),
            provider_config_path,
            default_user_id: std::env::var("DEFAULT_USER_ID")
                .unwrap_or_else(|_| "default_user".to_string()),
            provider,
        })
    }
}
