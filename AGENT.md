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
│   │   ├── mcp_client.rs    # MCP protocol client
│   │   └── mcp_bridge.rs    # MCP server management
│   ├── orchestrator/        # Multi-agent task orchestration
│   │   ├── mod.rs           # Orchestrator main
│   │   ├── types.rs         # Plan, TaskNode, AgentType, TaskSpecification
│   │   ├── task_queue.rs    # Task scheduling
│   │   ├── workers.rs       # Worker pool
│   │   ├── skill_factory.rs # Dynamic skill generation
│   │   ├── spec_extractor.rs # LLM-based task specification extraction
│   │   ├── validator.rs     # Output validation against specs
│   │   └── learning.rs      # Mistake context and correction prompts
│   ├── graph/
│   │   └── mistakes.rs      # Mistake storage and recall
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

MCP tools loaded from `mcp_servers.json` (includes Playwright for browser automation):
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

### 4. Browser Automation (Playwright MCP)

Browser automation is handled via Playwright MCP server (`@playwright/mcp`).

Actions available:
- `browser_navigate` - Go to URL
- `browser_click` - Click element by selector
- `browser_fill` - Fill form field
- `browser_snapshot` - Get page accessibility snapshot
- `browser_screenshot` - Capture page

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
- `stats` - Show graph statistics
- `mistakes` - Show all recorded mistakes from learning system
- `/multi <task>` - Execute task with multi-agent orchestrator
- `/learn <task>` - Execute task with validation and mistake recording
- Any other input - Send to single agent

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

## Self-Improvement System (Learning)

The learning system validates task outputs and records mistakes for future improvement.

### Flow

```
User Task → Spec Extraction → Execution → Validation
                                              ↓
                              [PASS]     [FAIL]
                                ↓          ↓
                             Done    Store Mistakes
                                          ↓
                                    Retry with Correction
                                          ↓
                                    Mark as Corrected
```

### Components

| Component | File | Purpose |
|-----------|------|---------|
| SpecExtractor | `orchestrator/spec_extractor.rs` | LLM-based extraction of numeric requirements and expected outputs |
| Validator | `orchestrator/validator.rs` | Compare actual output vs specification |
| LearningModule | `orchestrator/learning.rs` | Build mistake context for prompts |
| MistakeNode | `types.rs` + `graph/mistakes.rs` | Store/recall mistakes with similarity matching |

### MistakeNode Structure

```rust
struct MistakeNode {
    mistake_type: MistakeType,     // QuantityMismatch, MissingOutput, QualityIssue
    severity: Severity,            // Critical, Major, Minor
    description: String,           // "Only searched 2 sites instead of 3"
    prevention_strategy: String,   // "Verify all sites before starting"
    keywords: Vec<String>,         // For similarity matching
    task_fingerprint: String,      // Hash for task type matching
    was_corrected: bool,           // Corrected in retry?
}
```

### Usage

```
# Execute with validation and mistake recording
/learn search 3 e-commerce sites for headphones

# View recorded mistakes
mistakes
```

### Mistake Recall

When executing similar tasks, past mistakes are recalled and injected into the system prompt:

```
## LEARNED FROM PAST MISTAKES:
Apply these lessons to avoid repeating errors:

🚨 1. [QuantityMismatch] Only searched 2 sites instead of 3
   Prevention: Verify site count matches requirement before proceeding

⚠️ 2. [MissingOutput] Expected CSV file was not created
   Prevention: Always create required output files before completing task
```

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

## Logging

Logs are written to `/tmp/momo.log`. Tail in a separate terminal:
```bash
tail -f /tmp/momo.log
```

### Log Prefixes

The system logs operations with prefixes for easy identification:

**Memory System:**
- `🧠 [MEMORY]` - Memory extraction operations (storing new memories)
- `🔍 [RECALL]` - Memory recall operations (retrieving memories)
- `📚 [AGENT]` - Agent using memories for context
- `🧠 [BACKGROUND]` - Background memory extraction tasks

**Learning System (Self-Improvement):**
- `🧠 [LEARNING]` - Learning system operations (execute_with_learning)
- `📚 [MISTAKE]` - Mistake storage and recall
- `✅ [SPEC]` - Task specification extraction
- `🔍 [VALIDATE]` - Output validation against specifications

**Log examples:**
```
🧠 [LEARNING] Starting execute_with_learning for: "search 3 sites..."
✅ [SPEC] Extracted specification: 3 numeric requirements, 1 output
🔍 [VALIDATE] Validating output against spec...
🔍 [VALIDATE] Validation failed: 2 missing elements
📚 [MISTAKE] Recording mistake: "Only searched 2 sites instead of 3"
📚 [MISTAKE] Retrieved 3 relevant past mistakes for context
🧠 [LEARNING] Retry 1/3 with correction prompt...
```

**To set log level:**
```bash
# Default
RUST_LOG=info,momo=debug cargo run

# Verbose
RUST_LOG=debug cargo run

# Specific modules
RUST_LOG=momo::orchestrator=trace,momo::graph=debug cargo run
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
- `wasmtime` - WASM sandbox for skills
- `lbug` - Graph database
- `warp` - Debug web server
- `tracing` - Logging
- `regex` - Pattern matching for spec extraction

Browser automation is via Playwright MCP (`@playwright/mcp` npm package).

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

### Playwright MCP not working
Ensure Playwright is installed: `npx playwright install`

### Debug page shows 0 memories in graph
Fixed - was using `recall()` with empty keywords, now uses `get_all_memories()`

### Learning system not recording mistakes
Use `/learn <task>` prefix to enable validation. Check logs with `tail -f /tmp/momo.log`
