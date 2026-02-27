# RayClaw Developer Guide

## Quick start

```sh
git clone <repo-url>
cd rayclaw
cp rayclaw.config.example.yaml rayclaw.config.yaml
# Edit rayclaw.config.yaml with your credentials
cargo run -- start
```

## Prerequisites

- Rust 1.70+ (2021 edition)
- At least one enabled channel adapter (Telegram bot token from @BotFather, Discord bot token from Discord Developer Portal, Slack app/bot tokens, Feishu/Lark app credentials, or Web UI)
- An Anthropic API key

No other external dependencies. SQLite is bundled via `rusqlite`.

## Project structure

```
src/
    main.rs              # Entry point. Parses CLI args, initializes subsystems, starts platform runtimes.
    config.rs            # Config struct. All settings loaded from rayclaw.config.yaml.
    error.rs             # RayClawError enum (thiserror). Centralized error types.
    telegram.rs          # Core orchestration:
                         #   - Telegram message handler
                         #   - Agentic tool-use loop (process_with_claude)
                         #   - Session resume (load/save full message state)
                         #   - Context compaction (summarize old messages)
                         #   - Continuous typing indicator
                         #   - Group chat catch-up logic
                         #   - Response splitting
    discord.rs           # Discord message handler (serenity gateway), reuses process_with_claude
    channels/slack.rs    # Slack adapter (Socket Mode WebSocket)
    channels/feishu.rs   # Feishu/Lark adapter (WebSocket long connection or webhook)
    claude.rs            # Anthropic Messages API client:
                         #   - Request/response types with serde
                         #   - HTTP calls with retry on 429
                         #   - Content block enums (Text, ToolUse, ToolResult)
    db.rs                # SQLite database:
                         #   - Schema creation (chats, messages, scheduled_tasks, sessions)
                         #   - Message storage and retrieval
                         #   - Session save/load/delete for resume
                         #   - Scheduled task CRUD
                         #   - Catch-up query (messages since last bot response)
    memory.rs            # MemoryManager:
                         #   - Reads/writes CLAUDE.md files (global + per-chat)
                         #   - Builds memory context for system prompts
    scheduler.rs         # Background scheduler:
                         #   - Spawns a tokio task that polls every 60s
                         #   - Finds due tasks, runs agent loop, sends results
                         #   - Computes next run time for cron tasks
    tools/
        mod.rs           # Tool trait, ToolRegistry, ToolResult.
                         # Registry takes data_dir, Bot, Arc<Database>.
        bash.rs          # Shell command execution (tokio::process::Command)
        read_file.rs     # File reading with line numbers, offset/limit
        write_file.rs    # File creation/overwrite, auto-creates directories
        edit_file.rs     # Find/replace editing, validates uniqueness
        glob.rs          # File pattern matching (glob crate)
        grep.rs          # Recursive regex search, directory traversal
        memory.rs        # read_memory / write_memory tools
        web_search.rs    # DuckDuckGo HTML search, regex result parsing
        web_fetch.rs     # URL fetching, HTML tag stripping, 20KB limit
        send_message.rs  # Mid-conversation messaging (Telegram/Discord)
        schedule.rs      # 5 scheduling tools (create/list/pause/resume/cancel)
        sub_agent.rs     # Sub-agent tool with restricted tool registry (9 tools)
```

## Architecture overview

### Data flow

```
Platform message (via adapter)
       |
       v
    Store in SQLite (message + chat metadata)
       |
       v
    Determine response: private=always, group=@mention only
       |
       v
    Start typing indicator (tokio::spawn, every 4s)
       |
       v
    Load session or history:
       - Try loading saved session (full message state with tool blocks)
       - If session exists: append new user messages since last save
       - If no session: fall back to DB history
           - Private: last N messages
           - Group: all messages since last bot response (catch-up)
       |
       v
    Build system prompt (bot identity + memory context + chat_id)
       |
       v
    Compact if needed (messages > max_session_messages):
       - Summarize old messages via Claude
       - Keep recent messages verbatim
       |
       v
    Agentic loop (up to max_tool_iterations):
       1. Call Claude API with messages + tool definitions
       2. If stop_reason == "tool_use" -> execute tools -> append results -> loop
       3. If stop_reason == "end_turn" -> extract text -> return
       |
       v
    Strip image base64 data, save session to SQLite
       |
       v
    Abort typing indicator
       |
       v
    Send response (split at channel limits: Telegram 4096 / Discord 2000 / Slack 4000 / Feishu 4000)
       |
       v
    Store bot response in SQLite
```

The same core loop is reused across adapters. Adding a new platform should primarily require a new ingress/egress adapter that maps platform events into the shared `process_with_claude` flow.

### Key types

| Type | Location | Description |
|------|----------|-------------|
| `AppState` | `telegram.rs` | Shared state: config, bot, db, memory, claude client, tool registry |
| `Database` | `db.rs` | SQLite wrapper with `Mutex<Connection>` |
| `ToolRegistry` | `tools/mod.rs` | Holds all `Box<dyn Tool>`, dispatches by name |
| `Tool` trait | `tools/mod.rs` | `name()`, `definition()`, `execute()` |
| `ClaudeClient` | `claude.rs` | HTTP client for Anthropic API |
| `MemoryManager` | `memory.rs` | CLAUDE.md file reader/writer |

### Shared state

`AppState` is wrapped in `Arc` and shared:
- Telegram handler has `Arc<AppState>` via dptree dependencies
- Discord handler has `Arc<AppState>` via serenity event handler state
- Scheduler gets `Arc<AppState>` at spawn time
- Tools that need `Bot` or `Database` hold their own clones/arcs (passed at construction)

### Multi-chat permission model

- `control_chat_ids` in `rayclaw.config.yaml` defines privileged chats.
- Tool execution receives trusted caller context from `process_with_claude` (not from model-provided args).
- Non-control chats can only operate on their own `chat_id`.
- Control chats can perform cross-chat actions.
- `write_memory` with `scope: "global"` is restricted to control chats.
- Enforcement currently applies to `send_message`, scheduler tools, `export_chat`, `todo_*`, and chat-scoped memory operations.

### Database tables

**chats:**
| Column | Type | Description |
|--------|------|-------------|
| chat_id | INTEGER PK | Channel-scoped chat ID |
| chat_title | TEXT | Chat title (nullable) |
| chat_type | TEXT | "private" or "group" |
| last_message_time | TEXT | ISO 8601 timestamp |

**messages:**
| Column | Type | Description |
|--------|------|-------------|
| id | TEXT | Message ID (PK with chat_id) |
| chat_id | INTEGER | Channel-scoped chat ID |
| sender_name | TEXT | Username or first name |
| content | TEXT | Message text |
| is_from_bot | INTEGER | 0 or 1 |
| timestamp | TEXT | ISO 8601 timestamp |

**scheduled_tasks:**
| Column | Type | Description |
|--------|------|-------------|
| id | INTEGER PK | Auto-increment task ID |
| chat_id | INTEGER | Target chat for results |
| prompt | TEXT | Instruction to execute |
| schedule_type | TEXT | "cron" or "once" |
| schedule_value | TEXT | Cron expression or ISO timestamp |
| next_run | TEXT | Next scheduled execution time |
| last_run | TEXT | Last execution time (nullable) |
| status | TEXT | "active", "paused", "completed", "cancelled" |
| created_at | TEXT | Creation timestamp |

**sessions:**
| Column | Type | Description |
|--------|------|-------------|
| chat_id | INTEGER PK | Channel-scoped chat ID |
| messages_json | TEXT | Serialized Vec<Message> JSON (full conversation state) |
| updated_at | TEXT | ISO 8601 timestamp of last save |

**task_run_logs:**
| Column | Type | Description |
|--------|------|-------------|
| id | INTEGER PK | Auto-increment log ID |
| task_id | INTEGER | Associated scheduled task |
| chat_id | INTEGER | Chat the task ran in |
| started_at | TEXT | Run start timestamp |
| finished_at | TEXT | Run end timestamp |
| duration_ms | INTEGER | Run duration in milliseconds |
| success | INTEGER | 0 or 1 |
| result_summary | TEXT | Summary of run result (nullable) |

## Adding a new tool

1. Create `src/tools/my_tool.rs`:

```rust
use async_trait::async_trait;
use serde_json::json;
use super::{schema_object, Tool, ToolResult};
use crate::claude::ToolDefinition;

pub struct MyTool;

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str {
        "my_tool"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "my_tool".into(),
            description: "What this tool does".into(),
            input_schema: schema_object(
                json!({
                    "param1": {
                        "type": "string",
                        "description": "Description of param1"
                    }
                }),
                &["param1"],
            ),
        }
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let param1 = match input.get("param1").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::error("Missing param1".into()),
        };
        // Do work...
        ToolResult::success(format!("Result: {param1}"))
    }
}
```

2. Add `pub mod my_tool;` to `src/tools/mod.rs`

3. Register in `ToolRegistry::new()`:
```rust
Box::new(my_tool::MyTool),
```

If your tool needs shared state (like `Bot` or `Arc<Database>`), add a constructor:
```rust
pub struct MyTool {
    db: Arc<Database>,
}

impl MyTool {
    pub fn new(db: Arc<Database>) -> Self {
        MyTool { db }
    }
}
```

And pass it in `ToolRegistry::new()`:
```rust
Box::new(my_tool::MyTool::new(db.clone())),
```

## Adding a new platform adapter

The core agent flow is already shared. New platform support should be implemented as an adapter around that core:

1. Add a new runtime module (for example `src/<platform>.rs`) that listens to platform events.
2. Normalize incoming platform messages into the canonical fields used by persistence and `process_with_claude`:
   - stable `chat_id`
   - `chat_type` (`private`/`group`-like semantics)
   - sender display name
   - text/media blocks
3. Reuse `process_with_claude(state, chat_id, sender, chat_type, override_prompt)` for inference/tool loop/session handling.
4. Implement outbound reply sending with:
   - platform length splitting policy
   - mention semantics for group/server channels
   - attachment support parity with `send_message` where applicable
5. Keep chat identity stable across restarts so `sessions`, `messages`, scheduler, and memory scopes remain consistent.
6. Enforce existing authorization boundaries (for example `control_chat_ids`) in any platform-specific entry points.
7. Add end-to-end tests to `TEST.md` mirroring existing platform suites (DM/private, group mention, reset, limits, failure handling).

## Scheduler internals

The scheduler is a `tokio::spawn` task started in `run_bot()`. Every 60 seconds it:

1. Queries `scheduled_tasks` for rows where `status = 'active' AND next_run <= NOW()`
2. For each due task, calls `process_with_claude()` with `override_prompt = Some(task.prompt)`
3. Sends the agent's response to the task's `chat_id`
4. For cron tasks: computes next run from the cron expression and updates the row
5. For one-shot tasks: sets `status = 'completed'`

Cron expressions use the `cron` crate's 6-field format: `sec min hour dom month dow`.

## Debugging

```sh
# Verbose logging
RUST_LOG=debug cargo run -- start

# Just rayclaw logs
RUST_LOG=rayclaw=debug cargo run -- start

# Check database directly
sqlite3 rayclaw.data/runtime/rayclaw.db
sqlite> SELECT * FROM messages ORDER BY timestamp DESC LIMIT 10;
sqlite> SELECT * FROM scheduled_tasks;
sqlite> SELECT * FROM chats;
```

## Common tasks

| Task | How |
|------|-----|
| Change the model | Set `model: "claude-sonnet-4-20250514"` in `rayclaw.config.yaml` |
| Increase context window | Set `max_history_messages: 100` in `rayclaw.config.yaml` (uses more tokens) |
| Increase tool iterations | Set `max_tool_iterations: 200` in `rayclaw.config.yaml` |
| Reset memory | Delete files under `rayclaw.data/runtime/groups/` |
| Reset all data | Delete the `rayclaw.data/` directory |
| Tune compaction threshold | Set `max_session_messages: 60` in `rayclaw.config.yaml` (higher = more context before compaction) |
| Keep more recent messages | Set `compact_keep_recent: 30` in `rayclaw.config.yaml` (more recent messages kept verbatim) |
| Reset a chat session | Send `/reset` in the chat, or: `sqlite3 rayclaw.data/runtime/rayclaw.db "DELETE FROM sessions WHERE chat_id=XXXX;"` |
| Cancel all scheduled tasks | `sqlite3 rayclaw.data/runtime/rayclaw.db "UPDATE scheduled_tasks SET status='cancelled' WHERE status='active';"` |

## Build

```sh
cargo build              # Dev build (fast compile, slow runtime)
cargo build --release    # Release build (slow compile, fast runtime)
cargo run -- start       # Run dev build
cargo run -- help        # Show CLI help
```

The release binary is fully self-contained -- no runtime dependencies, no database server, no config files beyond `rayclaw.config.yaml`.

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| teloxide | 0.17 | Telegram Bot API |
| serenity | 0.12 | Discord Gateway/API |
| tokio | 1 | Async runtime |
| reqwest | 0.12 | HTTP client (Anthropic API, web fetch/search) |
| rusqlite | 0.32 | SQLite (bundled) |
| serde / serde_json | 1 | Serialization |
| async-trait | 0.1 | Async trait support |
| chrono | 0.4 | Date/time handling |
| cron | 0.13 | Cron expression parsing |
| urlencoding | 2 | URL encoding for web search |
| regex | 1 | Regex for grep tool and HTML parsing |
| glob | 0.3 | File pattern matching |
| uuid | 1 | Message ID generation |
| thiserror | 2 | Error derive macro |
| anyhow | 1 | Error propagation |
| tracing | 0.1 | Logging |
