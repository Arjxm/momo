# Agent Brain

A command-line AI agent in Rust that uses Claude Sonnet to reason and execute tools using the ReAct pattern.

## Features

- **Calculator**: Evaluate mathematical expressions with support for +, -, *, /, parentheses, decimals, and negatives
- **arXiv Search**: Search and retrieve academic papers from arXiv
- **Web Fetch**: Fetch and extract text content from web pages

## Setup

1. Copy `.env.example` to `.env` and add your Anthropic API key:
   ```bash
   cp .env.example .env
   ```

2. Build the project:
   ```bash
   cargo build --release
   ```

3. Run the agent:
   ```bash
   ./target/release/agent-brain
   ```

## Usage

The agent runs as an interactive REPL. Type your questions or commands, and the agent will use Claude to reason through them, calling tools as needed.

Example prompts:
- `What is 42 * 17 + (100 / 4)?` - Uses calculator tool
- `Find the latest 3 papers about AI agents on arXiv` - Uses arXiv search
- `Fetch the content from https://example.com` - Uses web fetch

Type `quit` or `exit` to leave the REPL.

## Architecture

The agent follows the ReAct (Reason + Act) pattern:
1. Send user message to Claude with available tools
2. If Claude requests tool use, execute the tools and return results
3. Repeat until Claude provides a final answer
4. Display the response to the user

## License

MIT
