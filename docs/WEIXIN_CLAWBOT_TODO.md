# WeChat (Weixin) Channel Adapter - Implementation TODO

> Native Rust implementation of the WeChat iLink Bot protocol for RayClaw
> MVP scope: **text messaging only**, media/CDN deferred
> Status: **MVP IMPLEMENTED** (2026-03-23) — all phases complete, build passing

---

## Phase 1: Scaffolding

- [ ] **1.1** `Cargo.toml` — Add `weixin = []` feature flag, add to `default` and `all`
- [ ] **1.2** `src/channels/mod.rs` — Add `#[cfg(feature = "weixin")] pub mod weixin` + re-export `WeixinAdapter`
- [ ] **1.3** `src/channels/weixin.rs` — Create file with `WeixinChannelConfig` struct
  - `bot_token: String` (Bearer token from QR login)
  - `base_url: String` (default `https://ilinkai.weixin.qq.com`)
  - `route_tag: Option<String>` (optional `SKRouteTag` header)
  - `account_id: Option<String>` (ilink_bot_id)
- [ ] **1.4** `rayclaw.config.example.yaml` — Add weixin channel config example

---

## Phase 2: HTTP API Client (in `weixin.rs`)

### Serde Types

- [ ] **2.1** `WeixinMessage` struct (seq, message_id, from_user_id, to_user_id, client_id, session_id, group_id, message_type, message_state, item_list, context_token)
- [ ] **2.2** `MessageItem` struct (type, text_item, create_time_ms, ref_msg) — text only for MVP
- [ ] **2.3** `TextItem` struct (text)
- [ ] **2.4** `GetUpdatesResp` struct (ret, errcode, errmsg, msgs, get_updates_buf, longpolling_timeout_ms)
- [ ] **2.5** `SendMessageReq` struct (msg)
- [ ] **2.6** `GetConfigResp` struct (ret, errmsg, typing_ticket)
- [ ] **2.7** `SendTypingReq` struct (ilink_user_id, typing_ticket, status)
- [ ] **2.8** `BaseInfo` struct (channel_version)

### Request Helpers

- [ ] **2.9** `random_wechat_uin()` — random u32 -> decimal string -> base64
- [ ] **2.10** `build_headers(token, route_tag)` — Authorization, AuthorizationType, X-WECHAT-UIN, SKRouteTag
- [ ] **2.11** `api_post(client, base_url, endpoint, body, token, route_tag, timeout)` — common POST wrapper

### API Functions

- [ ] **2.12** `get_updates(client, base_url, token, route_tag, get_updates_buf)` -> `GetUpdatesResp` (35s timeout)
- [ ] **2.13** `send_message(client, base_url, token, route_tag, msg)` (15s timeout)
- [ ] **2.14** `get_config(client, base_url, token, route_tag, ilink_user_id, context_token)` -> `GetConfigResp` (10s timeout)
- [ ] **2.15** `send_typing(client, base_url, token, route_tag, req)` (10s timeout)

---

## Phase 3: WeixinAdapter + ChannelAdapter Trait

- [ ] **3.1** `WeixinAdapter` struct
  - `http_client: reqwest::Client`
  - `base_url: String`
  - `bot_token: String`
  - `route_tag: Option<String>`
  - `context_tokens: Arc<RwLock<HashMap<String, String>>>` (user_id -> context_token)
  - `typing_tickets: Arc<RwLock<HashMap<String, String>>>` (user_id -> typing_ticket)
- [ ] **3.2** `WeixinAdapter::new(config)` constructor
- [ ] **3.3** `ChannelAdapter::name()` -> `"weixin"`
- [ ] **3.4** `ChannelAdapter::chat_type_routes()` -> `[("weixin_dm", Private)]`
- [ ] **3.5** `ChannelAdapter::send_text(external_chat_id, text)`
  - Look up context_token for user
  - Build SendMessageReq: message_type=2(BOT), message_state=2(FINISH), text item
  - Split long text at ~4000 chars
  - POST to sendmessage endpoint

---

## Phase 4: Long-Poll Message Loop

- [ ] **4.1** `start_weixin_bot(app_state)` — main entry point
  - Load `WeixinChannelConfig` from app_state.config
  - Load persisted `get_updates_buf` from `{data_dir}/weixin_sync.json`
  - Enter poll loop
- [ ] **4.2** Poll loop logic
  - POST `getupdates` with `get_updates_buf`
  - On client timeout (35s) -> normal, retry
  - On `errcode == -14` -> log, pause 1 hour, continue
  - On other error -> log, sleep 5s, continue
  - Update & persist `get_updates_buf`
- [ ] **4.3** Message dispatch
  - Filter: `message_type == 1` (USER) only
  - Filter: `message_state == 0` (NEW) only — skip GENERATING/FINISH echoes
  - Extract text from `item_list` (type=1 TEXT items)
  - Cache `context_token`: map `from_user_id -> msg.context_token`
  - `tokio::spawn` -> `handle_weixin_message()`
- [ ] **4.4** `load_sync_buf(data_dir)` / `save_sync_buf(data_dir, buf)` — JSON file persistence

---

## Phase 5: Message Handler

- [ ] **5.1** `handle_weixin_message()` — follows Feishu pattern
  - `db.resolve_or_create_chat_id("weixin", from_user_id, title, "weixin_dm")`
  - `db.store_message()` for incoming message
- [ ] **5.2** Slash commands: `/reset`, `/skills`, `/archive`, `/usage`
  - Send response via direct API call (not via adapter send_text)
- [ ] **5.3** Typing indicator
  - Fetch `typing_ticket` via `get_config()` (cache in `typing_tickets` map)
  - Spawn task: POST `send_typing` every 4s
  - Abort on response ready
- [ ] **5.4** Agent engine call
  - `process_with_agent_with_events()` with `AgentRequestContext { caller_channel: "weixin", chat_id, chat_type: "private" }`
- [ ] **5.5** Response delivery
  - Check `event_rx` for `AgentEvent::SendMessageUsed` (skip if agent already sent via tool)
  - Send response text via adapter `send_text()`
  - Store bot response via `db.store_message()`

---

## Phase 6: Runtime Integration

- [ ] **6.1** `src/runtime.rs` — Add imports
  - `#[cfg(feature = "weixin")] use crate::channels::WeixinAdapter;`
  - `#[cfg(feature = "weixin")] use crate::channels::weixin::WeixinChannelConfig;`
- [ ] **6.2** `src/runtime.rs` — Add `has_weixin` flag + config loading + adapter registration
- [ ] **6.3** `src/runtime.rs` — Spawn `start_weixin_bot()` task
- [ ] **6.4** `src/runtime.rs` — Add weixin to `has_other_channel` check
- [ ] **6.5** `src/runtime.rs` — Add "weixin" to error message listing all channels

---

## Phase 7: Testing & Validation

- [ ] **7.1** `cargo build --features weixin` compiles without errors
- [ ] **7.2** `cargo build` (default features) compiles with weixin included
- [ ] **7.3** Config loading: weixin section parsed correctly from YAML
- [ ] **7.4** Smoke test: bot starts, connects to long-poll, receives messages
- [ ] **7.5** Text round-trip: send message -> agent processes -> reply delivered
- [ ] **7.6** Slash commands work: `/reset`, `/skills`, `/usage`
- [ ] **7.7** Session persistence: restart bot, `get_updates_buf` resumes correctly
- [ ] **7.8** Token expiry: `errcode=-14` pauses and recovers after 1 hour

---

## Deferred (Post-MVP)

- [ ] CDN media upload/download (AES-128-ECB encrypt/decrypt)
- [ ] Image receiving & sending
- [ ] File attachment support
- [ ] Video message support
- [ ] Voice/SILK transcoding
- [ ] QR code interactive login flow (via `rayclaw setup`)
- [ ] Multi-account support (multiple bot tokens)
- [ ] Group message support (if iLink API supports it)
- [ ] `send_attachment()` ChannelAdapter implementation
- [ ] Message deduplication cache (in-memory + DB)
- [ ] Configurable message split length
- [ ] Reconnect backoff strategy (exponential)

---

## Protocol Quick Reference

| Header | Value |
|--------|-------|
| `Content-Type` | `application/json` |
| `Authorization` | `Bearer {bot_token}` |
| `AuthorizationType` | `ilink_bot_token` |
| `X-WECHAT-UIN` | `base64(random_u32.to_string())` |
| `SKRouteTag` | *(optional, from config)* |

| Endpoint | Path | Timeout |
|----------|------|---------|
| getUpdates | `ilink/bot/getupdates` | 35s |
| sendMessage | `ilink/bot/sendmessage` | 15s |
| getConfig | `ilink/bot/getconfig` | 10s |
| sendTyping | `ilink/bot/sendtyping` | 10s |
| getUploadUrl | `ilink/bot/getuploadurl` | 15s *(deferred)* |

| Key | Format |
|-----|--------|
| User ID | `xxx@im.wechat` |
| Bot ID | `xxx@im.bot` |
| context_token | Opaque string, must echo in every reply |
| get_updates_buf | Base64 string, persist across restarts |
| message_type | 1=USER, 2=BOT |
| message_state | 0=NEW, 1=GENERATING, 2=FINISH |
| item type | 1=TEXT, 2=IMAGE, 3=VOICE, 4=FILE, 5=VIDEO |

---

## Files to Change

| File | Action | Lines (est.) |
|------|--------|-------------|
| `Cargo.toml` | Edit | +3 |
| `src/channels/mod.rs` | Edit | +4 |
| `src/channels/weixin.rs` | **New** | ~800-1000 |
| `src/runtime.rs` | Edit | +25 |
| `rayclaw.config.example.yaml` | Edit | +5 |
| **Total** | | ~850 |
