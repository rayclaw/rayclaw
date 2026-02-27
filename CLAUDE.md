# RayClaw

RayClaw is a multi-channel agentic AI runtime written in Rust. It connects to Telegram, Discord, Slack, Feishu/Lark, and a built-in Web UI through a unified agent engine. Every channel adapter routes conversations through the same tool-calling loop, LLM abstraction, and persistence layer. The project focuses on reliable tool execution, long-running session continuity, and cross-platform parity.

## Tech stack

Rust 2021 edition. Tokio async runtime. teloxide 0.17 (Telegram), serenity 0.12 (Discord), axum 0.7 (Web API + embedded React UI). LLM calls via reqwest — native Anthropic Messages API plus OpenAI-compatible endpoints. SQLite (rusqlite, bundled) with WAL mode. cron crate for scheduling.

## Directory overview

- `src/` — Rust source for the single binary
- `web/` — Embedded Web UI (React + Vite). Built to `web/dist/` and included in the binary via `include_dir!`. Serves the chat interface and settings panel at runtime.

## Source layout

| File | Role |
|------|------|
| `src/main.rs` | CLI entry: `start`, `setup`, `doctor`, `gateway`, `version` |
| `src/runtime.rs` | AppState wiring, channel boot, signal handling |
| `src/agent_engine.rs` | Shared agent loop, system prompt builder, context compaction |
| `src/llm.rs` | Provider abstraction: Anthropic native + OpenAI-compatible |
| `src/llm_types.rs` | Message, tool, and content-block DTOs |
| `src/config.rs` | YAML config loading and defaults |
| `src/error.rs` | Error enum (thiserror) |
| `src/db.rs` | SQLite schema, migrations, all persistence |
| `src/memory.rs` | File-based memory (AGENTS.md per chat / global) |
| `src/memory_quality.rs` | Remember parser, quality rules, dedup heuristics |
| `src/scheduler.rs` | Background task runner (60s poll) + memory reflector |
| `src/channels/telegram.rs` | Telegram adapter (teloxide dispatcher) |
| `src/channels/discord.rs` | Discord adapter (serenity gateway) |
| `src/channels/slack.rs` | Slack adapter (Socket Mode WebSocket) |
| `src/channels/feishu.rs` | Feishu/Lark adapter (WebSocket or webhook) |
| `src/channels/delivery.rs` | Cross-channel outbound helpers |
| `src/web.rs` | Web API routes, SSE stream, embedded React UI |
| `src/acp.rs` | ACP manager — external coding agents via JSON-RPC/stdio |
| `src/skills.rs` | Skill discovery and activation |
| `src/mcp.rs` | MCP server/tool federation |
| `src/tools/mod.rs` | Tool trait, ToolRegistry, sub-agent variant |
| `src/tools/bash.rs` | Shell execution |
| `src/tools/read_file.rs` / `write_file.rs` / `edit_file.rs` | File operations (path-guarded) |
| `src/tools/glob.rs` / `grep.rs` | File and content search (path-guarded) |
| `src/tools/memory.rs` | read_memory / write_memory |
| `src/tools/structured_memory.rs` | SQLite-backed structured memory |
| `src/tools/web_search.rs` | DuckDuckGo search |
| `src/tools/web_fetch.rs` | URL fetching with HTML→text |
| `src/tools/browser.rs` | Headless browser (agent-browser wrapper) |
| `src/tools/send_message.rs` | Mid-conversation messaging (all channels) |
| `src/tools/schedule.rs` | 5 scheduling tools |
| `src/tools/sub_agent.rs` | Sub-agent with restricted tool set |
| `src/tools/todo.rs` | Task plan tracking (todo_read / todo_write) |
| `src/tools/path_guard.rs` | Sensitive path blocklist |

## Key patterns

- **Agent loop** (`agent_engine.rs:process_with_agent`): call LLM → if tool_use → execute → loop (up to `max_tool_iterations`). On end_turn → persist session → return text.
- **Session resume**: full `Vec<Message>` (including tool_use/tool_result) persisted in `sessions` table. Next message loads the session and appends. `/reset` clears it.
- **Context compaction**: when messages exceed `max_session_messages`, older messages are summarized by the LLM, replaced with a compact summary, and recent messages are kept verbatim.
- **Sub-agent**: spawns a parallel agent loop with a restricted tool set (no send_message, write_memory, schedule, or recursive sub_agent).
- **Tool trait**: `name()`, `definition()` (JSON Schema), `execute(Value) -> ToolResult`.
- **Shared state**: `AppState` behind `Arc`, tools hold references to `Database`, channel adapters, etc.
- **Group catch-up**: `db.get_messages_since_last_bot_response()` loads all messages since the bot's last reply in a group.
- **Scheduler**: `tokio::spawn` loop polls DB every 60s for due tasks, runs them through the agent loop.
- **Typing indicator**: spawned task sends typing action every 4s, aborted when the response is ready.
- **Path guard**: file tools block access to sensitive paths (.ssh, .aws, .env, credentials, etc.).
- **SOUL.md**: optional personality file injected as `<soul>` XML in the system prompt. Load order: `soul_path` config → `<data_dir>/SOUL.md` → `./SOUL.md`. Per-chat overrides at `<data_dir>/runtime/groups/<chat_id>/SOUL.md`.
- **ACP**: `src/acp.rs` manages external coding agents (Claude Code, etc.) over JSON-RPC/stdio. Users control sessions via `#new <agent>`, `#end`, `#agents`, `#sessions`, `#help`.

## Build & run

```sh
cargo build
cargo run -- start    # requires rayclaw.config.yaml with credentials + at least one channel
cargo run -- setup    # interactive wizard to create the config
cargo run -- help
```

## Configuration

RayClaw reads `rayclaw.config.yaml` (or `.yml`). Override with `RAYCLAW_CONFIG` env var. See `rayclaw.config.example.yaml` for all fields.

## Adding a tool

1. Create `src/tools/my_tool.rs` implementing the `Tool` trait
2. Add `pub mod my_tool;` to `src/tools/mod.rs`
3. Register in `ToolRegistry::new()` with `Box::new(MyTool::new(...))`

## Database

Core tables: `chats`, `messages`, `scheduled_tasks`, `sessions`, `memories`, `llm_usage_logs`. SQLite with WAL mode. Schema versioned via `db_meta` + `schema_migrations`. Access through `Mutex<Connection>` in `Database`, shared as `Arc<Database>`.

## Conventions

- Timestamps: ISO 8601 / RFC 3339 strings throughout
- Cron: 6-field format (sec min hour dom month dow)
- All chat messages are stored regardless of whether the bot responds
- In groups, the bot only responds to @mentions
- Consecutive same-role messages are merged before LLM calls
- Long responses are split at newline boundaries: 4096 chars (Telegram), 2000 (Discord), 4000 (Slack/Feishu)
