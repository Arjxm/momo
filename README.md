# MOMO - Memory-Oriented Model Orchestrator

[![Rust](https://img.shields.io/badge/rust-%23000000.svg?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg?style=for-the-badge)](https://opensource.org/licenses/MIT)
[![GitHub Stars](https://img.shields.io/github/stars/Arjxm/momo?style=for-the-badge)](https://github.com/Arjxm/momo/stargazers)

An autonomous AI agent built in Rust with **persistent graph-based memory**, multi-provider LLM support, and extensible tool architecture. MOMO learns from every interaction, building a knowledge graph that makes it smarter over time.

**Unlike stateless AI agents, MOMO remembers.** It extracts facts and preferences from conversations, detects contradictions, and uses smart recall to surface relevant memories when you need them.

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
- [x] Browser automation via Playwright MCP
- [x] Operation tracking and provenance
- [x] Session conversation history

### In Progress

- [ ] Task orchestrator with parallel workers
- [ ] Autonomous task decomposition
- [ ] Plan execution and monitoring
- [ ] Self-improvement loop (validation, mistake storage, learning)

### Self-Improvement System (Under Development)

The self-improvement system enables MOMO to learn from mistakes:

| Component | Status | Description |
|-----------|--------|-------------|
| Specification Extraction | Implemented | LLM extracts numeric requirements and expected outputs from tasks |
| Output Validation | Implemented | Compares actual output against extracted specification |
| Mistake Storage | Implemented | Records failures with prevention strategies in graph database |
| Mistake Recall | Implemented | Retrieves relevant past mistakes for similar tasks |
| Correction Prompts | Implemented | Generates retry prompts with mistake context |
| Auto-Retry Loop | Implemented | Retries failed tasks with correction guidance |

**Usage:** Prefix tasks with `/learn` to enable validation:
```
/learn search 3 e-commerce sites for headphones and save to CSV
```

**View mistakes:** Type `mistakes` to see recorded failures.

### Roadmap

1. **Tool Discovery**: Automatically discover and integrate new tools based on task requirements
2. **Skill Learning**: Generate new skills from successful interaction patterns
3. **Memory Consolidation**: Compress and merge related memories over time
4. **Goal Autonomy**: Break down high-level goals into executable subtasks

## Project Structure

```
src/
├── agent.rs          # Core ReAct loop
├── providers/        # LLM provider implementations
├── graph/
│   ├── mod.rs        # GraphBrain - unified graph interface
│   ├── memory.rs     # Memory extraction and storage
│   ├── mistakes.rs   # Mistake storage and recall
│   └── schema.rs     # Graph schema definitions
├── tools/
│   ├── mod.rs        # Tool registry
│   └── mcp_bridge.rs # MCP protocol client (includes Playwright)
├── skills/
│   ├── loader.rs     # Skill discovery and loading
│   ├── registry.rs   # Skill management
│   └── sandbox.rs    # WASM sandbox
└── orchestrator/
    ├── planner.rs        # Task planning
    ├── task_queue.rs     # Priority queue
    ├── workers.rs        # Parallel execution
    ├── spec_extractor.rs # Task specification extraction
    ├── validator.rs      # Output validation
    └── learning.rs       # Mistake context builder
```

## License

MIT
