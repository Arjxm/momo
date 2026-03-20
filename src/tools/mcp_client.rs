//! MCP (Model Context Protocol) client implementation.
//! Connects to MCP servers and calls their tools.
//!
//! MCP spec: https://modelcontextprotocol.io/specification/2025-11-25

use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use crate::config::MCPServerConfig;
use crate::types::AgentError;

/// Connection timeout in seconds
const CONNECT_TIMEOUT_SECS: u64 = 10;
/// Tool call timeout in seconds
const CALL_TIMEOUT_SECS: u64 = 30;

/// MCP-specific errors
#[derive(Debug)]
pub enum MCPError {
    ConnectionFailed(String),
    ConnectionTimeout,
    ProtocolError(String),
    ToolCallFailed(String),
    ToolCallTimeout,
    ServerDisconnected,
    InvalidResponse(String),
}

impl std::fmt::Display for MCPError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MCPError::ConnectionFailed(msg) => write!(f, "Connection failed: {}", msg),
            MCPError::ConnectionTimeout => write!(f, "Connection timeout"),
            MCPError::ProtocolError(msg) => write!(f, "Protocol error: {}", msg),
            MCPError::ToolCallFailed(msg) => write!(f, "Tool call failed: {}", msg),
            MCPError::ToolCallTimeout => write!(f, "Tool call timeout"),
            MCPError::ServerDisconnected => write!(f, "Server disconnected"),
            MCPError::InvalidResponse(msg) => write!(f, "Invalid response: {}", msg),
        }
    }
}

impl std::error::Error for MCPError {}

impl From<MCPError> for AgentError {
    fn from(err: MCPError) -> Self {
        AgentError::ToolError(err.to_string())
    }
}

/// Information about an MCP tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// JSON-RPC request
#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// JSON-RPC response
#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: u64,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

/// MCP transport implementations
enum MCPTransport {
    Stdio {
        child: Child,
        stdin: ChildStdin,
        stdout: BufReader<ChildStdout>,
    },
    SSE {
        url: String,
        client: reqwest::Client,
    },
    HTTP {
        url: String,
        client: reqwest::Client,
    },
}

/// Client for communicating with an MCP server
pub struct MCPClient {
    transport: MCPTransport,
    server_id: String,
    server_name: String,
    request_id: u64,
    connected: bool,
}

impl MCPClient {
    /// Connect to an MCP server
    pub async fn connect(config: &MCPServerConfig) -> Result<Self, MCPError> {
        info!("Connecting to MCP server: {}", config.name);

        let transport = match config.transport.as_str() {
            "stdio" => Self::connect_stdio(config).await?,
            "sse" => Self::connect_sse(config).await?,
            "http" => Self::connect_http(config).await?,
            other => {
                return Err(MCPError::ConnectionFailed(format!(
                    "Unknown transport: {}",
                    other
                )))
            }
        };

        let mut client = MCPClient {
            transport,
            server_id: uuid::Uuid::new_v4().to_string(),
            server_name: config.name.clone(),
            request_id: 0,
            connected: false,
        };

        // Initialize the connection
        client.initialize().await?;

        info!("Connected to MCP server: {}", config.name);
        Ok(client)
    }

    async fn connect_stdio(config: &MCPServerConfig) -> Result<MCPTransport, MCPError> {
        let command = config
            .command
            .as_ref()
            .ok_or_else(|| MCPError::ConnectionFailed("No command specified".to_string()))?;

        let mut cmd = Command::new(command);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        // Set environment variables
        for (key, value) in &config.env {
            // Expand environment variables in the value
            let expanded = if value.starts_with("${") && value.ends_with('}') {
                let var_name = &value[2..value.len() - 1];
                std::env::var(var_name).unwrap_or_else(|_| value.clone())
            } else {
                value.clone()
            };
            cmd.env(key, expanded);
        }

        let connect_future = async {
            let mut child = cmd
                .spawn()
                .map_err(|e| MCPError::ConnectionFailed(format!("Failed to spawn process: {}", e)))?;

            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| MCPError::ConnectionFailed("Failed to get stdin".to_string()))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| MCPError::ConnectionFailed("Failed to get stdout".to_string()))?;

            Ok(MCPTransport::Stdio {
                child,
                stdin,
                stdout: BufReader::new(stdout),
            })
        };

        timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS), connect_future)
            .await
            .map_err(|_| MCPError::ConnectionTimeout)?
    }

    async fn connect_sse(config: &MCPServerConfig) -> Result<MCPTransport, MCPError> {
        let url = config
            .url
            .as_ref()
            .ok_or_else(|| MCPError::ConnectionFailed("No URL specified".to_string()))?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(CALL_TIMEOUT_SECS))
            .build()
            .map_err(|e| MCPError::ConnectionFailed(format!("Failed to create HTTP client: {}", e)))?;

        Ok(MCPTransport::SSE {
            url: url.clone(),
            client,
        })
    }

    async fn connect_http(config: &MCPServerConfig) -> Result<MCPTransport, MCPError> {
        let url = config
            .url
            .as_ref()
            .ok_or_else(|| MCPError::ConnectionFailed("No URL specified".to_string()))?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(CALL_TIMEOUT_SECS))
            .build()
            .map_err(|e| MCPError::ConnectionFailed(format!("Failed to create HTTP client: {}", e)))?;

        Ok(MCPTransport::HTTP {
            url: url.clone(),
            client,
        })
    }

    /// Initialize the MCP connection
    async fn initialize(&mut self) -> Result<(), MCPError> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "clientInfo": {
                "name": "agent-brain",
                "version": "0.2.0"
            }
        });

        let response = self.send_request("initialize", Some(params)).await?;

        if response.is_some() {
            self.connected = true;

            // Send initialized notification
            self.send_notification("notifications/initialized", None)
                .await?;
        }

        Ok(())
    }

    /// List available tools from the server
    pub async fn list_tools(&mut self) -> Result<Vec<MCPToolInfo>, MCPError> {
        if !self.connected {
            return Err(MCPError::ServerDisconnected);
        }

        let response = self.send_request("tools/list", None).await?;

        let tools_value = response.ok_or_else(|| {
            MCPError::InvalidResponse("No result in tools/list response".to_string())
        })?;

        let tools_array = tools_value["tools"]
            .as_array()
            .ok_or_else(|| MCPError::InvalidResponse("tools field is not an array".to_string()))?;

        let mut tools = Vec::new();
        for tool in tools_array {
            let name = tool["name"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            let description = tool["description"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let input_schema = tool["inputSchema"].clone();

            tools.push(MCPToolInfo {
                name,
                description,
                input_schema,
            });
        }

        debug!("Found {} tools on server {}", tools.len(), self.server_name);
        Ok(tools)
    }

    /// Call a tool on the server
    pub async fn call_tool(
        &mut self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<String, MCPError> {
        if !self.connected {
            return Err(MCPError::ServerDisconnected);
        }

        let params = serde_json::json!({
            "name": name,
            "arguments": args
        });

        let call_future = async {
            let response = self.send_request("tools/call", Some(params)).await?;

            let result = response.ok_or_else(|| {
                MCPError::ToolCallFailed("No result in tool call response".to_string())
            })?;

            // Extract content from the response
            if let Some(content) = result["content"].as_array() {
                let text_parts: Vec<String> = content
                    .iter()
                    .filter_map(|c| {
                        if c["type"].as_str() == Some("text") {
                            c["text"].as_str().map(|s| s.to_string())
                        } else {
                            None
                        }
                    })
                    .collect();
                Ok(text_parts.join("\n"))
            } else if let Some(text) = result.as_str() {
                Ok(text.to_string())
            } else {
                Ok(result.to_string())
            }
        };

        timeout(Duration::from_secs(CALL_TIMEOUT_SECS), call_future)
            .await
            .map_err(|_| MCPError::ToolCallTimeout)?
    }

    /// Disconnect from the server
    pub async fn disconnect(&mut self) -> Result<(), MCPError> {
        if !self.connected {
            return Ok(());
        }

        match &mut self.transport {
            MCPTransport::Stdio { child, .. } => {
                if let Err(e) = child.kill().await {
                    warn!("Failed to kill MCP server process: {}", e);
                }
            }
            MCPTransport::SSE { .. } | MCPTransport::HTTP { .. } => {
                // HTTP transports don't need explicit disconnect
            }
        }

        self.connected = false;
        info!("Disconnected from MCP server: {}", self.server_name);
        Ok(())
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Get server name
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Get server ID
    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    /// Send a JSON-RPC request
    async fn send_request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, MCPError> {
        self.request_id += 1;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: self.request_id,
            method: method.to_string(),
            params,
        };

        let request_json = serde_json::to_string(&request)
            .map_err(|e| MCPError::ProtocolError(format!("Failed to serialize request: {}", e)))?;

        debug!("MCP request: {}", request_json);

        match &mut self.transport {
            MCPTransport::Stdio { stdin, stdout, .. } => {
                // Write request
                stdin
                    .write_all(request_json.as_bytes())
                    .await
                    .map_err(|e| MCPError::ProtocolError(format!("Failed to write request: {}", e)))?;
                stdin
                    .write_all(b"\n")
                    .await
                    .map_err(|e| MCPError::ProtocolError(format!("Failed to write newline: {}", e)))?;
                stdin
                    .flush()
                    .await
                    .map_err(|e| MCPError::ProtocolError(format!("Failed to flush: {}", e)))?;

                // Read response
                let mut response_line = String::new();
                stdout
                    .read_line(&mut response_line)
                    .await
                    .map_err(|e| MCPError::ProtocolError(format!("Failed to read response: {}", e)))?;

                debug!("MCP response: {}", response_line.trim());

                let response: JsonRpcResponse = serde_json::from_str(&response_line)
                    .map_err(|e| MCPError::InvalidResponse(format!("Failed to parse response: {}", e)))?;

                if let Some(error) = response.error {
                    return Err(MCPError::ToolCallFailed(error.message));
                }

                Ok(response.result)
            }
            MCPTransport::SSE { url, client } | MCPTransport::HTTP { url, client } => {
                let resp = client
                    .post(url.as_str())
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| MCPError::ProtocolError(format!("HTTP request failed: {}", e)))?;

                let response: JsonRpcResponse = resp
                    .json()
                    .await
                    .map_err(|e| MCPError::InvalidResponse(format!("Failed to parse response: {}", e)))?;

                if let Some(error) = response.error {
                    return Err(MCPError::ToolCallFailed(error.message));
                }

                Ok(response.result)
            }
        }
    }

    /// Send a JSON-RPC notification (no response expected)
    async fn send_notification(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), MCPError> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(serde_json::json!({}))
        });

        let notification_json = serde_json::to_string(&notification)
            .map_err(|e| MCPError::ProtocolError(format!("Failed to serialize notification: {}", e)))?;

        match &mut self.transport {
            MCPTransport::Stdio { stdin, .. } => {
                stdin
                    .write_all(notification_json.as_bytes())
                    .await
                    .map_err(|e| {
                        MCPError::ProtocolError(format!("Failed to write notification: {}", e))
                    })?;
                stdin
                    .write_all(b"\n")
                    .await
                    .map_err(|e| MCPError::ProtocolError(format!("Failed to write newline: {}", e)))?;
                stdin
                    .flush()
                    .await
                    .map_err(|e| MCPError::ProtocolError(format!("Failed to flush: {}", e)))?;
            }
            MCPTransport::SSE { url, client } | MCPTransport::HTTP { url, client } => {
                client
                    .post(url.as_str())
                    .json(&notification)
                    .send()
                    .await
                    .map_err(|e| MCPError::ProtocolError(format!("HTTP request failed: {}", e)))?;
            }
        }

        Ok(())
    }
}

impl Drop for MCPClient {
    fn drop(&mut self) {
        if self.connected {
            if let MCPTransport::Stdio { child, .. } = &mut self.transport {
                // Try to kill the child process on drop
                let _ = child.start_kill();
            }
        }
    }
}
