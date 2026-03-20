//! Graph schema definitions.
//! Note: lbug uses prepared statements - schema creation is handled in GraphBrain::open()

use crate::types::AgentError;

/// Schema creation is handled directly in GraphBrain for now
/// This module provides schema constants and utilities

pub const SCHEMA_VERSION: u32 = 1;

/// Initialize the database schema using raw query execution
pub fn create_schema_queries() -> Vec<&'static str> {
    vec![
        // Node tables
        r#"CREATE NODE TABLE IF NOT EXISTS User(
            id STRING,
            name STRING,
            telegram_chat_id STRING,
            timezone STRING DEFAULT 'UTC',
            created_at TIMESTAMP,
            PRIMARY KEY(id)
        )"#,
        r#"CREATE NODE TABLE IF NOT EXISTS Tool(
            id STRING,
            name STRING,
            description STRING,
            tool_type STRING,
            input_schema STRING,
            source STRING,
            enabled BOOLEAN DEFAULT true,
            usage_count INT64 DEFAULT 0,
            success_rate DOUBLE DEFAULT 1.0,
            created_at TIMESTAMP,
            last_used_at TIMESTAMP,
            PRIMARY KEY(id)
        )"#,
        r#"CREATE NODE TABLE IF NOT EXISTS MCPServer(
            id STRING,
            name STRING,
            url STRING,
            transport STRING,
            status STRING,
            auto_connect BOOLEAN DEFAULT true,
            last_connected_at TIMESTAMP,
            PRIMARY KEY(id)
        )"#,
        r#"CREATE NODE TABLE IF NOT EXISTS Memory(
            id STRING,
            content STRING,
            memory_type STRING,
            importance DOUBLE DEFAULT 0.5,
            valid_from TIMESTAMP,
            valid_until TIMESTAMP,
            created_at TIMESTAMP,
            PRIMARY KEY(id)
        )"#,
        r#"CREATE NODE TABLE IF NOT EXISTS Episode(
            id STRING,
            user_input STRING,
            agent_response STRING,
            tools_used STRING,
            success BOOLEAN,
            duration_ms INT64,
            tokens_used INT64,
            cost_usd DOUBLE,
            created_at TIMESTAMP,
            PRIMARY KEY(id)
        )"#,
        r#"CREATE NODE TABLE IF NOT EXISTS Topic(
            id STRING,
            name STRING,
            PRIMARY KEY(id)
        )"#,
        // Relationship tables
        "CREATE REL TABLE IF NOT EXISTS INTERESTED_IN(FROM User TO Topic)",
        "CREATE REL TABLE IF NOT EXISTS PREFERS(FROM User TO Memory)",
        "CREATE REL TABLE IF NOT EXISTS INTERACTED(FROM User TO Episode)",
        "CREATE REL TABLE IF NOT EXISTS HOSTED_ON(FROM Tool TO MCPServer)",
        "CREATE REL TABLE IF NOT EXISTS OPERATES_ON(FROM Tool TO Topic)",
        "CREATE REL TABLE IF NOT EXISTS COMPOSES_WITH(FROM Tool TO Tool, description STRING)",
        "CREATE REL TABLE IF NOT EXISTS USED_IN(FROM Tool TO Episode)",
        "CREATE REL TABLE IF NOT EXISTS ABOUT(FROM Memory TO Topic)",
        "CREATE REL TABLE IF NOT EXISTS LEARNED_FROM(FROM Memory TO Episode)",
        "CREATE REL TABLE IF NOT EXISTS CONTRADICTS(FROM Memory TO Memory)",
    ]
}
