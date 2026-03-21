pub mod arxiv;
pub mod browser;
pub mod calculator;
pub mod free_apis;
pub mod mcp_bridge;
pub mod mcp_client;
pub mod web_fetch;

use async_trait::async_trait;
use std::collections::HashMap;

use crate::types::{AgentError, ToolDefinition, ToolResult, ToolType};

/// Maximum length for tool descriptions (in characters)
const MAX_DESCRIPTION_LENGTH: usize = 300;

/// Maximum depth for schema simplification
const MAX_SCHEMA_DEPTH: usize = 2;

/// Optimize a tool definition to reduce token usage
fn optimize_tool_definition(mut def: ToolDefinition) -> ToolDefinition {
    // Truncate description
    if def.description.len() > MAX_DESCRIPTION_LENGTH {
        def.description = format!("{}...", &def.description[..MAX_DESCRIPTION_LENGTH - 3]);
    }

    // Simplify the input schema
    def.input_schema = simplify_schema(def.input_schema, 0);

    def
}

/// Simplify a JSON schema to reduce token usage
fn simplify_schema(schema: serde_json::Value, depth: usize) -> serde_json::Value {
    if depth > MAX_SCHEMA_DEPTH {
        // At max depth, return minimal schema
        return serde_json::json!({"type": "object"});
    }

    match schema {
        serde_json::Value::Object(mut obj) => {
            // Remove verbose fields that aren't essential
            obj.remove("examples");
            obj.remove("default");
            obj.remove("$schema");
            obj.remove("definitions");
            obj.remove("$defs");

            // Truncate description in schema properties
            if let Some(desc) = obj.get_mut("description") {
                if let Some(s) = desc.as_str() {
                    if s.len() > 100 {
                        *desc = serde_json::Value::String(format!("{}...", &s[..97]));
                    }
                }
            }

            // Recursively simplify properties
            if let Some(props) = obj.get_mut("properties") {
                if let Some(props_obj) = props.as_object_mut() {
                    for (_, prop_schema) in props_obj.iter_mut() {
                        *prop_schema = simplify_schema(prop_schema.take(), depth + 1);
                    }
                }
            }

            // Simplify items in arrays
            if let Some(items) = obj.get_mut("items") {
                *items = simplify_schema(items.take(), depth + 1);
            }

            // Simplify anyOf/oneOf/allOf
            for key in &["anyOf", "oneOf", "allOf"] {
                if let Some(arr) = obj.get_mut(*key) {
                    if let Some(arr_vec) = arr.as_array_mut() {
                        for item in arr_vec.iter_mut() {
                            *item = simplify_schema(item.take(), depth + 1);
                        }
                    }
                }
            }

            serde_json::Value::Object(obj)
        }
        other => other,
    }
}

pub use arxiv::ArxivSearch;
pub use browser::BrowserTool;
pub use calculator::Calculator;
pub use free_apis::{ExchangeRates, HackerNews, Weather, Wikipedia};
pub use mcp_bridge::MCPBridge;
pub use mcp_client::MCPClient;
pub use web_fetch::WebFetch;

/// Trait for implementing tools that Claude can use
#[async_trait]
pub trait Tool: Send + Sync {
    /// Returns the tool definition for the Claude API
    fn definition(&self) -> ToolDefinition;

    /// Executes the tool with the given input
    async fn execute(
        &self,
        input: HashMap<String, serde_json::Value>,
    ) -> Result<String, AgentError>;
}

/// Information about a registered tool
pub struct RegisteredTool {
    pub tool: Box<dyn Tool>,
    pub tool_type: ToolType,
}

/// Registry for managing available tools
pub struct ToolRegistry {
    tools: HashMap<String, RegisteredTool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a new tool with its type
    pub fn register_with_type<T: Tool + 'static>(&mut self, tool: T, tool_type: ToolType) {
        let def = tool.definition();
        self.tools.insert(
            def.name.clone(),
            RegisteredTool {
                tool: Box::new(tool),
                tool_type,
            },
        );
    }

    /// Register a native tool (convenience method)
    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        self.register_with_type(tool, ToolType::Native);
    }

    /// Get tool definitions for the Claude API (optimized to reduce tokens)
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|t| optimize_tool_definition(t.tool.definition()))
            .collect()
    }

    /// Get tool definitions filtered by type
    pub fn definitions_by_type(&self, tool_type: &ToolType) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .filter(|t| &t.tool_type == tool_type)
            .map(|t| t.tool.definition())
            .collect()
    }

    /// Get a list of tool names
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Get tool names by type
    pub fn tool_names_by_type(&self, tool_type: &ToolType) -> Vec<String> {
        self.tools
            .iter()
            .filter(|(_, t)| &t.tool_type == tool_type)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Count tools by type
    pub fn count_by_type(&self) -> HashMap<ToolType, usize> {
        let mut counts = HashMap::new();
        for reg_tool in self.tools.values() {
            *counts.entry(reg_tool.tool_type.clone()).or_insert(0) += 1;
        }
        counts
    }

    /// Get total number of tools
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Check if a tool exists
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Get the type of a tool
    pub fn get_tool_type(&self, name: &str) -> Option<&ToolType> {
        self.tools.get(name).map(|t| &t.tool_type)
    }

    /// Execute a tool by name
    pub async fn execute(
        &self,
        tool_use_id: &str,
        name: &str,
        input: HashMap<String, serde_json::Value>,
    ) -> ToolResult {
        match self.tools.get(name) {
            Some(reg_tool) => match reg_tool.tool.execute(input).await {
                Ok(content) => ToolResult::success(tool_use_id.to_string(), content),
                Err(e) => ToolResult::error(tool_use_id.to_string(), e.to_string()),
            },
            None => ToolResult::error(
                tool_use_id.to_string(),
                format!("Unknown tool: {}", name),
            ),
        }
    }

    /// Remove a tool by name
    pub fn remove(&mut self, name: &str) -> bool {
        self.tools.remove(name).is_some()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
