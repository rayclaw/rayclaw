# RayClaw Web Dashboard — Implementation Task

**Version**: v0.2.1
**Type**: Read-only information panel
**Approach**: Extend existing Web Channel (Axum + React + Radix UI Themes)

---

## Scope

A lightweight, read-only dashboard embedded in the existing Web UI. Three tabs: **Overview**, **Tasks**, **Memories**. No new dependencies. All endpoints GET-only behind existing Bearer token auth.

---

## Current Codebase Baseline

| Layer | Status |
|-------|--------|
| HTTP Server | Axum, 24 route paths, `require_auth` on all API handlers |
| Frontend | React 18.3 + Radix UI Themes 3.2 + Tailwind v4 + Vite 5.4 |
| Data | SQLite WAL, rusqlite, `ScheduledTask` (10 fields), `Memory` (12 fields) |
| Auth | Bearer Token (`web_auth_token`), per-handler check |
| Build | Vite -> `web/dist/` -> `include_dir!` embedded in binary |

### Existing API (reusable for Dashboard)

| Endpoint | Returns |
|----------|---------|
| `GET /api/health` | version, uptime |
| `GET /api/sessions` | all-channel session list |
| `GET /api/history?session=X` | message history |
| `GET /api/usage` | token usage + memory stats |
| `GET /api/memory_observability` | reflector/injection time-series |
| `GET /api/config` | sanitized runtime config |
| `GET /api/acp/agents` | configured ACP agents |
| `GET /api/acp/sessions` | ACP coding sessions |

### Existing DB Methods (reusable)

| Method | Notes |
|--------|-------|
| `get_tasks_for_chat(chat_id)` | Per-chat only, no global query |
| `get_task_run_logs(task_id, limit)` | Ready to use |
| `get_all_memories_for_chat(chat_id: Option<i64>)` | `None` = global, but no offset pagination |
| `search_memories(chat_id, query, limit)` | Keyword search, no offset |
| `get_memory_observability_summary(...)` | Stats for overview |

---

## New Backend Work

### Phase 1: Database (`src/db.rs`)

#### 1.1 `get_all_tasks(status, schedule_type, limit, offset) -> (Vec<ScheduledTask>, usize)`

```sql
SELECT * FROM scheduled_tasks
WHERE (?1 IS NULL OR status = ?1)
  AND (?2 IS NULL OR schedule_type = ?2)
ORDER BY
  CASE WHEN status = 'active' THEN 0 WHEN status = 'paused' THEN 1 ELSE 2 END,
  next_run ASC
LIMIT ?3 OFFSET ?4;

SELECT COUNT(*) FROM scheduled_tasks
WHERE (?1 IS NULL OR status = ?1)
  AND (?2 IS NULL OR schedule_type = ?2);
```

#### 1.2 `get_tasks_summary() -> TasksSummary`

```rust
pub struct TasksSummary {
    pub total: usize,
    pub active: usize,
    pub paused: usize,
    pub completed: usize,
    pub cancelled: usize,
    pub runs_24h: usize,
    pub failures_24h: usize,
}
```

```sql
SELECT status, COUNT(*) FROM scheduled_tasks GROUP BY status;
SELECT COUNT(*) FROM task_run_logs WHERE started_at > datetime('now', '-24 hours');
SELECT COUNT(*) FROM task_run_logs WHERE success = 0 AND started_at > datetime('now', '-24 hours');
```

#### 1.3 `browse_memories(chat_id, category, include_archived, search, limit, offset) -> (Vec<Memory>, usize)`

Extend existing `get_all_memories_for_chat` with offset pagination and filters.

```sql
SELECT * FROM memories
WHERE (?1 IS NULL OR chat_id = ?1)
  AND (?2 IS NULL OR category = ?2)
  AND (?3 = 1 OR is_archived = 0)
  AND (?4 IS NULL OR content LIKE '%' || ?4 || '%')
ORDER BY updated_at DESC
LIMIT ?5 OFFSET ?6;
```

#### 1.4 `get_db_stats() -> DbStats`

```rust
pub struct DbStats {
    pub chats_count: usize,
    pub messages_count: usize,
    pub memories_count: usize,
    pub tasks_count: usize,
    pub db_size_bytes: u64,
}
```

```sql
SELECT COUNT(*) FROM chats;
SELECT COUNT(*) FROM messages;
SELECT COUNT(*) FROM memories;
SELECT COUNT(*) FROM scheduled_tasks;
PRAGMA page_count; PRAGMA page_size; -- multiply for db_size_bytes
```

**Estimated: ~120 lines in db.rs**

---

### Phase 2: API Handlers (`src/web.rs`)

All under `/api/dashboard/`, all GET, all behind `require_auth`.

| Endpoint | Handler | Query Params |
|----------|---------|-------------|
| `GET /api/dashboard/tasks` | `api_dashboard_tasks` | `?status=active&type=cron&limit=50&offset=0` |
| `GET /api/dashboard/tasks/summary` | `api_dashboard_tasks_summary` | — |
| `GET /api/dashboard/tasks/:id/logs` | `api_dashboard_task_logs` | `?limit=20` |
| `GET /api/dashboard/memories` | `api_dashboard_memories` | `?chat_id=&category=&archived=false&search=&limit=50&offset=0` |
| `GET /api/dashboard/db/stats` | `api_dashboard_db_stats` | — |

Response format: JSON, consistent with existing API style.

**Estimated: ~200 lines in web.rs**

---

## New Frontend Work

### Phase 3: Dashboard Component (`web/src/components/dashboard.tsx`)

#### Entry point

- Sidebar bottom: "Dashboard" button (alongside existing Settings / Usage buttons)
- Hash route: `#/dashboard` (or `#/dashboard/tasks`, `#/dashboard/memories`)
- Full-width panel replacing chat area when active

#### Tab 1: Overview

| Card | Source | Content |
|------|--------|---------|
| Runtime | `/api/health` | Version, uptime, start time |
| Sessions | `/api/sessions` | Total count, per-channel breakdown |
| Tasks | `/api/dashboard/tasks/summary` | Active/paused counts, 24h runs/failures |
| Memory | `/api/usage` | Total memories, active/archived ratio |
| Token Usage | `/api/usage` | 24h token consumption |
| Database | `/api/dashboard/db/stats` | Row counts, DB file size |

#### Tab 2: Tasks

**Table columns:**

| Column | Field | Notes |
|--------|-------|-------|
| Status | `status` | Badge: active / paused / completed / cancelled |
| Type | `schedule_type` | `cron` / `once` |
| Schedule | `schedule_value` | Cron expression or ISO timestamp |
| Next Run | `next_run` | Relative time ("in 3m") |
| Last Run | `last_run` | Relative time + success badge |
| Chat | `chat_id` | Channel + title (lookup from sessions) |
| Prompt | `prompt` | Truncated to 80 chars, expandable |

**Expandable row:** Task execution logs from `/api/dashboard/tasks/:id/logs`

**Filters:**
- Status: all / active / paused / completed / cancelled
- Type: all / cron / once
- Sort: next run (default) / created / last run

#### Tab 3: Memories

**Table columns:**

| Column | Field | Notes |
|--------|-------|-------|
| Content | `content` | Truncated, expandable |
| Category | `category` | Badge: PROFILE / KNOWLEDGE / EVENT |
| Scope | `chat_id` | "Global" or channel + chat title |
| Confidence | `confidence` | Progress bar 0-1.0 |
| Source | `source` | tool / reflector / write_memory_tool |
| Updated | `updated_at` | Relative time |
| Status | `is_archived` | active / archived |

**Filters:**
- Category: all / PROFILE / KNOWLEDGE / EVENT
- Scope: all / global / per-chat
- Include archived: toggle
- Search: keyword input

**Estimated: ~800 lines in dashboard.tsx**

---

## Implementation Plan

### Phase 1: Backend API (~120 + 200 = 320 lines)

```
1.1  Add TasksSummary, DbStats structs to db.rs
1.2  Implement get_all_tasks() in db.rs
1.3  Implement get_tasks_summary() in db.rs
1.4  Implement browse_memories() in db.rs
1.5  Implement get_db_stats() in db.rs
1.6  Add 5 handler functions in web.rs
1.7  Register routes in build_router()
1.8  cargo build + cargo test
```

### Phase 2: Frontend Dashboard (~800 lines)

```
2.1  Create web/src/components/dashboard.tsx skeleton
2.2  Add Dashboard button to session-sidebar.tsx
2.3  Wire hash routing in main.tsx (#/dashboard)
2.4  Implement Overview tab (stat cards)
2.5  Implement Tasks tab (table + filters + expandable logs)
2.6  Implement Memories tab (table + filters + search)
2.7  npm run build + verify embedded assets
```

### Phase 3: Polish

```
3.1  Manual refresh button per tab
3.2  Responsive layout for narrow viewports
3.3  Loading / empty / error states for each tab
3.4  cargo clippy + cargo fmt + cargo test
```

---

## File Change Summary

| File | Action | Lines |
|------|--------|-------|
| `src/db.rs` | Add 4 query methods + 2 structs | +120 |
| `src/web.rs` | Add 5 handlers + route registration | +200 |
| `web/src/components/dashboard.tsx` | New file, 3 tabs | +800 |
| `web/src/components/session-sidebar.tsx` | Add Dashboard button | +10 |
| `web/src/main.tsx` | Hash route + panel switching | +30 |
| **Total** | | **~1,160 lines** |

---

## Constraints

- **Read-only**: All dashboard endpoints are GET. No mutations from dashboard.
- **Zero new deps**: Frontend uses existing Radix UI Themes components (Table, Tabs, Badge, Card, Dialog). Backend uses existing rusqlite.
- **Auth**: All `/api/dashboard/*` routes use existing `require_auth` with `web_auth_token`.
- **Embed**: Dashboard ships in the binary via `include_dir!("web/dist")`. No separate deployment.
- **No auto-refresh**: Manual refresh button only. Avoids unnecessary polling load.
