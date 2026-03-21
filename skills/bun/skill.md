---
name: bun
description: Build fast applications with Bun JavaScript runtime. Use when creating Bun projects, using Bun APIs, bundling, testing, or optimizing Node.js alternatives. Triggers on Bun, Bun runtime, bun.sh, bunx, Bun serve, Bun test, JavaScript runtime.
version: 1
topics:
  - javascript
  - typescript
  - runtime
  - bundler
  - testing
---

# Bun - The Fast JavaScript Runtime

Build and run JavaScript/TypeScript applications with Bun's all-in-one toolkit.

## Quick Start

```bash
# Install Bun
curl -fsSL https://bun.sh/install | bash

# Create new project
bun init

# Run a file
bun run index.ts

# Install packages (faster than npm)
bun install
```

## HTTP Server

```typescript
const server = Bun.serve({
  port: 3000,
  fetch(req) {
    const url = new URL(req.url);

    if (url.pathname === "/") {
      return new Response("Hello, Bun!");
    }

    if (url.pathname === "/json") {
      return Response.json({ message: "Hello" });
    }

    return new Response("Not Found", { status: 404 });
  },
});

console.log(`Server running at http://localhost:${server.port}`);
```

## File I/O

```typescript
// Read file
const content = await Bun.file("data.txt").text();

// Write file
await Bun.write("output.txt", "Hello, World!");

// Read JSON
const config = await Bun.file("config.json").json();
```

## Package Management

```bash
# Install dependencies
bun install

# Add a package
bun add express

# Add dev dependency
bun add -d typescript

# Remove package
bun remove express

# Run scripts from package.json
bun run dev
bun run build
```

## Testing

```typescript
// test.ts
import { expect, test, describe } from "bun:test";

describe("math", () => {
  test("addition", () => {
    expect(2 + 2).toBe(4);
  });

  test("async test", async () => {
    const result = await Promise.resolve(42);
    expect(result).toBe(42);
  });
});
```

```bash
# Run tests
bun test

# Watch mode
bun test --watch

# Run specific file
bun test math.test.ts
```

## Bundling

```typescript
// Build for production
await Bun.build({
  entrypoints: ["./src/index.ts"],
  outdir: "./dist",
  minify: true,
  target: "browser",
});
```

```bash
# CLI bundling
bun build ./src/index.ts --outdir ./dist --minify
```

## Environment Variables

```typescript
// Access env vars
const apiKey = Bun.env.API_KEY;
const port = Bun.env.PORT || "3000";

// .env files are loaded automatically
```

## SQLite (Built-in)

```typescript
import { Database } from "bun:sqlite";

const db = new Database("mydb.sqlite");
db.run("CREATE TABLE IF NOT EXISTS users (id INTEGER PRIMARY KEY, name TEXT)");

// Insert
db.run("INSERT INTO users (name) VALUES (?)", ["Alice"]);

// Query
const users = db.query("SELECT * FROM users").all();
```

## WebSocket Server

```typescript
Bun.serve({
  fetch(req, server) {
    if (server.upgrade(req)) {
      return; // Upgraded to WebSocket
    }
    return new Response("Upgrade failed", { status: 500 });
  },
  websocket: {
    message(ws, message) {
      ws.send(`Echo: ${message}`);
    },
    open(ws) {
      console.log("Client connected");
    },
    close(ws) {
      console.log("Client disconnected");
    },
  },
});
```

## Key Differences from Node.js

- **Faster**: Up to 4x faster than Node.js for many workloads
- **Built-in bundler**: No need for webpack/esbuild
- **Native TypeScript**: Run .ts files directly without compilation
- **Built-in test runner**: `bun:test` module
- **Built-in SQLite**: `bun:sqlite` module
- **Web-standard APIs**: fetch, Request, Response built-in
- **Package manager**: `bun install` is faster than npm/yarn
