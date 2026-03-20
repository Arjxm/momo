//! MCP Bridge - bridges MCP tools into the graph and tool registry.

use std::collections::HashMap;
use std::sync::Arc;

use tracing::{debug, error, info, warn};

/// Maximum number of tools to register per MCP server (to manage token usage)
const MAX_TOOLS_PER_SERVER: usize = 25;

use crate::config::{MCPServerConfig, MCPServersConfig};
use crate::graph::GraphBrain;
use crate::tools::mcp_client::{MCPClient, MCPError, MCPToolInfo};
use crate::types::{AgentError, MCPServerNode, MCPStatus, ToolNode, ToolType};

/// Bridge that connects MCP servers to the graph and provides tool execution
pub struct MCPBridge {
    clients: HashMap<String, MCPClient>,
    brain: Arc<GraphBrain>,
}

impl MCPBridge {
    /// Create a new MCP bridge
    pub fn new(brain: Arc<GraphBrain>) -> Self {
        Self {
            clients: HashMap::new(),
            brain,
        }
    }

    /// Connect to an MCP server and register its tools in the graph
    pub async fn connect_server(
        &mut self,
        config: &MCPServerConfig,
    ) -> Result<Vec<ToolNode>, AgentError> {
        info!("Connecting to MCP server: {}", config.name);

        // Check if already connected
        if self.clients.contains_key(&config.name) {
            warn!("Server {} already connected, refreshing...", config.name);
            self.disconnect_server(&config.name).await?;
        }

        // Connect to the server
        let mut client = MCPClient::connect(config)
            .await
            .map_err(|e| AgentError::ToolError(format!("Failed to connect to MCP server: {}", e)))?;

        // Discover tools
        let mcp_tools = client.list_tools().await.map_err(|e| {
            AgentError::ToolError(format!("Failed to list tools from MCP server: {}", e))
        })?;

        // Register server in graph
        let server_node = MCPServerNode {
            id: client.server_id().to_string(),
            name: config.name.clone(),
            url: config.url.clone(),
            transport: config.transport.clone(),
            status: MCPStatus::Connected,
            auto_connect: config.auto_connect,
            last_connected_at: Some(chrono::Utc::now()),
        };
        self.brain.register_mcp_server(&server_node)?;

        // Register each tool in the graph (limited to MAX_TOOLS_PER_SERVER)
        let mut registered_tools = Vec::new();
        let tools_to_register = if mcp_tools.len() > MAX_TOOLS_PER_SERVER {
            warn!(
                "MCP server {} has {} tools, limiting to {} to manage token usage",
                config.name,
                mcp_tools.len(),
                MAX_TOOLS_PER_SERVER
            );
            &mcp_tools[..MAX_TOOLS_PER_SERVER]
        } else {
            &mcp_tools[..]
        };

        for mcp_tool in tools_to_register {
            let tool_node = self.register_mcp_tool(&config.name, &client.server_id(), mcp_tool)?;
            registered_tools.push(tool_node);
        }

        // Store the client
        self.clients.insert(config.name.clone(), client);

        info!(
            "Connected to {} with {} tools",
            config.name,
            registered_tools.len()
        );

        Ok(registered_tools)
    }

    /// Connect to all servers from a configuration file
    pub async fn connect_all_from_config(&mut self, config_path: &str) -> Result<(), AgentError> {
        let config = MCPServersConfig::load(config_path)?;

        let auto_connect_servers: Vec<_> = config.auto_connect_servers().into_iter().cloned().collect();

        for server_config in auto_connect_servers {
            match self.connect_server(&server_config).await {
                Ok(tools) => {
                    info!(
                        "Connected to {} ({} tools)",
                        server_config.name,
                        tools.len()
                    );
                }
                Err(e) => {
                    error!("Failed to connect to {}: {}", server_config.name, e);
                    // Mark server as error in graph
                    self.brain
                        .update_mcp_status(&server_config.name, "error")
                        .ok();
                }
            }
        }

        Ok(())
    }

    /// Call a tool through the MCP bridge
    pub async fn call_tool(
        &mut self,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> Result<String, AgentError> {
        // Find which server hosts this tool
        let server_name = self.find_server_for_tool(tool_name)?;

        let client = self
            .clients
            .get_mut(&server_name)
            .ok_or_else(|| AgentError::ToolError(format!("Server {} not connected", server_name)))?;

        // Call the tool
        let result = client.call_tool(tool_name, input).await.map_err(|e| {
            // Update stats on failure
            self.brain.update_tool_stats(tool_name, false).ok();

            match e {
                MCPError::ServerDisconnected => {
                    self.brain.update_mcp_status(&server_name, "disconnected").ok();
                    AgentError::ToolError("MCP server disconnected".to_string())
                }
                _ => AgentError::ToolError(format!("MCP tool call failed: {}", e)),
            }
        })?;

        // Update stats on success
        self.brain.update_tool_stats(tool_name, true).ok();

        Ok(result)
    }

    /// Disconnect from a server
    pub async fn disconnect_server(&mut self, server_name: &str) -> Result<(), AgentError> {
        if let Some(mut client) = self.clients.remove(server_name) {
            client.disconnect().await.map_err(|e| {
                AgentError::ToolError(format!("Failed to disconnect from {}: {}", server_name, e))
            })?;
        }

        // Update status in graph
        self.brain.update_mcp_status(server_name, "disconnected")?;

        info!("Disconnected from MCP server: {}", server_name);
        Ok(())
    }

    /// Refresh tools from a server
    pub async fn refresh_server(&mut self, server_name: &str) -> Result<Vec<ToolNode>, AgentError> {
        let client = self
            .clients
            .get_mut(server_name)
            .ok_or_else(|| AgentError::ToolError(format!("Server {} not connected", server_name)))?;

        let server_id = client.server_id().to_string();

        // List tools again
        let mcp_tools = client.list_tools().await.map_err(|e| {
            AgentError::ToolError(format!("Failed to list tools from {}: {}", server_name, e))
        })?;

        // Re-register tools
        let mut registered_tools = Vec::new();
        for mcp_tool in &mcp_tools {
            let tool_node = self.register_mcp_tool(server_name, &server_id, mcp_tool)?;
            registered_tools.push(tool_node);
        }

        info!(
            "Refreshed {} tools from {}",
            registered_tools.len(),
            server_name
        );
        Ok(registered_tools)
    }

    /// Get list of connected servers
    pub fn connected_servers(&self) -> Vec<String> {
        self.clients.keys().cloned().collect()
    }

    /// Check if a server is connected
    pub fn is_connected(&self, server_name: &str) -> bool {
        self.clients
            .get(server_name)
            .map(|c| c.is_connected())
            .unwrap_or(false)
    }

    /// Register an MCP tool in the graph
    fn register_mcp_tool(
        &self,
        server_name: &str,
        server_id: &str,
        mcp_tool: &MCPToolInfo,
    ) -> Result<ToolNode, AgentError> {
        let tool_node = ToolNode::new(
            mcp_tool.name.clone(),
            mcp_tool.description.clone(),
            ToolType::Mcp,
            mcp_tool.input_schema.clone(),
            server_id.to_string(),
        );

        self.brain.register_tool(&tool_node)?;

        // Create HOSTED_ON relationship (via raw query since we need custom edge)
        // The graph mod handles this internally based on source field

        // Auto-detect topics from description
        let topics = extract_topics_from_description(&mcp_tool.description);
        for topic in &topics {
            self.brain.link_tool_topic(&mcp_tool.name, topic)?;
        }

        debug!(
            "Registered MCP tool: {} from {} with topics: {:?}",
            mcp_tool.name, server_name, topics
        );

        Ok(tool_node)
    }

    /// Find which server hosts a given tool
    fn find_server_for_tool(&self, tool_name: &str) -> Result<String, AgentError> {
        // Query the graph to find the tool's source (server_id)
        // Then look up which client has that server_id
        // For now, we'll do a simple iteration
        for (server_name, client) in &self.clients {
            // This is a simplified check - ideally we'd query the graph
            // For now, assume the tool name is unique across servers
            if client.is_connected() {
                // Try to find the tool in this server's registered tools
                let tools = self.brain.get_tools_by_type("mcp")?;
                for tool in tools {
                    if tool.name == tool_name && tool.source == client.server_id() {
                        return Ok(server_name.clone());
                    }
                }
            }
        }

        Err(AgentError::ToolError(format!(
            "No server found hosting tool: {}",
            tool_name
        )))
    }
}

/// Extract topic keywords from a tool description
fn extract_topics_from_description(description: &str) -> Vec<String> {
    let description_lower = description.to_lowercase();
    let mut topics = Vec::new();

    // Common topic keywords
    let topic_keywords = [
        ("file", "filesystem"),
        ("directory", "filesystem"),
        ("folder", "filesystem"),
        ("read", "io"),
        ("write", "io"),
        ("search", "search"),
        ("query", "search"),
        ("git", "vcs"),
        ("github", "vcs"),
        ("database", "database"),
        ("sql", "database"),
        ("http", "web"),
        ("api", "web"),
        ("fetch", "web"),
        ("email", "communication"),
        ("slack", "communication"),
        ("calendar", "productivity"),
        ("todo", "productivity"),
        ("task", "productivity"),
        ("weather", "weather"),
        ("map", "geography"),
        ("code", "development"),
        ("compile", "development"),
        ("test", "testing"),
    ];

    for (keyword, topic) in topic_keywords {
        if description_lower.contains(keyword) && !topics.contains(&topic.to_string()) {
            topics.push(topic.to_string());
        }
    }

    // Limit to 3 topics
    topics.truncate(3);
    topics
}

/// MCP management tools for the agent to use
pub mod management_tools {
    use super::*;
    use async_trait::async_trait;
    use crate::tools::Tool;
    use crate::types::ToolDefinition;
    use tokio::sync::Mutex;

    /// Tool to list MCP servers
    pub struct MCPListServersTool {
        bridge: Arc<Mutex<MCPBridge>>,
    }

    impl MCPListServersTool {
        pub fn new(bridge: Arc<Mutex<MCPBridge>>) -> Self {
            Self { bridge }
        }
    }

    #[async_trait]
    impl Tool for MCPListServersTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "mcp_list_servers".to_string(),
                description: "List all MCP servers and their connection status".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            }
        }

        async fn execute(
            &self,
            _input: HashMap<String, serde_json::Value>,
        ) -> Result<String, AgentError> {
            let bridge = self.bridge.lock().await;
            let servers = bridge.connected_servers();

            if servers.is_empty() {
                return Ok("No MCP servers connected.".to_string());
            }

            let mut output = format!("Connected MCP servers ({}):\n", servers.len());
            for server in servers {
                let status = if bridge.is_connected(&server) {
                    "connected"
                } else {
                    "disconnected"
                };
                output.push_str(&format!("  - {} ({})\n", server, status));
            }

            Ok(output)
        }
    }

    /// Tool to connect to an MCP server
    pub struct MCPConnectTool {
        bridge: Arc<Mutex<MCPBridge>>,
        config_path: String,
    }

    impl MCPConnectTool {
        pub fn new(bridge: Arc<Mutex<MCPBridge>>, config_path: String) -> Self {
            Self { bridge, config_path }
        }
    }

    #[async_trait]
    impl Tool for MCPConnectTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "mcp_connect".to_string(),
                description: "Connect to an MCP server by name (must be configured in mcp_servers.json)".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "server_name": {
                            "type": "string",
                            "description": "Name of the MCP server to connect to"
                        }
                    },
                    "required": ["server_name"]
                }),
            }
        }

        async fn execute(
            &self,
            input: HashMap<String, serde_json::Value>,
        ) -> Result<String, AgentError> {
            let server_name = input
                .get("server_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::ToolError("Missing server_name".to_string()))?;

            let config = MCPServersConfig::load(&self.config_path)?;
            let server_config = config
                .get_server(server_name)
                .ok_or_else(|| {
                    AgentError::ToolError(format!(
                        "Server '{}' not found in configuration",
                        server_name
                    ))
                })?
                .clone();

            let mut bridge = self.bridge.lock().await;
            let tools = bridge.connect_server(&server_config).await?;

            Ok(format!(
                "Connected to '{}'. Found {} tools.",
                server_name,
                tools.len()
            ))
        }
    }

    /// Tool to disconnect from an MCP server
    pub struct MCPDisconnectTool {
        bridge: Arc<Mutex<MCPBridge>>,
    }

    impl MCPDisconnectTool {
        pub fn new(bridge: Arc<Mutex<MCPBridge>>) -> Self {
            Self { bridge }
        }
    }

    #[async_trait]
    impl Tool for MCPDisconnectTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "mcp_disconnect".to_string(),
                description: "Disconnect from an MCP server".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "server_name": {
                            "type": "string",
                            "description": "Name of the MCP server to disconnect from"
                        }
                    },
                    "required": ["server_name"]
                }),
            }
        }

        async fn execute(
            &self,
            input: HashMap<String, serde_json::Value>,
        ) -> Result<String, AgentError> {
            let server_name = input
                .get("server_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::ToolError("Missing server_name".to_string()))?;

            let mut bridge = self.bridge.lock().await;
            bridge.disconnect_server(server_name).await?;

            Ok(format!("Disconnected from '{}'.", server_name))
        }
    }

    /// Tool to refresh tools from an MCP server
    pub struct MCPRefreshTool {
        bridge: Arc<Mutex<MCPBridge>>,
    }

    impl MCPRefreshTool {
        pub fn new(bridge: Arc<Mutex<MCPBridge>>) -> Self {
            Self { bridge }
        }
    }

    #[async_trait]
    impl Tool for MCPRefreshTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "mcp_refresh".to_string(),
                description: "Refresh the list of tools from an MCP server".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "server_name": {
                            "type": "string",
                            "description": "Name of the MCP server to refresh"
                        }
                    },
                    "required": ["server_name"]
                }),
            }
        }

        async fn execute(
            &self,
            input: HashMap<String, serde_json::Value>,
        ) -> Result<String, AgentError> {
            let server_name = input
                .get("server_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::ToolError("Missing server_name".to_string()))?;

            let mut bridge = self.bridge.lock().await;
            let tools = bridge.refresh_server(server_name).await?;

            Ok(format!(
                "Refreshed '{}'. Found {} tools.",
                server_name,
                tools.len()
            ))
        }
    }
}
