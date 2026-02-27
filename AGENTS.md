# AGENTS.md

## Project overview

RayClaw is a Rust-based multi-channel agentic runtime. It connects to Telegram, Discord, Slack, Feishu/Lark, and a built-in Web UI through a unified agent engine, routing all conversations through a single provider-agnostic LLM layer with full tool execution.

Core capabilities:
- Multi-step tool-calling agent loop with session persistence
- Context compaction for long-running conversations
- Cron and one-shot background task scheduler
- Dual memory system: file-based (AGENTS.md) + structured (SQLite)
- Memory reflector with quality scoring and dedup
- Skill framework + MCP tool federation
- ACP integration for spawning external coding agents

## Tech stack

- Language: Rust (edition 2021)
- Async runtime: Tokio
- Telegram: teloxide
- Discord: serenity
- Web API/UI: axum + embedded React frontend (web/)
- Database: SQLite via rusqlite (bundled, WAL mode)
- LLM: provider abstraction — native Anthropic + OpenAI-compatible endpoints

## Source layout (`src/`)

- `main.rs` — CLI entry point: `start`, `setup`, `doctor`, `gateway`, `version`
- `runtime.rs` — AppState assembly, provider/tool init, channel boot, signal handling
- `agent_engine.rs` — shared agent loop (`process_with_agent`), system prompt builder, explicit-memory fast path, context compaction
- `llm.rs` — LLM provider implementations, streaming, response translation
- `llm_types.rs` — model/tool/message data types
- `channels/telegram.rs` — Telegram adapter (teloxide)
- `channels/discord.rs` — Discord adapter (serenity)
- `channels/slack.rs` — Slack adapter (Socket Mode WebSocket)
- `channels/feishu.rs` — Feishu/Lark adapter (WebSocket or webhook)
- `channels/delivery.rs` — cross-channel outbound helpers
- `channel.rs` — channel abstraction types
- `web.rs` — Web API routes, SSE stream, config/usage/memory endpoints
- `db.rs` — SQLite schema, migrations, chat/session/task/memory persistence
- `memory.rs` — file-memory manager (runtime/groups/.../AGENTS.md)
- `memory_quality.rs` — remember parser, normalization, quality rules
- `scheduler.rs` — scheduled-task runner + memory reflector loop
- `usage.rs` — token/cost/memory usage assembly
- `embedding.rs` — optional embedding providers (sqlite-vec)
- `skills.rs` — skill discovery and activation
- `builtin_skills.rs` — bundled skill materialization
- `mcp.rs` — MCP server/tool integration
- `acp.rs` — Agent Client Protocol (external coding agent lifecycle)
- `gateway.rs` — systemd/launchd service management
- `setup.rs` — interactive TUI setup wizard
- `doctor.rs` — environment diagnostics
- `tools/` — built-in tool implementations and ToolRegistry

## Tool system

`src/tools/mod.rs` defines:
- `Tool` trait: `name()`, `definition()`, `execute()`
- `ToolRegistry`: dispatch, auth/permission context injection, risk gate
- Sub-agent registry variant with restricted tool set

Regenerate docs artifacts:
```sh
node scripts/generate_docs_artifacts.mjs
```

## Agent loop

`process_with_agent` flow:
1. Explicit-memory fast path — `remember ...` / `记住...` writes structured memory directly
2. Load resumable session from `sessions` table, or rebuild from chat history
3. Assemble system prompt: identity (SOUL.md) + capabilities + memories + skills catalog
4. Compact context if session exceeds limits (summarize older messages, keep recent verbatim)
5. Call LLM provider with tool schemas
6. If `tool_use` → execute tool(s), append results, loop (up to max_tool_iterations)
7. If `end_turn` → persist session, return final text

## Memory architecture

Two layers:

1. **File memory** — Markdown notes
   - Global: `runtime/groups/AGENTS.md`
   - Per-chat: `runtime/groups/{chat_id}/AGENTS.md`

2. **Structured memory** — SQLite `memories` table
   - Fields: category, confidence, source, last_seen, archived
   - Explicit remember fast path with quality gate
   - Reflector extracts memories from conversation history
   - Dedup/supersede via `memory_supersede_edges` graph

Observability:
- `memory_reflector_runs` / `memory_injection_logs` tables
- `/api/usage` summary
- `/api/memory_observability` time-series API

## Database

`db.rs` manages:
- Schema creation + versioned migrations (`db_meta`, `schema_migrations`)
- Chat/message/session/task persistence
- Structured memory CRUD + archive/supersede
- Usage logging and memory observability queries

## Web/API

`web.rs` routes:
- `POST /api/send`, `POST /api/send_stream` — chat input
- `GET /api/stream` — SSE stream replay
- `GET /api/sessions`, `GET /api/history` — session management
- `POST /api/reset`, `POST /api/delete_session` — session control
- `GET /api/config`, `PUT /api/config` — runtime config
- `GET /api/usage` — token/cost report
- `GET /api/memory_observability` — memory trends

## Build and test

```sh
cargo build
cargo test
npm --prefix web run build
```

Docs drift guard:
```sh
node scripts/generate_docs_artifacts.mjs --check
```
