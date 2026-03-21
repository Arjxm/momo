# MOMO - Memory-Oriented Model Orchestrator

An autonomous AI agent built in Rust with persistent graph-based memory, multi-provider LLM support, and extensible tool architecture. MOMO learns from every interaction, building a knowledge graph that makes it smarter over time.

> **Status:** Active Development - Core memory and agent loop functional, orchestrator in progress

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         MOMO Agent                              │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │   Providers  │  │  Tool Layer  │  │    Memory System     │   │
│  │  ──────────  │  │  ──────────  │  │    ──────────────    │   │
│  │  • Anthropic │  │  • Native    │  │  • GraphBrain (lbug) │   │
│  │  • OpenAI    │  │  • MCP       │  │  • Smart Recall      │   │
│  │  • Ollama    │  │  • Skills    │  │  • Deduplication     │   │
│  │  • Gemini    │  │  • Browser   │  │  • Contradiction     │   │
│  └──────────────┘  └──────────────┘  └──────────────────────┘   │
├─────────────────────────────────────────────────────────────────┤
│                      Orchestrator (WIP)                         │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │   Planner    │  │  Task Queue  │  │      Workers         │   │
│  │  (Decompose) │  │  (Priority)  │  │  (Parallel Exec)     │   │
│  └──────────────┘  └──────────────┘  └──────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

## Memory System

MOMO's core differentiator is its persistent, graph-based memory that learns from every conversation.

### How Memory Works

1. **Extraction**: After each interaction, an LLM extracts facts and preferences from the conversation
2. **Deduplication**: SHA256 fingerprints prevent storing duplicate memories
3. **Contradiction Detection**: New memories are checked against existing ones for conflicts
4. **Smart Recall**: Multi-factor scoring retrieves the most relevant memories:
   - **Keyword Match** (30%): How well content matches the query
   - **Recency** (30%): Exponential decay with 7-day half-life
   - **Importance** (25%): Extracted importance from 0.0-1.0
   - **Frequency** (15%): How often the memory has been accessed

### Memory Types

| Type | Description | Example |
|------|-------------|---------|
| `fact` | Information about the user or context | "User works at Acme Corp" |
| `preference` | User's explicit preferences | "User prefers concise responses" |
| `episode_summary` | Condensed interaction summaries | "Discussed project architecture" |

### Graph Schema

```
User ──PREFERS──> Memory ──ABOUT──> Topic
  │                  │
  └──INTERACTED──> Episode ──LEARNED_FROM──> Memory
                     │
                  Task ──PERFORMED──> Operation ──EXECUTED_BY──> Tool
                    │
                    └──DECOMPOSED_INTO──> Task (subtasks)
```

## Tool Architecture

MOMO supports four types of tools:

| Type | Description | Hot-Reload |
|------|-------------|------------|
| **Native** | Built-in Rust tools (shell, web, etc.) | No |
| **MCP** | Model Context Protocol servers | Yes |
| **Skills** | User-added Python/JS/WASM scripts | Yes |
| **Browser** | Chromium automation (navigate, click, fill) | No |

### Adding Custom Skills

Drop a folder in `skills/` with either:

**Executable Skill:**
```
skills/my_tool/
├── SKILL.toml    # Manifest with name, description, schema
└── main.py       # Implementation (stdin/stdout JSON)
```

**Knowledge Skill:**
```
skills/my_docs/
└── skill.md      # Markdown with YAML frontmatter
```

Knowledge skills are automatically injected into context when queries match their keywords.

## Setup

1. Clone and configure:
   ```bash
   cp .env.example .env
   # Add your API keys to .env
   ```

2. Configure providers in `provider_config.json`:
   ```json
   {
     "provider_type": "anthropic",
     "api_key": "sk-ant-...",
     "model": "claude-sonnet-4-20250514",
     "max_tokens": 4096
   }
   ```

3. Build and run:
   ```bash
   cargo build --release
   ./target/release/agent-brain
   ```

## Configuration

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `AGENT_MAX_ITERATIONS` | Max ReAct loop iterations | 50 |
| `ANTHROPIC_API_KEY` | Anthropic API key | - |
| `OPENAI_API_KEY` | OpenAI API key | - |

### MCP Servers

Configure in `mcp_servers.json`:
```json
{
  "servers": [
    {
      "name": "filesystem",
      "command": "npx",
      "args": ["-y", "@anthropic/mcp-server-filesystem", "/path/to/dir"]
    }
  ]
}
```

## Development Status

### Implemented

- [x] Multi-provider LLM abstraction (Anthropic, OpenAI, Ollama, Gemini)
- [x] Graph-based persistent memory with LadybugDB
- [x] Smart memory recall with multi-factor scoring
- [x] Memory deduplication and contradiction detection
- [x] MCP server integration
- [x] Custom skills system (executable + knowledge)
- [x] Browser automation with Chromium
- [x] Operation tracking and provenance
- [x] Session conversation history

### In Progress

- [ ] Task orchestrator with parallel workers
- [ ] Autonomous task decomposition
- [ ] Plan execution and monitoring

### Roadmap: Self-Improving Agents

The next major milestone is enabling MOMO to improve itself:

1. **Tool Discovery**: Automatically discover and integrate new tools based on task requirements
2. **Skill Learning**: Generate new skills from successful interaction patterns
3. **Self-Reflection**: Analyze failed tasks and adjust strategies
4. **Memory Consolidation**: Compress and merge related memories over time
5. **Goal Autonomy**: Break down high-level goals into executable subtasks
6. **Error Recovery**: Learn from failures and develop fallback strategies

## Project Structure

```
src/
├── agent.rs          # Core ReAct loop
├── providers/        # LLM provider implementations
├── graph/
│   ├── mod.rs        # GraphBrain - unified graph interface
│   ├── memory.rs     # Memory extraction and storage
│   └── schema.rs     # Graph schema definitions
├── tools/
│   ├── mod.rs        # Tool registry
│   ├── browser.rs    # Chromium automation
│   └── mcp_bridge.rs # MCP protocol client
├── skills/
│   ├── loader.rs     # Skill discovery and loading
│   ├── registry.rs   # Skill management
│   └── sandbox.rs    # WASM sandbox
└── orchestrator/     # Task decomposition (WIP)
    ├── planner.rs    # Task planning
    ├── task_queue.rs # Priority queue
    └── workers.rs    # Parallel execution
```

## License

MIT
