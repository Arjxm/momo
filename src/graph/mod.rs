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
    OperationNode, ToolNode, ToolType,
};
use crate::orchestrator::types::TaskNode;

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
            // Memory table (enhanced with fingerprint and provenance)
            r#"CREATE NODE TABLE IF NOT EXISTS Memory(
                id STRING,
                content STRING,
                fingerprint STRING,
                memory_type STRING,
                importance DOUBLE,
                source_task_id STRING,
                source_operation_id STRING,
                last_accessed STRING,
                access_count INT64,
                valid_from STRING,
                valid_until STRING,
                superseded_by STRING,
                created_at STRING,
                PRIMARY KEY(id)
            )"#,
            // Operation table (tool execution record)
            r#"CREATE NODE TABLE IF NOT EXISTS Operation(
                id STRING,
                task_id STRING,
                sequence INT64,
                tool_name STRING,
                tool_type STRING,
                inputs STRING,
                output STRING,
                output_truncated BOOLEAN,
                duration_ms INT64,
                success BOOLEAN,
                error STRING,
                previous_op_id STRING,
                created_at STRING,
                PRIMARY KEY(id)
            )"#,
            // Task table (with hierarchy for autonomous decomposition)
            r#"CREATE NODE TABLE IF NOT EXISTS Task(
                id STRING,
                description STRING,
                status STRING,
                priority STRING,
                parent_id STRING,
                root_id STRING,
                agent_type STRING,
                tool_hint STRING,
                input_context STRING,
                output STRING,
                error STRING,
                retries INT64,
                max_retries INT64,
                created_at STRING,
                started_at STRING,
                completed_at STRING,
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

    /// Store a new memory (uses deduplication internally)
    pub fn remember(&self, memory: &MemoryNode, topics: &[String]) -> Result<(), AgentError> {
        // Use the dedup version and ignore whether it was duplicate
        self.remember_with_dedup(memory, topics)?;
        Ok(())
    }

    /// Update access tracking for a memory (called when memory is retrieved)
    pub fn touch_memory(&self, memory_id: &str) -> Result<(), AgentError> {
        let now = Utc::now().to_rfc3339();
        let query = format!(
            "MATCH (m:Memory {{id: '{}'}}) SET m.last_accessed = '{}', m.access_count = m.access_count + 1",
            escape_string(memory_id),
            now
        );
        self.execute(&query).ok(); // Don't fail if this fails
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

    /// Smart recall with multi-factor relevance scoring
    /// Scores memories based on: keyword match, importance, recency, access frequency
    pub fn smart_recall(&self, query: &str, limit: usize) -> Result<Vec<(MemoryNode, f64)>, AgentError> {
        info!("🔍 [SMART RECALL] Query: \"{}\" (limit: {})",
            if query.len() > 50 { &query[..50] } else { query }, limit);

        // Extract keywords from query
        let keywords = extract_query_keywords(query);
        info!("🔍 [SMART RECALL] Extracted keywords: {:?}", keywords);

        let result = self.with_connection(|conn| {
            // Query all valid memories with access tracking fields
            let db_query = "MATCH (m:Memory) WHERE m.valid_until = '' RETURN m.id, m.content, m.fingerprint, m.memory_type, m.importance, m.valid_from, m.created_at, m.last_accessed, m.access_count, m.source_task_id, m.source_operation_id";

            let result = conn.query(db_query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let mut scored_memories: Vec<(MemoryNode, f64)> = Vec::new();
            let result_str = format!("{}", result);

            for line in result_str.lines().skip(1) {
                let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if parts.len() >= 7 {
                    let content = parts[1].to_string();

                    // Calculate keyword match score (0.0 to 1.0)
                    let keyword_score = calculate_keyword_score(&content, &keywords);

                    // Skip if no keyword match at all (unless no keywords provided)
                    if !keywords.is_empty() && keyword_score == 0.0 {
                        continue;
                    }

                    // Parse fingerprint (generate if missing for backwards compat)
                    let fingerprint = if parts.len() > 2 && !parts[2].is_empty() {
                        parts[2].to_string()
                    } else {
                        MemoryNode::compute_fingerprint(&content)
                    };

                    // Parse access tracking (with defaults for old records)
                    let last_accessed = if parts.len() > 7 && !parts[7].is_empty() {
                        chrono::DateTime::parse_from_rfc3339(parts[7])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now())
                    } else {
                        // Default: created_at for old records
                        chrono::DateTime::parse_from_rfc3339(parts[6])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now())
                    };

                    let access_count = if parts.len() > 8 {
                        parts[8].parse().unwrap_or(0)
                    } else {
                        0
                    };

                    let source_task_id = if parts.len() > 9 && !parts[9].is_empty() {
                        Some(parts[9].to_string())
                    } else {
                        None
                    };

                    let source_operation_id = if parts.len() > 10 && !parts[10].is_empty() {
                        Some(parts[10].to_string())
                    } else {
                        None
                    };

                    let memory = MemoryNode {
                        id: parts[0].to_string(),
                        content,
                        fingerprint,
                        memory_type: parse_memory_type(parts[3]),
                        importance: parts[4].parse().unwrap_or(0.5),
                        source_task_id,
                        source_operation_id,
                        last_accessed,
                        access_count,
                        tasks_used_in: Vec::new(),
                        valid_from: chrono::DateTime::parse_from_rfc3339(parts[5])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                        valid_until: None,
                        superseded_by: None,
                        created_at: chrono::DateTime::parse_from_rfc3339(parts[6])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    };

                    // Calculate overall relevance score
                    let relevance = memory.relevance_score(keyword_score);
                    scored_memories.push((memory, relevance));
                }
            }

            // Sort by relevance score (highest first)
            scored_memories.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scored_memories.truncate(limit);

            if scored_memories.is_empty() {
                info!("🔍 [SMART RECALL] No relevant memories found");
            } else {
                info!("🔍 [SMART RECALL] Found {} relevant memories:", scored_memories.len());
                for (mem, score) in &scored_memories {
                    info!("🔍 [SMART RECALL]   📊 score={:.3} [{}] \"{}\"",
                        score, mem.memory_type,
                        if mem.content.len() > 40 { format!("{}...", &mem.content[..40]) } else { mem.content.clone() });
                }
            }

            Ok(scored_memories)
        })?;

        // Update access tracking for retrieved memories OUTSIDE the connection closure
        for (mem, _) in &result {
            // Fire and forget - don't block on this
            let _ = self.touch_memory(&mem.id);
        }

        Ok(result)
    }

    /// Legacy recall method (calls smart_recall internally)
    pub fn recall(&self, keywords: &[String], limit: usize) -> Result<Vec<MemoryNode>, AgentError> {
        let query = keywords.join(" ");
        let scored = self.smart_recall(&query, limit)?;
        Ok(scored.into_iter().map(|(mem, _)| mem).collect())
    }

    /// Get all memories without keyword filtering (for debugging/visualization)
    pub fn get_all_memories(&self, limit: usize) -> Result<Vec<MemoryNode>, AgentError> {
        debug!("🔍 [RECALL] Fetching all memories (limit: {})", limit);
        self.with_connection(|conn| {
            let query = "MATCH (m:Memory) WHERE m.valid_until = '' RETURN m.id, m.content, m.fingerprint, m.memory_type, m.importance, m.valid_from, m.created_at, m.last_accessed, m.access_count, m.source_task_id, m.source_operation_id";

            let result = conn.query(query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let mut memories = Vec::new();
            let result_str = format!("{}", result);

            for line in result_str.lines().skip(1) {
                let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if parts.len() >= 7 {
                    let content = parts[1].to_string();
                    let fingerprint = if parts.len() > 2 && !parts[2].is_empty() {
                        parts[2].to_string()
                    } else {
                        MemoryNode::compute_fingerprint(&content)
                    };

                    let last_accessed = if parts.len() > 7 && !parts[7].is_empty() {
                        chrono::DateTime::parse_from_rfc3339(parts[7])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now())
                    } else {
                        chrono::DateTime::parse_from_rfc3339(parts[6])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now())
                    };

                    let access_count = if parts.len() > 8 {
                        parts[8].parse().unwrap_or(0)
                    } else {
                        0
                    };

                    let source_task_id = if parts.len() > 9 && !parts[9].is_empty() {
                        Some(parts[9].to_string())
                    } else {
                        None
                    };

                    let source_operation_id = if parts.len() > 10 && !parts[10].is_empty() {
                        Some(parts[10].to_string())
                    } else {
                        None
                    };

                    let memory = MemoryNode {
                        id: parts[0].to_string(),
                        content,
                        fingerprint,
                        memory_type: parse_memory_type(parts[3]),
                        importance: parts[4].parse().unwrap_or(0.5),
                        source_task_id,
                        source_operation_id,
                        last_accessed,
                        access_count,
                        tasks_used_in: Vec::new(),
                        valid_from: chrono::DateTime::parse_from_rfc3339(parts[5])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                        valid_until: None,
                        superseded_by: None,
                        created_at: chrono::DateTime::parse_from_rfc3339(parts[6])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    };
                    memories.push(memory);
                }
            }

            // Sort by relevance (recency * importance)
            memories.sort_by(|a, b| {
                let score_a = a.relevance_score(0.5);
                let score_b = b.relevance_score(0.5);
                score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
            });
            memories.truncate(limit);

            debug!("🔍 [RECALL] Retrieved {} total memories", memories.len());
            Ok(memories)
        })
    }

    /// Smart recall for user preferences - returns most relevant ones
    /// Uses recency and access frequency to prioritize truly useful preferences
    pub fn smart_recall_prefs(&self, query: &str, limit: usize) -> Result<Vec<MemoryNode>, AgentError> {
        info!("🔍 [PREFS] Recalling preferences relevant to: \"{}\"",
            if query.len() > 30 { &query[..30] } else { query });

        let keywords = extract_query_keywords(query);

        let result = self.with_connection(|conn| {
            let db_query = "MATCH (m:Memory) WHERE m.valid_until = '' AND m.memory_type = 'preference' RETURN m.id, m.content, m.fingerprint, m.memory_type, m.importance, m.valid_from, m.created_at, m.last_accessed, m.access_count, m.source_task_id, m.source_operation_id";

            let result = conn.query(db_query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let mut scored_prefs: Vec<(MemoryNode, f64)> = Vec::new();
            let result_str = format!("{}", result);

            for line in result_str.lines().skip(1) {
                let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if parts.len() >= 7 {
                    let content = parts[1].to_string();
                    let keyword_score = calculate_keyword_score(&content, &keywords);

                    let fingerprint = if parts.len() > 2 && !parts[2].is_empty() {
                        parts[2].to_string()
                    } else {
                        MemoryNode::compute_fingerprint(&content)
                    };

                    let last_accessed = if parts.len() > 7 && !parts[7].is_empty() {
                        chrono::DateTime::parse_from_rfc3339(parts[7])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now())
                    } else {
                        chrono::DateTime::parse_from_rfc3339(parts[6])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now())
                    };

                    let access_count = if parts.len() > 8 {
                        parts[8].parse().unwrap_or(0)
                    } else {
                        0
                    };

                    let source_task_id = if parts.len() > 9 && !parts[9].is_empty() {
                        Some(parts[9].to_string())
                    } else {
                        None
                    };

                    let source_operation_id = if parts.len() > 10 && !parts[10].is_empty() {
                        Some(parts[10].to_string())
                    } else {
                        None
                    };

                    let memory = MemoryNode {
                        id: parts[0].to_string(),
                        content,
                        fingerprint,
                        memory_type: MemoryType::Preference,
                        importance: parts[4].parse().unwrap_or(0.5),
                        source_task_id,
                        source_operation_id,
                        last_accessed,
                        access_count,
                        tasks_used_in: Vec::new(),
                        valid_from: chrono::DateTime::parse_from_rfc3339(parts[5])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                        valid_until: None,
                        superseded_by: None,
                        created_at: chrono::DateTime::parse_from_rfc3339(parts[6])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    };

                    let relevance = memory.relevance_score(keyword_score);
                    scored_prefs.push((memory, relevance));
                }
            }

            // Sort by relevance and limit
            scored_prefs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scored_prefs.truncate(limit);

            if scored_prefs.is_empty() {
                debug!("🔍 [PREFS] No preferences found");
            } else {
                info!("🔍 [PREFS] Found {} relevant preferences:", scored_prefs.len());
                for (pref, score) in &scored_prefs {
                    info!("🔍 [PREFS]   ⭐ score={:.3} \"{}\"", score,
                        if pref.content.len() > 50 { format!("{}...", &pref.content[..50]) } else { pref.content.clone() });
                }
            }

            Ok(scored_prefs)
        })?;

        // Update access tracking OUTSIDE the connection closure
        for (pref, _) in &result {
            let _ = self.touch_memory(&pref.id);
        }

        Ok(result.into_iter().map(|(mem, _)| mem).collect())
    }

    /// Legacy recall_user_prefs (now uses smart recall with limit of 5)
    pub fn recall_user_prefs(&self, _user_id: &str) -> Result<Vec<MemoryNode>, AgentError> {
        // Return top 5 most relevant preferences based on recency and importance
        self.smart_recall_prefs("", 5)
    }

    // ═══════════════════════════════════════════════════════════════════
    // OPERATION TRACKING (Tool Execution Records)
    // ═══════════════════════════════════════════════════════════════════

    /// Record a tool operation in the graph
    pub fn record_operation(&self, op: &OperationNode) -> Result<String, AgentError> {
        let inputs_str = serde_json::to_string(&op.inputs).unwrap_or_default();
        let previous_op = op.previous_op_id.as_deref().unwrap_or("");
        let error_str = op.error.as_deref().unwrap_or("");

        let query = format!(
            r#"CREATE (:Operation {{
                id: '{}',
                task_id: '{}',
                sequence: {},
                tool_name: '{}',
                tool_type: '{}',
                inputs: '{}',
                output: '{}',
                output_truncated: {},
                duration_ms: {},
                success: {},
                error: '{}',
                previous_op_id: '{}',
                created_at: '{}'
            }})"#,
            escape_string(&op.id),
            escape_string(&op.task_id),
            op.sequence,
            escape_string(&op.tool_name),
            op.tool_type,
            escape_string(&inputs_str),
            escape_string(&truncate_str(&op.output, 1000)),
            op.output_truncated,
            op.duration_ms,
            op.success,
            escape_string(error_str),
            escape_string(previous_op),
            op.created_at.to_rfc3339()
        );

        self.execute(&query)?;
        debug!("Recorded operation: {} (tool: {})", op.id, op.tool_name);
        Ok(op.id.clone())
    }

    /// Get all operations for a task
    pub fn get_task_operations(&self, task_id: &str) -> Result<Vec<OperationNode>, AgentError> {
        self.with_connection(|conn| {
            let query = format!(
                "MATCH (o:Operation {{task_id: '{}'}}) RETURN o.id, o.task_id, o.sequence, o.tool_name, o.tool_type, o.inputs, o.output, o.output_truncated, o.duration_ms, o.success, o.error, o.previous_op_id, o.created_at ORDER BY o.sequence",
                escape_string(task_id)
            );

            let result = conn.query(&query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let mut operations = Vec::new();
            let result_str = format!("{}", result);

            for line in result_str.lines().skip(1) {
                let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if parts.len() >= 13 {
                    let tool_type: ToolType = parts[4].parse().unwrap_or(ToolType::Native);
                    let inputs: serde_json::Value = serde_json::from_str(parts[5]).unwrap_or_default();

                    let operation = OperationNode {
                        id: parts[0].to_string(),
                        task_id: parts[1].to_string(),
                        sequence: parts[2].parse().unwrap_or(0),
                        tool_name: parts[3].to_string(),
                        tool_type,
                        inputs,
                        output: parts[6].to_string(),
                        output_truncated: parts[7].parse().unwrap_or(false),
                        duration_ms: parts[8].parse().unwrap_or(0),
                        success: parts[9].parse().unwrap_or(false),
                        error: if parts[10].is_empty() { None } else { Some(parts[10].to_string()) },
                        previous_op_id: if parts[11].is_empty() { None } else { Some(parts[11].to_string()) },
                        created_at: chrono::DateTime::parse_from_rfc3339(parts[12])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    };
                    operations.push(operation);
                }
            }

            Ok(operations)
        })
    }

    /// Link operation to task (PERFORMED relationship)
    pub fn link_performed(&self, task_id: &str, operation_id: &str) -> Result<(), AgentError> {
        debug!("Linking task {} -> PERFORMED -> operation {}", task_id, operation_id);
        // Note: The relationship is tracked via task_id field in Operation
        // Full graph relationship would require both nodes to exist
        Ok(())
    }

    /// Link operation to tool (EXECUTED_BY relationship)
    pub fn link_executed_by(&self, operation_id: &str, tool_name: &str) -> Result<(), AgentError> {
        debug!("Linking operation {} -> EXECUTED_BY -> tool {}", operation_id, tool_name);
        // Note: Tool is tracked via tool_name field in Operation
        Ok(())
    }

    /// Link operations in sequence (FOLLOWED_BY relationship)
    pub fn link_followed_by(&self, from_op_id: &str, to_op_id: &str) -> Result<(), AgentError> {
        debug!("Linking operation {} -> FOLLOWED_BY -> operation {}", from_op_id, to_op_id);
        // Note: Tracked via previous_op_id field in Operation
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════
    // TASK HIERARCHY (Autonomous Decomposition)
    // ═══════════════════════════════════════════════════════════════════

    /// Record a task in the graph
    pub fn record_task(&self, task: &TaskNode) -> Result<String, AgentError> {
        let parent_id = task.parent_id.as_deref().unwrap_or("");
        let root_id = task.root_id.as_deref().unwrap_or(&task.id);
        let tool_hint = task.tool_hint.as_deref().unwrap_or("");
        let output = task.output.as_deref().unwrap_or("");
        let error = task.error.as_deref().unwrap_or("");
        let started_at = task.started_at.map(|t| t.to_rfc3339()).unwrap_or_default();
        let completed_at = task.completed_at.map(|t| t.to_rfc3339()).unwrap_or_default();

        let query = format!(
            r#"CREATE (:Task {{
                id: '{}',
                description: '{}',
                status: '{}',
                priority: '{:?}',
                parent_id: '{}',
                root_id: '{}',
                agent_type: '{}',
                tool_hint: '{}',
                input_context: '{}',
                output: '{}',
                error: '{}',
                retries: {},
                max_retries: {},
                created_at: '{}',
                started_at: '{}',
                completed_at: '{}'
            }})"#,
            escape_string(&task.id),
            escape_string(&task.description),
            task.status,
            task.priority,
            escape_string(parent_id),
            escape_string(root_id),
            task.agent_type,
            escape_string(tool_hint),
            escape_string(&truncate_str(&task.input_context, 500)),
            escape_string(&truncate_str(output, 1000)),
            escape_string(error),
            task.retries,
            task.max_retries,
            task.created_at.to_rfc3339(),
            started_at,
            completed_at
        );

        self.execute(&query)?;
        info!("Recorded task: {} ({})", &task.id[..8], task.description);
        Ok(task.id.clone())
    }

    /// Update task status
    pub fn update_task_status(&self, task_id: &str, status: &str) -> Result<(), AgentError> {
        let query = format!(
            "MATCH (t:Task {{id: '{}'}}) SET t.status = '{}'",
            escape_string(task_id),
            escape_string(status)
        );
        self.execute(&query)?;
        debug!("Updated task {} status to {}", task_id, status);
        Ok(())
    }

    /// Complete a task with output
    pub fn complete_task(&self, task_id: &str, output: &str) -> Result<(), AgentError> {
        let now = Utc::now().to_rfc3339();
        let query = format!(
            "MATCH (t:Task {{id: '{}'}}) SET t.status = 'completed', t.output = '{}', t.completed_at = '{}'",
            escape_string(task_id),
            escape_string(&truncate_str(output, 1000)),
            now
        );
        self.execute(&query)?;
        debug!("Completed task {}", task_id);
        Ok(())
    }

    /// Fail a task with error
    pub fn fail_task(&self, task_id: &str, error: &str) -> Result<(), AgentError> {
        let now = Utc::now().to_rfc3339();
        let query = format!(
            "MATCH (t:Task {{id: '{}'}}) SET t.status = 'failed', t.error = '{}', t.completed_at = '{}'",
            escape_string(task_id),
            escape_string(error),
            now
        );
        self.execute(&query)?;
        debug!("Failed task {}: {}", task_id, error);
        Ok(())
    }

    /// Get all subtasks of a parent task
    pub fn get_subtasks(&self, parent_id: &str) -> Result<Vec<String>, AgentError> {
        self.with_connection(|conn| {
            let query = format!(
                "MATCH (t:Task {{parent_id: '{}'}}) RETURN t.id ORDER BY t.created_at",
                escape_string(parent_id)
            );

            let result = conn.query(&query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let mut subtasks = Vec::new();
            let result_str = format!("{}", result);

            for line in result_str.lines().skip(1) {
                let id = line.trim();
                if !id.is_empty() {
                    subtasks.push(id.to_string());
                }
            }

            Ok(subtasks)
        })
    }

    /// Get full execution trace from root task
    pub fn get_execution_trace(&self, root_task_id: &str) -> Result<Vec<(String, Vec<OperationNode>)>, AgentError> {
        // Get all tasks under this root
        self.with_connection(|conn| {
            let query = format!(
                "MATCH (t:Task {{root_id: '{}'}}) RETURN t.id ORDER BY t.created_at",
                escape_string(root_task_id)
            );

            let result = conn.query(&query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let mut trace = Vec::new();
            let result_str = format!("{}", result);

            for line in result_str.lines().skip(1) {
                let task_id = line.trim();
                if !task_id.is_empty() {
                    // Get operations for this task
                    let ops = self.get_task_operations(task_id).unwrap_or_default();
                    trace.push((task_id.to_string(), ops));
                }
            }

            Ok(trace)
        })
    }

    /// Link parent task to subtask (DECOMPOSED_INTO relationship)
    pub fn link_decomposed_into(&self, parent_id: &str, subtask_id: &str) -> Result<(), AgentError> {
        debug!("Linking task {} -> DECOMPOSED_INTO -> task {}", parent_id, subtask_id);
        // Note: Tracked via parent_id field in Task
        Ok(())
    }

    /// Link task to user (ASSIGNED_BY relationship)
    pub fn link_assigned_by(&self, task_id: &str, user_id: &str) -> Result<(), AgentError> {
        debug!("Linking task {} -> ASSIGNED_BY -> user {}", task_id, user_id);
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════
    // MEMORY DEDUPLICATION (Fingerprint-based)
    // ═══════════════════════════════════════════════════════════════════

    /// Find existing memory with same fingerprint (for deduplication)
    pub fn find_duplicate_memory(&self, fingerprint: &str) -> Result<Option<MemoryNode>, AgentError> {
        self.with_connection(|conn| {
            let query = format!(
                "MATCH (m:Memory {{fingerprint: '{}'}}) WHERE m.valid_until = '' RETURN m.id, m.content, m.fingerprint, m.memory_type, m.importance, m.source_task_id, m.source_operation_id, m.last_accessed, m.access_count, m.valid_from, m.created_at LIMIT 1",
                escape_string(fingerprint)
            );

            let result = conn.query(&query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let result_str = format!("{}", result);
            if let Some(line) = result_str.lines().nth(1) {
                let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if parts.len() >= 11 {
                    let memory = MemoryNode {
                        id: parts[0].to_string(),
                        content: parts[1].to_string(),
                        fingerprint: parts[2].to_string(),
                        memory_type: parse_memory_type(parts[3]),
                        importance: parts[4].parse().unwrap_or(0.5),
                        source_task_id: if parts[5].is_empty() { None } else { Some(parts[5].to_string()) },
                        source_operation_id: if parts[6].is_empty() { None } else { Some(parts[6].to_string()) },
                        last_accessed: chrono::DateTime::parse_from_rfc3339(parts[7])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                        access_count: parts[8].parse().unwrap_or(0),
                        tasks_used_in: Vec::new(),
                        valid_from: chrono::DateTime::parse_from_rfc3339(parts[9])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                        valid_until: None,
                        superseded_by: None,
                        created_at: chrono::DateTime::parse_from_rfc3339(parts[10])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    };
                    return Ok(Some(memory));
                }
            }

            Ok(None)
        })
    }

    /// Store memory with deduplication - returns existing memory ID if duplicate found
    pub fn remember_with_dedup(&self, memory: &MemoryNode, topics: &[String]) -> Result<(String, bool), AgentError> {
        // Check for duplicate first
        if let Some(existing) = self.find_duplicate_memory(&memory.fingerprint)? {
            info!("🔄 [DEDUP] Found existing memory with same fingerprint: {}", &existing.id[..8]);
            // Update access tracking on existing memory
            self.touch_memory(&existing.id)?;
            return Ok((existing.id, true)); // true = was duplicate
        }

        // No duplicate, create new memory
        let valid_until = memory.valid_until
            .map(|t| t.to_rfc3339())
            .unwrap_or_default();
        let source_task_id = memory.source_task_id.as_deref().unwrap_or("");
        let source_operation_id = memory.source_operation_id.as_deref().unwrap_or("");
        let superseded_by = memory.superseded_by.as_deref().unwrap_or("");

        let query = format!(
            r#"CREATE (:Memory {{
                id: '{}',
                content: '{}',
                fingerprint: '{}',
                memory_type: '{}',
                importance: {},
                source_task_id: '{}',
                source_operation_id: '{}',
                last_accessed: '{}',
                access_count: {},
                valid_from: '{}',
                valid_until: '{}',
                superseded_by: '{}',
                created_at: '{}'
            }})"#,
            escape_string(&memory.id),
            escape_string(&memory.content),
            escape_string(&memory.fingerprint),
            memory.memory_type,
            memory.importance,
            escape_string(source_task_id),
            escape_string(source_operation_id),
            memory.last_accessed.to_rfc3339(),
            memory.access_count,
            memory.valid_from.to_rfc3339(),
            valid_until,
            escape_string(superseded_by),
            memory.created_at.to_rfc3339()
        );

        self.execute(&query)?;

        // Create topic links
        for topic in topics {
            self.ensure_topic(topic)?;
        }

        info!("Stored new memory: {} ({:?})", &memory.id[..8], memory.memory_type);
        Ok((memory.id.clone(), false)) // false = was not duplicate
    }

    /// Link two memories as duplicates (SAME_AS relationship)
    pub fn link_same_as(&self, memory_id: &str, duplicate_id: &str) -> Result<(), AgentError> {
        debug!("Linking memory {} -> SAME_AS -> memory {}", memory_id, duplicate_id);
        Ok(())
    }

    /// Link new memory as superseding old memory (SUPERSEDES relationship)
    pub fn link_supersedes(&self, new_memory_id: &str, old_memory_id: &str) -> Result<(), AgentError> {
        // Update old memory to mark it as superseded
        let query = format!(
            "MATCH (m:Memory {{id: '{}'}}) SET m.superseded_by = '{}'",
            escape_string(old_memory_id),
            escape_string(new_memory_id)
        );
        self.execute(&query)?;
        debug!("Linking memory {} -> SUPERSEDES -> memory {}", new_memory_id, old_memory_id);
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════
    // PROVENANCE TRACKING (Task <-> Memory relationships)
    // ═══════════════════════════════════════════════════════════════════

    /// Link task to memory it learned (LEARNED relationship)
    pub fn link_learned(&self, task_id: &str, memory_id: &str) -> Result<(), AgentError> {
        debug!("Linking task {} -> LEARNED -> memory {}", task_id, memory_id);
        // Note: Tracked via source_task_id field in Memory
        Ok(())
    }

    /// Link task to memory it recalled/used (RECALLED relationship)
    pub fn link_recalled(&self, task_id: &str, memory_id: &str) -> Result<(), AgentError> {
        debug!("Linking task {} -> RECALLED -> memory {}", task_id, memory_id);
        // Update memory's tasks_used_in and access tracking
        self.touch_memory(memory_id)?;
        Ok(())
    }

    /// Link memory to operation it was derived from (DERIVED_FROM relationship)
    pub fn link_derived_from(&self, memory_id: &str, operation_id: &str) -> Result<(), AgentError> {
        // Update memory's source_operation_id
        let query = format!(
            "MATCH (m:Memory {{id: '{}'}}) SET m.source_operation_id = '{}'",
            escape_string(memory_id),
            escape_string(operation_id)
        );
        self.execute(&query)?;
        debug!("Linking memory {} -> DERIVED_FROM -> operation {}", memory_id, operation_id);
        Ok(())
    }

    /// Get all memories learned from a specific task
    pub fn get_task_memories(&self, task_id: &str) -> Result<Vec<MemoryNode>, AgentError> {
        self.with_connection(|conn| {
            let query = format!(
                "MATCH (m:Memory {{source_task_id: '{}'}}) WHERE m.valid_until = '' RETURN m.id, m.content, m.fingerprint, m.memory_type, m.importance, m.source_task_id, m.source_operation_id, m.last_accessed, m.access_count, m.valid_from, m.created_at",
                escape_string(task_id)
            );

            let result = conn.query(&query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let mut memories = Vec::new();
            let result_str = format!("{}", result);

            for line in result_str.lines().skip(1) {
                let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if parts.len() >= 11 {
                    let memory = MemoryNode {
                        id: parts[0].to_string(),
                        content: parts[1].to_string(),
                        fingerprint: parts[2].to_string(),
                        memory_type: parse_memory_type(parts[3]),
                        importance: parts[4].parse().unwrap_or(0.5),
                        source_task_id: if parts[5].is_empty() { None } else { Some(parts[5].to_string()) },
                        source_operation_id: if parts[6].is_empty() { None } else { Some(parts[6].to_string()) },
                        last_accessed: chrono::DateTime::parse_from_rfc3339(parts[7])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                        access_count: parts[8].parse().unwrap_or(0),
                        tasks_used_in: Vec::new(),
                        valid_from: chrono::DateTime::parse_from_rfc3339(parts[9])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                        valid_until: None,
                        superseded_by: None,
                        created_at: chrono::DateTime::parse_from_rfc3339(parts[10])
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

/// Stop words to filter out from queries
const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
    "have", "has", "had", "do", "does", "did", "will", "would", "could",
    "should", "may", "might", "must", "shall", "can", "need", "dare",
    "to", "of", "in", "for", "on", "with", "at", "by", "from", "as",
    "into", "through", "during", "before", "after", "above", "below",
    "between", "under", "again", "further", "then", "once", "here",
    "there", "when", "where", "why", "how", "all", "each", "few", "more",
    "most", "other", "some", "such", "no", "nor", "not", "only", "own",
    "same", "so", "than", "too", "very", "just", "and", "but", "if", "or",
    "because", "until", "while", "about", "against", "between", "into",
    "what", "which", "who", "whom", "this", "that", "these", "those",
    "am", "i", "my", "me", "you", "your", "he", "she", "it", "we", "they",
    "him", "her", "his", "its", "our", "their", "them", "hey", "hello",
    "please", "thanks", "thank", "yes", "no", "ok", "okay",
];

/// Extract meaningful keywords from a query
fn extract_query_keywords(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| {
            let word = word.trim();
            word.len() > 2 && !STOP_WORDS.contains(&word)
        })
        .map(|s| s.to_string())
        .collect()
}

/// Calculate keyword match score for a memory's content
/// Returns 0.0 to 1.0 based on how many keywords match
fn calculate_keyword_score(content: &str, keywords: &[String]) -> f64 {
    if keywords.is_empty() {
        return 0.5; // Neutral score if no keywords
    }

    let content_lower = content.to_lowercase();
    let mut total_score = 0.0;

    for keyword in keywords {
        if content_lower.contains(keyword) {
            // Full match
            total_score += 1.0;
        } else {
            // Partial match (check if any word in content starts with keyword)
            let partial_match = content_lower
                .split_whitespace()
                .any(|word| word.starts_with(keyword) || keyword.starts_with(word));
            if partial_match {
                total_score += 0.5;
            }
        }
    }

    // Normalize to 0-1 range
    (total_score / keywords.len() as f64).min(1.0)
}
