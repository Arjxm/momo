//! Graph schema definitions for the autonomous agent entity-relationship model.
//!
//! Core Entities:
//! - Task: User's request / Agent's subtask with hierarchy support
//! - Operation: Single tool execution record
//! - Tool: Available capability (Native, MCP, Skill, Browser)
//! - Memory: Learned fact/preference with fingerprint dedup
//! - Artifact: Output produced (files, results, summaries)
//!
//! Supporting Entities:
//! - User, Episode, Topic, MCPServer, Plan

use crate::types::AgentError;

/// Schema creation is handled directly in GraphBrain for now
/// This module provides schema constants and utilities

pub const SCHEMA_VERSION: u32 = 3;

/// Initialize the database schema using raw query execution
pub fn create_schema_queries() -> Vec<&'static str> {
    vec![
        // ═══════════════════════════════════════════════════════════════════
        // NODE TABLES
        // ═══════════════════════════════════════════════════════════════════

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

        // ═══════════════════════════════════════════════════════════════════
        // OPERATION TABLE (NEW - Tool Execution Record)
        // ═══════════════════════════════════════════════════════════════════
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

        // ═══════════════════════════════════════════════════════════════════
        // MEMORY TABLE (ENHANCED - with fingerprint for dedup)
        // ═══════════════════════════════════════════════════════════════════
        r#"CREATE NODE TABLE IF NOT EXISTS Memory(
            id STRING,
            content STRING,
            fingerprint STRING,
            memory_type STRING,
            importance DOUBLE DEFAULT 0.5,
            source_task_id STRING,
            source_operation_id STRING,
            last_accessed STRING,
            access_count INT64 DEFAULT 0,
            valid_from STRING,
            valid_until STRING,
            superseded_by STRING,
            created_at STRING,
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

        r#"CREATE NODE TABLE IF NOT EXISTS Plan(
            id STRING,
            original_request STRING,
            status STRING DEFAULT 'pending',
            created_at TIMESTAMP,
            completed_at TIMESTAMP,
            PRIMARY KEY(id)
        )"#,

        // ═══════════════════════════════════════════════════════════════════
        // TASK TABLE (ENHANCED - with hierarchy for autonomous decomposition)
        // ═══════════════════════════════════════════════════════════════════
        r#"CREATE NODE TABLE IF NOT EXISTS Task(
            id STRING,
            description STRING,
            status STRING DEFAULT 'pending',
            priority STRING DEFAULT 'normal',
            parent_id STRING,
            root_id STRING,
            agent_type STRING,
            tool_hint STRING,
            input_context STRING,
            output STRING,
            error STRING,
            retries INT64 DEFAULT 0,
            max_retries INT64 DEFAULT 3,
            created_at STRING,
            started_at STRING,
            completed_at STRING,
            PRIMARY KEY(id)
        )"#,

        // ═══════════════════════════════════════════════════════════════════
        // RELATIONSHIP TABLES - User Relations
        // ═══════════════════════════════════════════════════════════════════
        "CREATE REL TABLE IF NOT EXISTS INTERESTED_IN(FROM User TO Topic)",
        "CREATE REL TABLE IF NOT EXISTS PREFERS(FROM User TO Memory)",
        "CREATE REL TABLE IF NOT EXISTS INTERACTED(FROM User TO Episode)",

        // ═══════════════════════════════════════════════════════════════════
        // RELATIONSHIP TABLES - Task Hierarchy (Autonomous Decomposition)
        // ═══════════════════════════════════════════════════════════════════
        "CREATE REL TABLE IF NOT EXISTS ASSIGNED_BY(FROM Task TO User)",
        "CREATE REL TABLE IF NOT EXISTS DECOMPOSED_INTO(FROM Task TO Task)",
        "CREATE REL TABLE IF NOT EXISTS DEPENDS_ON(FROM Task TO Task)",
        "CREATE REL TABLE IF NOT EXISTS BLOCKED_BY(FROM Task TO Task)",

        // ═══════════════════════════════════════════════════════════════════
        // RELATIONSHIP TABLES - Task Execution
        // ═══════════════════════════════════════════════════════════════════
        "CREATE REL TABLE IF NOT EXISTS PERFORMED(FROM Task TO Operation)",
        "CREATE REL TABLE IF NOT EXISTS PRODUCED(FROM Task TO Memory)",
        "CREATE REL TABLE IF NOT EXISTS LEARNED(FROM Task TO Memory)",
        "CREATE REL TABLE IF NOT EXISTS RECALLED(FROM Task TO Memory)",

        // ═══════════════════════════════════════════════════════════════════
        // RELATIONSHIP TABLES - Operation Tracking
        // ═══════════════════════════════════════════════════════════════════
        "CREATE REL TABLE IF NOT EXISTS EXECUTED_BY(FROM Operation TO Tool)",
        "CREATE REL TABLE IF NOT EXISTS FOLLOWED_BY(FROM Operation TO Operation)",
        "CREATE REL TABLE IF NOT EXISTS FAILED_WITH(FROM Operation TO Topic)",

        // ═══════════════════════════════════════════════════════════════════
        // RELATIONSHIP TABLES - Tool Provenance
        // ═══════════════════════════════════════════════════════════════════
        "CREATE REL TABLE IF NOT EXISTS HOSTED_ON(FROM Tool TO MCPServer)",
        "CREATE REL TABLE IF NOT EXISTS PROVIDED_BY(FROM Tool TO MCPServer)",
        "CREATE REL TABLE IF NOT EXISTS OPERATES_ON(FROM Tool TO Topic)",
        "CREATE REL TABLE IF NOT EXISTS EFFECTIVE_FOR(FROM Tool TO Topic)",
        "CREATE REL TABLE IF NOT EXISTS COMPOSES_WITH(FROM Tool TO Tool, description STRING)",
        "CREATE REL TABLE IF NOT EXISTS USED_IN(FROM Tool TO Episode)",

        // ═══════════════════════════════════════════════════════════════════
        // RELATIONSHIP TABLES - Memory Graph
        // ═══════════════════════════════════════════════════════════════════
        "CREATE REL TABLE IF NOT EXISTS ABOUT(FROM Memory TO Topic)",
        "CREATE REL TABLE IF NOT EXISTS SAME_AS(FROM Memory TO Memory)",
        "CREATE REL TABLE IF NOT EXISTS CONTRADICTS(FROM Memory TO Memory)",
        "CREATE REL TABLE IF NOT EXISTS SUPERSEDES(FROM Memory TO Memory)",
        "CREATE REL TABLE IF NOT EXISTS DERIVED_FROM(FROM Memory TO Operation)",
        "CREATE REL TABLE IF NOT EXISTS LEARNED_FROM(FROM Memory TO Episode)",

        // ═══════════════════════════════════════════════════════════════════
        // RELATIONSHIP TABLES - Plan Relations
        // ═══════════════════════════════════════════════════════════════════
        "CREATE REL TABLE IF NOT EXISTS HAS_TASK(FROM Plan TO Task)",
        "CREATE REL TABLE IF NOT EXISTS INITIATED_BY(FROM Plan TO User)",
    ]
}
