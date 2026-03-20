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

impl MCPServersConfig {
    /// Load MCP server configuration from a JSON file
    pub fn load(path: &str) -> Result<Self, AgentError> {
        let path = Path::new(path);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| AgentError::ConfigError(format!("Failed to read MCP config: {}", e)))?;

        serde_json::from_str(&content)
            .map_err(|e| AgentError::ConfigError(format!("Failed to parse MCP config: {}", e)))
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
    pub anthropic_api_key: String,
    pub default_user_id: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, AgentError> {
        let anthropic_api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| AgentError::ConfigError("ANTHROPIC_API_KEY not set".to_string()))?;

        Ok(Self {
            db_path: std::env::var("AGENT_DB_PATH").unwrap_or_else(|_| "./agent_brain".to_string()),
            skills_dir: std::env::var("SKILLS_DIR").unwrap_or_else(|_| "./skills".to_string()),
            mcp_config_path: std::env::var("MCP_CONFIG_PATH")
                .unwrap_or_else(|_| "./mcp_servers.json".to_string()),
            anthropic_api_key,
            default_user_id: std::env::var("DEFAULT_USER_ID")
                .unwrap_or_else(|_| "default_user".to_string()),
        })
    }
}
