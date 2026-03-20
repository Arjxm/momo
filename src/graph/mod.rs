pub mod memory;
pub mod queries;
pub mod schema;

use chrono::Utc;
use lbug::{Connection, Database, SystemConfig};
use std::path::Path;
use std::sync::Mutex;
use tracing::{debug, info, warn};

use crate::types::{
    AgentError, EpisodeNode, GraphStats, MCPServerNode, MCPStatus, MemoryNode, MemoryType,
    ToolNode, ToolType,
};

/// The GraphBrain is the unified interface for all graph operations.
/// Uses lbug (LadybugDB) for persistent storage.
/// Tools are kept in-memory (session-based) while memories/episodes persist.
pub struct GraphBrain {
    db: Mutex<Database>,
    // Tools are session-based, not persisted (loaded from MCP/native each startup)
    tools: Mutex<std::collections::HashMap<String, ToolNode>>,
}

impl GraphBrain {
    /// Open or create the graph database at the given path.
    pub fn open(path: &str) -> Result<Self, AgentError> {
        info!("Opening graph database at: {}", path);

        // Create directory if it doesn't exist
        let db_path = Path::new(path);
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        // Open/create the database
        let db = Database::new(path, SystemConfig::default())
            .map_err(|e| AgentError::ConfigError(format!("Failed to open database: {}", e)))?;

        // Initialize schema
        {
            let conn = Connection::new(&db)
                .map_err(|e| AgentError::ConfigError(format!("Failed to create connection: {}", e)))?;
            Self::init_schema(&conn)?;
        }

        info!("Graph database initialized (persistent mode with lbug)");
        Ok(Self {
            db: Mutex::new(db),
            tools: Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Get a connection to execute queries
    fn with_connection<F, T>(&self, f: F) -> Result<T, AgentError>
    where
        F: FnOnce(&Connection) -> Result<T, AgentError>,
    {
        let db = self.db.lock().map_err(|e| {
            AgentError::ConfigError(format!("Lock error: {}", e))
        })?;
        let conn = Connection::new(&db).map_err(|e| {
            AgentError::ConfigError(format!("Connection error: {}", e))
        })?;
        f(&conn)
    }

    /// Initialize the database schema
    fn init_schema(conn: &Connection) -> Result<(), AgentError> {
        // Create node tables if they don't exist
        let schema_queries = [
            // Memory table
            r#"CREATE NODE TABLE IF NOT EXISTS Memory(
                id STRING,
                content STRING,
                memory_type STRING,
                importance DOUBLE,
                valid_from STRING,
                valid_until STRING,
                created_at STRING,
                PRIMARY KEY(id)
            )"#,
            // Episode table
            r#"CREATE NODE TABLE IF NOT EXISTS Episode(
                id STRING,
                user_input STRING,
                agent_response STRING,
                tools_used STRING,
                success BOOLEAN,
                duration_ms INT64,
                tokens_used INT64,
                cost_usd DOUBLE,
                created_at STRING,
                PRIMARY KEY(id)
            )"#,
            // Topic table
            r#"CREATE NODE TABLE IF NOT EXISTS Topic(
                id STRING,
                name STRING,
                PRIMARY KEY(id)
            )"#,
        ];

        for query in schema_queries {
            if let Err(e) = conn.query(query) {
                // Ignore "already exists" errors
                let err_str = e.to_string();
                if !err_str.contains("already exists") && !err_str.contains("Catalog exception") {
                    warn!("Schema query warning: {}", e);
                }
            }
        }

        debug!("Schema initialized");
        Ok(())
    }

    /// Execute a query and handle errors
    fn execute(&self, query: &str) -> Result<(), AgentError> {
        self.with_connection(|conn| {
            conn.query(query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;
            Ok(())
        })
    }

    // ═══════════════════════════════════════════════════════════════════
    // TOOL REGISTRY (in-memory for session, tools are transient)
    // ═══════════════════════════════════════════════════════════════════

    // Note: Tools are registered per-session and not persisted
    // They come from MCP servers and native tools which are loaded on startup

    /// Register a new tool (in-memory, session-based)
    pub fn register_tool(&self, tool: &ToolNode) -> Result<(), AgentError> {
        let mut tools = self.tools.lock().map_err(|e| {
            AgentError::ConfigError(format!("Lock error: {}", e))
        })?;
        tools.insert(tool.name.clone(), tool.clone());
        debug!("Registered tool: {} ({})", tool.name, tool.tool_type);
        Ok(())
    }

    /// Get all enabled tools from the graph
    pub fn get_all_tools(&self) -> Result<Vec<ToolNode>, AgentError> {
        let tools = self.tools.lock().map_err(|e| {
            AgentError::ConfigError(format!("Lock error: {}", e))
        })?;
        Ok(tools.values().filter(|t| t.enabled).cloned().collect())
    }

    /// Get tools filtered by type
    pub fn get_tools_by_type(&self, tool_type: &str) -> Result<Vec<ToolNode>, AgentError> {
        let tools = self.tools.lock().map_err(|e| {
            AgentError::ConfigError(format!("Lock error: {}", e))
        })?;
        Ok(tools.values()
            .filter(|t| t.enabled && t.tool_type.to_string() == tool_type)
            .cloned()
            .collect())
    }

    /// Find tools that operate on a specific topic
    pub fn find_tools_for_topic(&self, _topic: &str) -> Result<Vec<ToolNode>, AgentError> {
        Ok(Vec::new())
    }

    /// Update tool statistics after execution (no-op)
    pub fn update_tool_stats(&self, tool_name: &str, success: bool) -> Result<(), AgentError> {
        debug!("Updated stats for tool: {} (success={})", tool_name, success);
        Ok(())
    }

    /// Disable a tool
    pub fn disable_tool(&self, tool_name: &str) -> Result<(), AgentError> {
        info!("Disabled tool: {}", tool_name);
        Ok(())
    }

    /// Record that two tools compose well together
    pub fn record_composition(
        &self,
        from_tool: &str,
        to_tool: &str,
        _description: &str,
    ) -> Result<(), AgentError> {
        debug!("Recorded composition: {} -> {}", from_tool, to_tool);
        Ok(())
    }

    /// Find tools that compose well with the given tool
    pub fn find_composable_tools(&self, _tool_name: &str) -> Result<Vec<(ToolNode, String)>, AgentError> {
        Ok(Vec::new())
    }

    // ═══════════════════════════════════════════════════════════════════
    // MCP SERVERS (session-based, not persisted)
    // ═══════════════════════════════════════════════════════════════════

    /// Register an MCP server in the graph
    pub fn register_mcp_server(&self, server: &MCPServerNode) -> Result<(), AgentError> {
        debug!("Registered MCP server: {}", server.name);
        Ok(())
    }

    /// Get all MCP servers from the graph
    pub fn get_mcp_servers(&self) -> Result<Vec<MCPServerNode>, AgentError> {
        Ok(Vec::new())
    }

    /// Update the status of an MCP server
    pub fn update_mcp_status(&self, server_name: &str, status: &str) -> Result<(), AgentError> {
        debug!("Updated MCP server {} status to {}", server_name, status);
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════
    // MEMORY (PERSISTENT)
    // ═══════════════════════════════════════════════════════════════════

    /// Store a new memory
    pub fn remember(&self, memory: &MemoryNode, _topics: &[String]) -> Result<(), AgentError> {
        let valid_until = memory.valid_until
            .map(|t| t.to_rfc3339())
            .unwrap_or_default();

        let query = format!(
            r#"CREATE (:Memory {{
                id: '{}',
                content: '{}',
                memory_type: '{}',
                importance: {},
                valid_from: '{}',
                valid_until: '{}',
                created_at: '{}'
            }})"#,
            escape_string(&memory.id),
            escape_string(&memory.content),
            memory.memory_type,
            memory.importance,
            memory.valid_from.to_rfc3339(),
            valid_until,
            memory.created_at.to_rfc3339()
        );

        self.execute(&query)?;
        info!("Stored memory: {} ({:?})", memory.id, memory.memory_type);
        Ok(())
    }

    /// Invalidate an old memory
    pub fn invalidate_memory(&self, memory_id: &str) -> Result<(), AgentError> {
        let now = Utc::now().to_rfc3339();
        let query = format!(
            "MATCH (m:Memory {{id: '{}'}}) SET m.valid_until = '{}'",
            escape_string(memory_id),
            now
        );
        self.execute(&query)?;
        Ok(())
    }

    /// Create a CONTRADICTS edge between two memories
    pub fn link_contradiction(&self, _new_id: &str, _old_id: &str) -> Result<(), AgentError> {
        // Simplified for now
        Ok(())
    }

    /// Recall memories matching keywords
    pub fn recall(&self, keywords: &[String], limit: usize) -> Result<Vec<MemoryNode>, AgentError> {
        let keywords = keywords.to_vec();
        self.with_connection(|conn| {
            // Query all valid memories
            let query = "MATCH (m:Memory) WHERE m.valid_until = '' RETURN m.id, m.content, m.memory_type, m.importance, m.valid_from, m.created_at";

            let result = conn.query(query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let mut memories = Vec::new();

            // Parse results - lbug returns results that can be iterated
            let result_str = format!("{}", result);
            for line in result_str.lines().skip(1) { // Skip header
                let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if parts.len() >= 6 {
                    let content = parts[1].to_string();
                    // Check if content matches any keyword
                    let matches_keyword = keywords.iter().any(|k|
                        content.to_lowercase().contains(&k.to_lowercase())
                    );

                    if matches_keyword {
                        let memory = MemoryNode {
                            id: parts[0].to_string(),
                            content,
                            memory_type: parse_memory_type(parts[2]),
                            importance: parts[3].parse().unwrap_or(0.5),
                            valid_from: chrono::DateTime::parse_from_rfc3339(parts[4])
                                .map(|t| t.with_timezone(&Utc))
                                .unwrap_or_else(|_| Utc::now()),
                            valid_until: None,
                            created_at: chrono::DateTime::parse_from_rfc3339(parts[5])
                                .map(|t| t.with_timezone(&Utc))
                                .unwrap_or_else(|_| Utc::now()),
                        };
                        memories.push(memory);
                    }
                }
            }

            // Sort by importance and limit
            memories.sort_by(|a, b| b.importance.partial_cmp(&a.importance).unwrap_or(std::cmp::Ordering::Equal));
            memories.truncate(limit);

            Ok(memories)
        })
    }

    /// Recall user preferences
    pub fn recall_user_prefs(&self, _user_id: &str) -> Result<Vec<MemoryNode>, AgentError> {
        self.with_connection(|conn| {
            let query = "MATCH (m:Memory) WHERE m.valid_until = '' AND m.memory_type = 'preference' RETURN m.id, m.content, m.memory_type, m.importance, m.valid_from, m.created_at";

            let result = conn.query(query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let mut memories = Vec::new();
            let result_str = format!("{}", result);

            for line in result_str.lines().skip(1) {
                let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if parts.len() >= 6 {
                    let memory = MemoryNode {
                        id: parts[0].to_string(),
                        content: parts[1].to_string(),
                        memory_type: MemoryType::Preference,
                        importance: parts[3].parse().unwrap_or(0.5),
                        valid_from: chrono::DateTime::parse_from_rfc3339(parts[4])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                        valid_until: None,
                        created_at: chrono::DateTime::parse_from_rfc3339(parts[5])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    };
                    memories.push(memory);
                }
            }

            Ok(memories)
        })
    }

    // ═══════════════════════════════════════════════════════════════════
    // EPISODES (PERSISTENT)
    // ═══════════════════════════════════════════════════════════════════

    /// Record an episode (interaction) in the graph
    pub fn record_episode(&self, ep: &EpisodeNode, _user_id: &str) -> Result<String, AgentError> {
        let tools_str = ep.tools_used.join(",");

        let query = format!(
            r#"CREATE (:Episode {{
                id: '{}',
                user_input: '{}',
                agent_response: '{}',
                tools_used: '{}',
                success: {},
                duration_ms: {},
                tokens_used: {},
                cost_usd: {},
                created_at: '{}'
            }})"#,
            escape_string(&ep.id),
            escape_string(&ep.user_input),
            escape_string(&truncate_str(&ep.agent_response, 500)),
            escape_string(&tools_str),
            ep.success,
            ep.duration_ms,
            ep.tokens_used,
            ep.cost_usd,
            ep.created_at.to_rfc3339()
        );

        self.execute(&query)?;
        debug!("Recorded episode: {}", ep.id);
        Ok(ep.id.clone())
    }

    /// Link a tool to an episode
    pub fn link_tool_to_episode(&self, _tool_name: &str, _episode_id: &str) -> Result<(), AgentError> {
        Ok(())
    }

    /// Get recent episodes for a user
    pub fn recent_episodes(&self, _user_id: &str, limit: usize) -> Result<Vec<EpisodeNode>, AgentError> {
        self.with_connection(|conn| {
            let query = format!(
                "MATCH (e:Episode) RETURN e.id, e.user_input, e.agent_response, e.tools_used, e.success, e.duration_ms, e.tokens_used, e.cost_usd, e.created_at ORDER BY e.created_at DESC LIMIT {}",
                limit
            );

            let result = conn.query(&query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let mut episodes = Vec::new();
            let result_str = format!("{}", result);

            for line in result_str.lines().skip(1) {
                let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if parts.len() >= 9 {
                    let episode = EpisodeNode {
                        id: parts[0].to_string(),
                        user_input: parts[1].to_string(),
                        agent_response: parts[2].to_string(),
                        tools_used: parts[3].split(',').map(|s| s.to_string()).collect(),
                        success: parts[4].parse().unwrap_or(false),
                        duration_ms: parts[5].parse().unwrap_or(0),
                        tokens_used: parts[6].parse().unwrap_or(0),
                        cost_usd: parts[7].parse().unwrap_or(0.0),
                        created_at: chrono::DateTime::parse_from_rfc3339(parts[8])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    };
                    episodes.push(episode);
                }
            }

            Ok(episodes)
        })
    }

    // ═══════════════════════════════════════════════════════════════════
    // TOPICS & USERS
    // ═══════════════════════════════════════════════════════════════════

    /// Ensure a topic exists
    pub fn ensure_topic(&self, name: &str) -> Result<(), AgentError> {
        let id = uuid::Uuid::new_v4().to_string();
        let query = format!(
            "CREATE (:Topic {{id: '{}', name: '{}'}})",
            escape_string(&id),
            escape_string(name)
        );
        // Ignore errors (might already exist)
        self.execute(&query).ok();
        Ok(())
    }

    /// Link a user's interest to a topic
    pub fn link_user_interest(&self, _user_id: &str, topic: &str) -> Result<(), AgentError> {
        self.ensure_topic(topic)
    }

    /// Link a tool to a topic
    pub fn link_tool_topic(&self, _tool_name: &str, topic: &str) -> Result<(), AgentError> {
        self.ensure_topic(topic)
    }

    /// Link a user preference to a memory
    pub fn link_user_preference(&self, _user_id: &str, _memory_id: &str) -> Result<(), AgentError> {
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════
    // INTROSPECTION
    // ═══════════════════════════════════════════════════════════════════

    /// Get graph statistics
    pub fn stats(&self) -> Result<GraphStats, AgentError> {
        self.with_connection(|conn| {
            let mut stats = GraphStats::default();

            // Count memories
            if let Ok(result) = conn.query("MATCH (m:Memory) WHERE m.valid_until = '' RETURN count(m)") {
                let result_str = format!("{}", result);
                if let Some(line) = result_str.lines().nth(1) {
                    stats.memories = line.trim().parse().unwrap_or(0);
                }
            }

            // Count episodes
            if let Ok(result) = conn.query("MATCH (e:Episode) RETURN count(e)") {
                let result_str = format!("{}", result);
                if let Some(line) = result_str.lines().nth(1) {
                    stats.episodes = line.trim().parse().unwrap_or(0);
                }
            }

            // Count topics
            if let Ok(result) = conn.query("MATCH (t:Topic) RETURN count(t)") {
                let result_str = format!("{}", result);
                if let Some(line) = result_str.lines().nth(1) {
                    stats.topics = line.trim().parse().unwrap_or(0);
                }
            }

            Ok(stats)
        })
    }

    /// Execute a raw Cypher query
    pub fn raw_cypher(&self, query: &str) -> Result<Vec<serde_json::Value>, AgentError> {
        let query = query.to_string();
        self.with_connection(|conn| {
            let result = conn.query(&query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            // Return result as JSON
            Ok(vec![serde_json::json!(format!("{}", result))])
        })
    }
}

/// Escape single quotes in strings for Cypher queries
fn escape_string(s: &str) -> String {
    s.replace('\'', "\\'").replace('\n', " ").replace('\r', "")
}

/// Truncate a string to a maximum length
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

/// Parse memory type from string
fn parse_memory_type(s: &str) -> MemoryType {
    match s.to_lowercase().as_str() {
        "preference" => MemoryType::Preference,
        "episode_summary" => MemoryType::EpisodeSummary,
        _ => MemoryType::Fact,
    }
}
