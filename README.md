# Agent Brain

A command-line AI agent in Rust that uses Claude Sonnet to reason and execute tools using the ReAct pattern.

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


Type `quit` or `exit` to leave the REPL.

## Architecture

The agent follows the ReAct (Reason + Act) pattern:
1. Send user message to Claude with available tools
2. If Claude requests tool use, execute the tools and return results
3. Repeat until Claude provides a final answer
4. Display the response to the user

