# RayClaw ACP Enhancement Work Plan

> Reference: AWS ACP Bridge (github.com/aws-samples/sample-acp-bridge)
> Date: 2026-03-18

---

## Current State Summary

RayClaw's ACP implementation (`src/acp.rs`, ~1670 lines) covers:
- ACP JSON-RPC 2.0 over stdio transport
- `initialize` / `session/new` / `session/prompt` / `session/end` / `shutdown` lifecycle
- `prompt_streaming` with notification parsing (message chunks, thought chunks, tool calls, tool updates, plans)
- Permission auto-approve (`session/request_permission` handling)
- `AcpManager` with HashMap-based session store, chat-to-session binding
- Config via `acp.json` with `npx` / `binary` / `uvx` launch modes

Key gaps vs ACP Bridge: no concurrency limits, no async job mode, no stuck detection, no session auto-recovery, no PTY fallback, no HTTP API.

---

## Phase 0: Process Pool & Resource Guard

**Goal**: Prevent unbounded subprocess spawning in high-concurrency or adversarial scenarios.

**Priority**: P0 | **Effort**: Small | **Risk**: Low

### Tasks

| # | Task | File | Details |
|---|------|------|---------|
| 0.1 | Add pool config fields to `AcpConfig` | `src/acp.rs` | `max_sessions: usize` (default 20), `max_per_agent: usize` (default 10) |
| 0.2 | Enforce limits in `AcpManager::new_session` | `src/acp.rs` | Before spawning, count total sessions and per-agent sessions. Return descriptive error if limit reached. |
| 0.3 | Add tests for limit enforcement | `src/acp.rs` | Unit tests with mock config verifying rejection at capacity. |

### Implementation Notes

```rust
// In AcpConfig:
#[serde(default = "default_max_sessions")]
pub max_sessions: usize,          // default 20

#[serde(default = "default_max_per_agent")]
pub max_per_agent: usize,         // default 10

// In AcpManager::new_session, before spawn:
let sessions = self.sessions.read().await;
if sessions.len() >= self.config.max_sessions {
    return Err(format!("ACP session limit reached ({}/{})", sessions.len(), self.config.max_sessions));
}
let agent_count = sessions.values()
    .filter(|s| /* s.agent_id == agent_id */)
    .count();
if agent_count >= self.config.max_per_agent {
    return Err(format!("ACP per-agent limit reached for '{agent_id}' ({agent_count}/{})"), self.config.max_per_agent));
}
```

---

## Phase 1: Stuck Detection & Idle Timeout

**Goal**: Detect and terminate sessions where the agent process hangs or goes idle.

**Priority**: P0 | **Effort**: Small | **Risk**: Low

### Tasks

| # | Task | File | Details |
|---|------|------|---------|
| 1.1 | Add `last_activity` timestamp to `AcpSession` | `src/acp.rs` | `Instant` field, updated on every prompt start/complete/notification. |
| 1.2 | Add `idle_timeout_secs` to `AcpConfig` | `src/acp.rs` | Default 600 (10 min). Configurable per deployment. |
| 1.3 | Add reaper task in `AcpManager` | `src/acp.rs` | `tokio::spawn` loop that runs every 60s, checks `last_activity` for all `Active` sessions. End sessions that exceed idle timeout. |
| 1.4 | Log and notify on stuck termination | `src/acp.rs` | `warn!` log. Optionally set a flag on the session so the next chat interaction gets a "[session timed out]" message. |

### Implementation Notes

- The reaper needs `Arc<AcpManager>` or a separate spawned task with access.
- Reaper should skip sessions in `Prompting` state (they have their own `prompt_timeout_secs`).
- Consider adding a `cleanup_reason` field to track why a session was ended.

---

## Phase 2: Session Crash Recovery & User Notification

**Goal**: When an ACP agent process dies mid-session, detect it, clean up, and inform the user.

**Priority**: P0 | **Effort**: Medium | **Risk**: Low

### Tasks

| # | Task | File | Details |
|---|------|------|---------|
| 2.1 | Add process health check to `AcpConnection` | `src/acp.rs` | `is_alive() -> bool` method that calls `child.try_wait()` to check if process exited. |
| 2.2 | Detect crash in `AcpManager::prompt` | `src/acp.rs` | Before sending prompt, check `is_alive()`. If dead, attempt auto-recovery (re-spawn + re-initialize). Set `session_reset` flag. |
| 2.3 | Add `session_reset` flag to `AcpSession` | `src/acp.rs` | `bool` field. When true, prepend `"[Context lost: agent session was restarted]"` to next prompt result. Reset flag after delivery. |
| 2.4 | Surface reset notice in tool output | `src/tools/` (acp tools) | When formatting `AcpPromptResult`, check if session was reset and include notice. |

### Implementation Notes

```rust
// AcpConnection:
pub async fn is_alive(&self) -> bool {
    let mut inner = self.inner.lock().await;
    matches!(inner._child.try_wait(), Ok(None))
}

// AcpManager::prompt - before sending:
if !session.connection.is_alive().await {
    warn!("ACP [{}]: process died, attempting restart", session.agent_id);
    // Re-spawn connection, re-create ACP session
    // Set session.session_reset = true
    // Context from previous conversation is lost
}
```

---

## Phase 3: Async Job Mode

**Goal**: Allow long-running ACP tasks to execute asynchronously, returning a job ID immediately and delivering results via `send_message` when complete.

**Priority**: P1 | **Effort**: Large | **Risk**: Medium

### Tasks

| # | Task | File | Details |
|---|------|------|---------|
| 3.1 | Define `AcpJob` struct and job store | `src/acp.rs` | `AcpJob { id, session_id, status, result, created_at, completed_at }`. Store in `AcpManager` as `RwLock<HashMap<String, AcpJob>>`. |
| 3.2 | Add `submit_job` method to `AcpManager` | `src/acp.rs` | Spawns `tokio::spawn` that calls `prompt()`, stores result in job store on completion. |
| 3.3 | Add `acp_submit_job` tool | `src/tools/` | New tool that calls `submit_job`, returns job ID immediately. Takes same params as `acp_prompt`. |
| 3.4 | Add `acp_job_status` tool | `src/tools/` | Query job status by ID. Returns pending/running/completed/failed + result if done. |
| 3.5 | Integrate `send_message` callback | `src/acp.rs` | On job completion, if a `chat_id` is associated, use `send_message` to push results to the user's chat. |
| 3.6 | Add job TTL and cleanup | `src/acp.rs` | Jobs older than 1 hour auto-cleanup. Max 100 jobs in store. |

### Architecture

```
User: "acp_submit_job(session_id, prompt)"
  -> LLM gets back: { job_id: "abc-123", status: "submitted" }
  -> LLM can tell user "Task submitted, I'll notify you when done"

Background:
  tokio::spawn -> prompt(session_id, message)
    -> On completion: send_message(chat_id, formatted_result)
    -> Update job store: status=completed, result=...

User (later): "acp_job_status(abc-123)"
  -> { status: "completed", result: "...", duration_ms: 45000 }
```

### Considerations

- Job store should be in-memory only (no DB persistence needed for MVP).
- Need to handle: job cancellation, session ended while job running.
- `send_message` callback requires access to `AppState` — pass `Arc<AppState>` to job spawner.
- Rate-limit result delivery to avoid spamming channels.

---

## Phase 4: Streaming Progress Push

**Goal**: During ACP prompt execution, progressively send tool call and thinking updates to the user's chat instead of waiting for full completion.

**Priority**: P1 | **Effort**: Medium | **Risk**: Medium

### Tasks

| # | Task | File | Details |
|---|------|------|---------|
| 4.1 | Add `progress_tx` callback to `prompt_streaming` | `src/acp.rs` | Optional `mpsc::UnboundedSender<AcpProgressEvent>` parameter. Send events as notifications arrive. |
| 4.2 | Define `AcpProgressEvent` enum | `src/acp.rs` | `ToolStart { name }`, `ToolComplete { name, status }`, `ThinkingChunk { text }`, `MessageChunk { text }` |
| 4.3 | Wire progress into `AcpManager::prompt` | `src/acp.rs` | Accept optional sender, forward to `prompt_streaming`. |
| 4.4 | Consume progress in ACP command handler | Channel adapters | When routing chat messages to ACP, spawn a progress consumer task that batches updates and calls `send_message` periodically (e.g., every 5s or on tool_call events). |

### Throttling Strategy

- Batch `MessageChunk` / `ThinkingChunk` events (don't send each token).
- Send progress on `ToolStart` events immediately (user sees "Running bash...").
- Max 1 progress message per 5 seconds to avoid rate limits.
- Feishu/Telegram: use message edit (PATCH) to update a single "progress" message in-place.

---

## Phase 5: PTY Fallback Mode

**Goal**: Support CLI tools that don't implement the ACP protocol by wrapping them in a simple subprocess with stdin/stdout piping.

**Priority**: P1 | **Effort**: Medium | **Risk**: Low

### Tasks

| # | Task | File | Details |
|---|------|------|---------|
| 5.1 | Add `mode` field to `AcpAgentConfig` | `src/acp.rs` | `"acp"` (default) or `"pty"`. |
| 5.2 | Implement `PtyConnection` | `src/acp.rs` (or new `src/acp_pty.rs`) | Simpler than `AcpConnection` — no JSON-RPC, just write prompt to stdin, read stdout lines until EOF or timeout. |
| 5.3 | Adapt `AcpManager` to dispatch by mode | `src/acp.rs` | `new_session` checks `mode` field, creates either `AcpConnection` or `PtyConnection`. `AcpSession` holds an enum `ConnectionKind { Acp(AcpConnection), Pty(PtyConnection) }`. |
| 5.4 | Map PTY output to `AcpPromptResult` | `src/acp.rs` | Collect all stdout as `messages`, no structured tool_calls/files_changed. |

### Config Example

```json
{
  "acpAgents": {
    "codex": {
      "mode": "pty",
      "launch": "npx",
      "command": "@openai/codex@latest",
      "args": ["--quiet"],
      "workspace": "/home/user/project"
    }
  }
}
```

### Implementation Notes

- PTY mode doesn't support: session persistence, permission handling, tool call tracking.
- Each "prompt" in PTY mode spawns a new subprocess (stateless), or keeps one alive reading line-by-line.
- Consider using `tokio::process::Command` with `kill_on_drop(true)`.

---

## Phase 6: HTTP API Gateway (Optional)

**Goal**: Expose ACP capabilities as REST endpoints so external systems can invoke RayClaw-managed agents.

**Priority**: P2 | **Effort**: Large | **Risk**: Medium

### Tasks

| # | Task | File | Details |
|---|------|------|---------|
| 6.1 | Add ACP API routes to web server | `src/web.rs` | Mount under `/api/acp/`. Bearer token auth from config. |
| 6.2 | `GET /api/acp/agents` | `src/web.rs` | List configured agents. |
| 6.3 | `POST /api/acp/sessions` | `src/web.rs` | Create new session. Body: `{ agent_id, workspace?, auto_approve? }`. |
| 6.4 | `POST /api/acp/sessions/:id/prompt` | `src/web.rs` | Sync prompt. Body: `{ message, timeout? }`. Returns `AcpPromptResult`. |
| 6.5 | `POST /api/acp/sessions/:id/prompt/stream` | `src/web.rs` | SSE streaming prompt. Sends progress events as SSE, final result as last event. |
| 6.6 | `POST /api/acp/jobs` | `src/web.rs` | Async job submission. Returns job ID. |
| 6.7 | `GET /api/acp/jobs/:id` | `src/web.rs` | Job status query. |
| 6.8 | `DELETE /api/acp/sessions/:id` | `src/web.rs` | End session. |
| 6.9 | `GET /api/acp/health` | `src/web.rs` | Agent health check (are processes alive). |
| 6.10 | Add `acp_api_token` to config | `src/config.rs` | Optional bearer token. If set, all ACP API routes require it. |

### Considerations

- This is optional and gated behind config (`acp_api_enabled: true`).
- Rate limiting should be applied at the API layer.
- Could enable external systems (CI/CD, other bots, web dashboards) to use RayClaw as an agent orchestrator.

---

## Phase 7: Execution Isolation (Long-term)

**Goal**: Run ACP agent subprocesses in isolated environments to protect the host.

**Priority**: P2 | **Effort**: XL | **Risk**: High

### Options (evaluate, don't implement all)

| Approach | Complexity | Isolation Level |
|----------|-----------|----------------|
| Separate OS user per agent | Low | Basic file isolation |
| cgroups resource limits | Medium | CPU/memory caps |
| Docker container per session | High | Full filesystem + network isolation |
| Remote execution (SSH to worker) | High | Physical machine isolation |

### Minimal First Step

- Add `cgroup_limit` config per agent: `{ memory_mb: 4096, cpu_percent: 200 }`.
- On Linux, create cgroup before spawning, move child PID into it.
- Gracefully degrade on macOS/unsupported (log warning, no cgroup).

---

## Implementation Order & Dependencies

```
Phase 0 (Pool Limits)     ─── no dependencies ──────────────────── Week 1
Phase 1 (Stuck Detection) ─── no dependencies ──────────────────── Week 1
Phase 2 (Crash Recovery)  ─── no dependencies ──────────────────── Week 1-2
Phase 3 (Async Jobs)      ─── depends on Phase 2 (recovery) ───── Week 2-3
Phase 4 (Stream Progress) ─── depends on Phase 3 (callback) ───── Week 3-4
Phase 5 (PTY Fallback)    ─── no dependencies ──────────────────── Week 2-3
Phase 6 (HTTP API)        ─── depends on Phase 3 + 4 ──────────── Week 4-6
Phase 7 (Isolation)       ─── independent, long-term ───────────── Future
```

### Quick Wins (can ship in one PR each)

1. **Phase 0**: `max_sessions` / `max_per_agent` — ~50 lines of code change
2. **Phase 1**: Idle reaper task — ~80 lines
3. **Phase 2.1**: `is_alive()` health check — ~10 lines

### Risk Matrix

| Phase | Code Change | Breaking Config | Test Coverage |
|-------|------------|----------------|--------------|
| 0 | Small | No (new optional fields) | Easy |
| 1 | Small | No | Easy |
| 2 | Medium | No | Medium (needs mock process) |
| 3 | Large | No (new tools) | Medium |
| 4 | Medium | No | Hard (integration test) |
| 5 | Medium | No (new optional field) | Medium |
| 6 | Large | No (new optional routes) | Medium |
| 7 | XL | Possibly | Hard |

---

## Config Evolution

Current `acp.json`:
```json
{
  "defaultAutoApprove": true,
  "promptTimeoutSecs": 300,
  "acpAgents": {
    "claude": {
      "launch": "npx",
      "command": "@anthropic-ai/claude-code@latest",
      "args": ["--acp"],
      "workspace": "/home/user/project"
    }
  }
}
```

After all phases:
```json
{
  "defaultAutoApprove": true,
  "promptTimeoutSecs": 300,
  "maxSessions": 20,
  "maxPerAgent": 10,
  "idleTimeoutSecs": 600,
  "acpApiEnabled": false,
  "acpApiToken": "bearer-secret",
  "acpAgents": {
    "claude": {
      "mode": "acp",
      "launch": "npx",
      "command": "@anthropic-ai/claude-code@latest",
      "args": ["--acp"],
      "workspace": "/home/user/project"
    },
    "codex": {
      "mode": "pty",
      "launch": "npx",
      "command": "@openai/codex@latest",
      "args": ["--quiet"],
      "workspace": "/home/user/project"
    }
  }
}
```

All new fields have serde defaults, so existing configs remain compatible.
