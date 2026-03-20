//! Reusable Cypher query builders for common graph operations.

/// Query to find tools that operate on topics the user is interested in
pub fn tools_for_user_interests() -> &'static str {
    r#"
    MATCH (u:User)-[:INTERESTED_IN]->(t:Topic)<-[:OPERATES_ON]-(tool:Tool)
    WHERE u.id = $user_id AND tool.enabled = true
    RETURN tool.name, tool.description, tool.tool_type, t.name AS topic
    "#
}

/// Query to find tools that compose well with a given tool
pub fn composable_tools() -> &'static str {
    r#"
    MATCH (a:Tool)-[r:COMPOSES_WITH]->(b:Tool)
    WHERE a.name = $tool_name AND b.enabled = true
    RETURN b.name, b.description, r.description AS composition_desc
    "#
}

/// Query to get all user preferences (current memories)
pub fn user_preferences() -> &'static str {
    r#"
    MATCH (u:User)-[:PREFERS]->(m:Memory)
    WHERE u.id = $user_id AND m.valid_until IS NULL
    RETURN m.content, m.memory_type, m.importance
    ORDER BY m.importance DESC
    "#
}

/// Query to find tools used in recent successful episodes
pub fn successful_tool_usage() -> &'static str {
    r#"
    MATCH (tool:Tool)-[:USED_IN]->(e:Episode)
    WHERE e.success = true
    RETURN tool.name, COUNT(e) AS times_used
    ORDER BY times_used DESC
    "#
}

/// Query to get all enabled tools
pub fn all_enabled_tools() -> &'static str {
    r#"
    MATCH (t:Tool)
    WHERE t.enabled = true
    RETURN t.id, t.name, t.description, t.tool_type, t.input_schema, t.source,
           t.enabled, t.usage_count, t.success_rate, t.created_at, t.last_used_at
    "#
}

/// Query to get tools by type
pub fn tools_by_type() -> &'static str {
    r#"
    MATCH (t:Tool)
    WHERE t.tool_type = $tool_type AND t.enabled = true
    RETURN t.id, t.name, t.description, t.tool_type, t.input_schema, t.source,
           t.enabled, t.usage_count, t.success_rate, t.created_at, t.last_used_at
    "#
}

/// Query to get a tool by name
pub fn tool_by_name() -> &'static str {
    r#"
    MATCH (t:Tool)
    WHERE t.name = $name
    RETURN t.id, t.name, t.description, t.tool_type, t.input_schema, t.source,
           t.enabled, t.usage_count, t.success_rate, t.created_at, t.last_used_at
    "#
}

/// Query to get tools for a specific topic
pub fn tools_for_topic() -> &'static str {
    r#"
    MATCH (tool:Tool)-[:OPERATES_ON]->(t:Topic)
    WHERE t.name = $topic AND tool.enabled = true
    RETURN tool.name, tool.description, tool.tool_type
    "#
}

/// Query to search memories by keywords
pub fn search_memories() -> &'static str {
    r#"
    MATCH (m:Memory)
    WHERE m.valid_until IS NULL AND m.content CONTAINS $keyword
    RETURN m.id, m.content, m.memory_type, m.importance, m.created_at
    ORDER BY m.importance DESC, m.created_at DESC
    LIMIT $limit
    "#
}

/// Query to get all MCP servers
pub fn all_mcp_servers() -> &'static str {
    r#"
    MATCH (s:MCPServer)
    RETURN s.id, s.name, s.url, s.transport, s.status, s.auto_connect, s.last_connected_at
    "#
}

/// Query to get tools hosted on an MCP server
pub fn tools_on_server() -> &'static str {
    r#"
    MATCH (t:Tool)-[:HOSTED_ON]->(s:MCPServer)
    WHERE s.name = $server_name
    RETURN t.name, t.description, t.enabled
    "#
}

/// Query to get recent episodes for a user
pub fn recent_episodes() -> &'static str {
    r#"
    MATCH (u:User)-[:INTERACTED]->(e:Episode)
    WHERE u.id = $user_id
    RETURN e.id, e.user_input, e.agent_response, e.tools_used, e.success,
           e.duration_ms, e.tokens_used, e.cost_usd, e.created_at
    ORDER BY e.created_at DESC
    LIMIT $limit
    "#
}

/// Query to count nodes by type
pub fn count_nodes() -> &'static str {
    r#"
    MATCH (n)
    RETURN labels(n)[0] AS label, COUNT(*) AS count
    "#
}

/// Query to get topic by name
pub fn topic_by_name() -> &'static str {
    r#"
    MATCH (t:Topic)
    WHERE t.name = $name
    RETURN t.id, t.name
    "#
}

/// Query to check for contradicting memories
pub fn find_contradictions() -> &'static str {
    r#"
    MATCH (m:Memory)-[:ABOUT]->(t:Topic)
    WHERE t.name IN $topics AND m.valid_until IS NULL AND m.memory_type = $memory_type
    RETURN m.id, m.content, m.memory_type, m.importance
    "#
}

/// Query to get user by ID
pub fn user_by_id() -> &'static str {
    r#"
    MATCH (u:User)
    WHERE u.id = $user_id
    RETURN u.id, u.name, u.telegram_chat_id, u.timezone, u.created_at
    "#
}
