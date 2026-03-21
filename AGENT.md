# Agent Brain - Development Memory

## Project Overview

A Rust CLI AI agent that uses LLMs to reason and execute tools (ReAct pattern). Supports multiple LLM providers, persistent graph memory, MCP tool integration, browser automation, and a debug visualization server.

## Current Version: 0.3.0

## Architecture

```
agent-brain/
├── Cargo.toml
├── src/
│   ├── main.rs              # Entry point, REPL loop
│   ├── agent.rs             # Simple ReAct agent loop
│   ├── claude.rs            # Legacy Anthropic-specific client
│   ├── config.rs            # Configuration management
│   ├── types.rs             # Shared types and error handling
│   ├── debug_server.rs      # Web visualization server (warp + vis.js)
│   ├── providers/           # Multi-provider LLM support
│   │   ├── mod.rs           # LLMProvider trait, ProviderType enum
│   │   ├── anthropic.rs     # Claude models
│   │   ├── openai_compat.rs # OpenAI, DeepSeek, Ollama, Groq, Together, LMStudio, OpenRouter
│   │   └── gemini.rs        # Google Gemini
│   ├── graph/               # Persistent memory (lbug/LadybugDB)
│   │   ├── mod.rs           # GraphBrain - main interface
│   │   ├── memory.rs        # Memory extraction from conversations
│   │   ├── queries.rs       # Cypher query templates
│   │   └── schema.rs        # Database schema
│   ├── tools/               # Tool implementations
│   │   ├── mod.rs           # ToolRegistry, Tool trait
│   │   ├── arxiv.rs         # arXiv paper search
│   │   ├── calculator.rs    # Math expression parser
│   │   ├── web_fetch.rs     # HTTP fetching
│   │   ├── browser.rs       # Headless Chrome automation (chromiumoxide)
│   │   ├── mcp_client.rs    # MCP protocol client
│   │   └── mcp_bridge.rs    # MCP server management
│   ├── orchestrator/        # Multi-agent task orchestration
│   │   ├── mod.rs           # Orchestrator main
│   │   ├── types.rs         # Plan, TaskNode, AgentType
│   │   ├── task_queue.rs    # Task scheduling
│   │   ├── workers.rs       # Worker pool
│   │   └── skill_factory.rs # Dynamic skill generation
│   └── skills/              # WASM-based user skills
│       ├── mod.rs           # SkillManager
│       ├── loader.rs        # Skill manifest loading
│       ├── registry.rs      # Skill registration
│       └── sandbox.rs       # WASM sandbox (wasmtime)
```

## Key Components

### 1. LLM Providers (`src/providers/`)

Unified `LLMProvider` trait supporting:
- **Anthropic** - Claude models (claude-3-5-sonnet, claude-3-opus, etc.)
- **OpenAI** - GPT-4o, GPT-4-turbo, o1
- **Gemini** - Google's models (requires schema cleaning for `additionalProperties`)
- **DeepSeek** - Very cheap alternative
- **Ollama** - Local models (free, no API key)
- **LM Studio** - Local models (free, no API key)
- **Groq** - Fast inference
- **Together AI** - Various open models
- **OpenRouter** - Model routing

Configuration via `provider_config.json`:
```json
{
  "provider_type": "lmstudio",
  "api_key": "",
  "model": "qwen2.5-3b-instruct",
  "max_tokens": 4096
}
```

### 2. Graph Memory (`src/graph/`)

Persistent memory using LadybugDB (lbug crate):
- **MemoryNode** - Facts, preferences, episode summaries (with access tracking)
- **EpisodeNode** - Interaction history
- **TopicNode** - Subject categorization
- **ToolNode** - In-memory tool registry (session-based)

#### Smart Memory Retrieval

Multi-factor relevance scoring:
```
score = (importance × 0.25) + (recency × 0.30) + (frequency × 0.15) + (keyword_match × 0.30)
```

Features:
- **Recency decay** - Memories decay with 7-day half-life
- **Access tracking** - `last_accessed` and `access_count` fields
- **Keyword extraction** - Filters stop words, extracts meaningful terms
- **Session history** - Tracks conversation within current session

Key methods on `GraphBrain`:
- `smart_recall(query, limit)` - Smart retrieval with scoring
- `smart_recall_prefs(query, limit)` - Smart preference retrieval
- `remember()` - Store a memory with access tracking
- `touch_memory()` - Update access timestamp and count
- `recall()` - Legacy method (calls smart_recall internally)
- `record_episode()` - Log an interaction
- `stats()` - Get node counts

#### MemoryNode Fields
```rust
struct MemoryNode {
    id: String,
    content: String,
    memory_type: MemoryType,  // Fact, Preference, EpisodeSummary
    importance: f64,          // 0.0 to 1.0
    last_accessed: DateTime,  // For recency scoring
    access_count: u32,        // For frequency scoring
}
```

Database location: `~/.agent-brain/graph.db`

### 3. Tools (`src/tools/`)

Native tools:
- `calculator` - Math expressions
- `arxiv_search` - Paper search
- `web_fetch` - HTTP GET with HTML stripping
- `browser` - Chrome automation (visible mode)

MCP tools loaded from `mcp_servers.json`:
```json
{
  "servers": [
    {
      "name": "filesystem",
      "command": "npx",
      "args": ["-y", "@anthropic/mcp-filesystem", "/path/to/dir"],
      "enabled": true
    }
  ]
}
```

### 4. Browser Automation (`src/tools/browser.rs`)

Uses chromiumoxide with CDP. Currently runs in **visible mode** (not headless).

Actions:
- `navigate` - Go to URL
- `extract_text` - Get page text
- `extract_links` - Get all links
- `click` - Click element by selector
- `fill` - Fill form field
- `screenshot` - Capture page
- `run_js` - Execute JavaScript
- `get_html` - Get HTML content

### 5. Debug Server (`src/debug_server.rs`)

Web interface at http://localhost:3030 for visualizing:
- Graph statistics (tools, memories, episodes, topics)
- Interactive node graph (vis.js)
- Memory search
- Tool listing

Start with `debug` command in REPL.

## Environment Variables

```bash
# Required for cloud providers
ANTHROPIC_API_KEY=sk-ant-...
OPENAI_API_KEY=sk-...
GOOGLE_API_KEY=...
DEEPSEEK_API_KEY=...
OPENROUTER_API_KEY=...
GROQ_API_KEY=...
TOGETHER_API_KEY=...

# Optional
LLM_MAX_TOKENS=8192        # Default max tokens
RUST_LOG=info              # Logging level
```

## REPL Commands

- `quit` / `exit` - Exit the agent
- `debug` - Start debug visualization server
- `cost` - Show token usage and cost
- `clear` - Clear session conversation history
- `stats` - Show graph statistics
- `mode` - Toggle between simple/orchestration mode
- `provider` - Show current provider info
- Any other input - Send to agent

## Session vs Persistent Memory

| Type | Scope | Storage |
|------|-------|---------|
| Session history | Current session only | In-memory |
| Facts/Preferences | Persistent across sessions | Graph DB |
| Episodes | Persistent across sessions | Graph DB |
| Tools | Reloaded each startup | In-memory |

The agent remembers:
- **Within session**: Full conversation (last 10 messages)
- **Across sessions**: Extracted facts and preferences (scored by relevance)

## Recent Fixes (Session History)

1. **401 Unauthorized** - Orchestrator was using ClaudeClient instead of LLMProvider
2. **402 Payment Required** - max_tokens too high, added LLM_MAX_TOKENS env var
3. **Gemini schema errors** - Added `clean_schema_for_gemini()` to strip unsupported fields
4. **LM Studio API key** - Skipped API key validation for local providers
5. **Browser not visible** - Changed from headless to `.with_head()` mode
6. **Debug page missing memories** - `recall(&[], N)` returns nothing with empty keywords, added `get_all_memories()`
7. **Memory extraction 401** - `MemoryExtractor` was using hardcoded `ClaudeClient`, changed to use configured `LLMProvider`
8. **Too many irrelevant memories** - Implemented smart memory retrieval with multi-factor scoring (recency, importance, frequency, keyword match)
9. **No session history** - Added `ConversationEntry` tracking within sessions, agent now knows previous messages

## Building & Running

```bash
# Build
cargo build --release

# Run
./target/release/agent-brain

# With debug logging
RUST_LOG=debug ./target/release/agent-brain

# With memory-specific logging (see all memory operations)
RUST_LOG=info ./target/release/agent-brain
```

## Memory Logging

The system logs memory operations with emojis for easy identification:

- `🧠 [MEMORY]` - Memory extraction operations (storing new memories)
- `🔍 [RECALL]` - Memory recall operations (retrieving memories)
- `📚 [AGENT]` - Agent using memories for context
- `🧠 [BACKGROUND]` - Background memory extraction tasks

**Log examples:**
```
🧠 [AGENT] Spawning background memory extraction task...
🧠 [BACKGROUND] Memory extraction started for episode: abc123...
🧠 [MEMORY] Starting memory extraction from interaction
🧠 [MEMORY] Calling LLM to extract facts/preferences...
🧠 [MEMORY] Extracted 2 memories from interaction:
🧠 [MEMORY]   1. [fact] (importance: 0.80) "User is working on a Rust project"
🧠 [MEMORY]   2. [preference] (importance: 0.70) "User prefers concise responses"
🧠 [MEMORY] ✅ Stored memory: "User is working on a Rust project" (id: a1b2c3d4)
🔍 [RECALL] Found 3 memories matching keywords:
📚 [AGENT] Using 3 memories and 1 preferences for context
```

**To see detailed memory flow:**
```bash
RUST_LOG=agent_brain::graph=debug,agent_brain::agent=info ./target/release/agent-brain
```

## Configuration Files

All in `~/.agent-brain/`:
- `provider_config.json` - LLM provider settings
- `mcp_servers.json` - MCP server configurations
- `graph.db/` - LadybugDB database directory

## Dependencies (Cargo.toml)

Key crates:
- `tokio` - Async runtime
- `reqwest` - HTTP client
- `serde` / `serde_json` / `toml` - Serialization
- `quick-xml` - arXiv XML parsing
- `chromiumoxide` - Browser automation
- `wasmtime` - WASM sandbox for skills
- `lbug` - Graph database
- `warp` - Debug web server
- `tracing` - Logging

## Next Steps / TODO

1. Add episode nodes to debug visualization
2. Add edges between nodes in graph visualization
3. Implement memory consolidation (merge similar memories)
4. Add streaming responses for long outputs
5. Implement skill hot-reloading
6. Add conversation summarization for context management

## Code Patterns

### Creating a new LLM provider

```rust
// In src/providers/mod.rs, add to ProviderType enum
pub enum ProviderType {
    // ... existing
    NewProvider,
}

// Implement default_base_url(), is_openai_compatible(), Display
// If OpenAI-compatible, it will use openai_compat.rs automatically
// Otherwise, create new file like gemini.rs
```

### Adding a new tool

```rust
// In src/tools/my_tool.rs
pub struct MyTool { ... }

#[async_trait]
impl Tool for MyTool {
    fn definition(&self) -> ToolDefinition { ... }
    async fn execute(&self, input: HashMap<String, Value>) -> Result<String, AgentError> { ... }
}

// Register in main.rs
registry.register(Arc::new(MyTool::new()));
```

### Storing a memory

```rust
let memory = MemoryNode {
    id: uuid::Uuid::new_v4().to_string(),
    content: "User prefers dark mode".to_string(),
    memory_type: MemoryType::Preference,
    importance: 0.8,
    valid_from: Utc::now(),
    valid_until: None,
    created_at: Utc::now(),
};
brain.remember(&memory, &["preferences".to_string(), "ui".to_string()])?;
```

## Troubleshooting

### "n_keep >= n_ctx" error (LM Studio)
Increase context length in LM Studio settings to 8192+

### MCP tools not loading
Check `mcp_servers.json` exists and servers are enabled

### Browser not launching
Ensure Chrome/Chromium is installed, check for sandbox issues on Linux

### Debug page shows 0 memories in graph
Fixed - was using `recall()` with empty keywords, now uses `get_all_memories()`
