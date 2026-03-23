# WeChat (Weixin) Adapter - Local Testing Guide

## Prerequisites

- RayClaw compiled with `weixin` feature (default enabled)
- A personal WeChat account (for scanning QR code)
- LLM provider configured (Anthropic/OpenAI, via `rayclaw setup`)
- Network access to `ilinkai.weixin.qq.com`

## Step 1: Build

```bash
cargo build
```

## Step 2: QR Code Login

Run the login command to obtain a `bot_token`:

```bash
cargo run -- weixin-login
```

Options:
```bash
# Custom API base URL
cargo run -- weixin-login --base-url https://ilinkai.weixin.qq.com

# Custom data directory
cargo run -- weixin-login --data-dir ./my-data
```

### What happens:

1. RayClaw calls `GET ilink/bot/get_bot_qrcode?bot_type=3`
2. A QR code URL is printed to the terminal
3. **Open the URL in a browser** to see the QR image, then scan it with WeChat
4. RayClaw long-polls `GET ilink/bot/get_qrcode_status` until you confirm
5. On success, credentials are saved to `rayclaw.data/weixin_credentials.json`

### Expected output:

```
WeChat iLink Bot — QR Code Login
================================

Base URL: https://ilinkai.weixin.qq.com
Fetching QR code...

  Scan this QR code with WeChat:
  https://ilinkai.weixin.qq.com/...

Waiting for scan (timeout: 480s)...

.......
  Scanned! Please confirm on your phone...

  Login successful!
  Account ID: b0f5860f...@im.bot
  Base URL:   https://ilinkai.weixin.qq.com
  Token:      eyJhbGci...abcd

  Credentials saved to ./rayclaw.data/weixin_credentials.json

  Next step: add to your rayclaw.config.yaml:
    channels:
      weixin:
        bot_token: "eyJhbGci..."
```

## Step 3: Configure

Edit `rayclaw.config.yaml` and add the WeChat channel:

```yaml
# LLM provider (already configured via setup)
llm_provider: anthropic
api_key: "sk-ant-..."
model: "claude-sonnet-4-20250514"

# WeChat channel
channels:
  weixin:
    bot_token: "PASTE_TOKEN_FROM_STEP_2"
    # base_url: "https://ilinkai.weixin.qq.com"  # default, usually no need to change
    # route_tag: ""  # optional
```

Or copy the token from the saved credentials:

```bash
cat rayclaw.data/weixin_credentials.json | python3 -c "import sys,json; print(json.load(sys.stdin)['bot_token'])"
```

## Step 4: Start

```bash
cargo run -- start
```

### Expected log output:

```
INFO  RayClaw runtime starting...
INFO  Database initialized
INFO  Memory manager initialized
INFO  Starting WeChat bot (iLink long-poll)
INFO  Weixin: starting long-poll message loop
INFO  Weixin: long-poll loop started, base_url=https://ilinkai.weixin.qq.com
```

## Step 5: Test Text Messaging

1. Open WeChat on your phone
2. Find the bot account (the one you scanned for) in your contacts
3. Send a text message, e.g., "Hello"
4. You should see in the RayClaw logs:
   ```
   INFO  Weixin message from user123@im.wechat : Hello
   ```
5. The bot should reply via WeChat within a few seconds

## Step 6: Test Slash Commands

| Command | Expected Result |
|---------|-----------------|
| `/reset` | "Context cleared (session + chat history)." |
| `/skills` | List of available skills |
| `/usage` | Token usage statistics |
| `/archive` | Archives current session |

## Step 7: Test Session Persistence

1. Send a few messages to establish context
2. Stop RayClaw (`Ctrl+C`)
3. Restart: `cargo run -- start`
4. Check that `rayclaw.data/weixin_sync.json` exists and contains `get_updates_buf`
5. The bot should resume without replaying old messages

## Troubleshooting

### "Weixin: bot_token is empty"
Token not configured. Run `weixin-login` first and copy the token to config.

### "getUpdates: request failed: ..."
Network issue. Check connectivity to `ilinkai.weixin.qq.com`.

### "Weixin: session expired (errcode=-14), pausing for 1 hour"
Bot token expired. You need to re-login:
```bash
cargo run -- weixin-login
```
Then update the `bot_token` in your config and restart.

### "Weixin: sending without context_token — reply may be orphaned"
This happens when the bot tries to reply but hasn't received an inbound message yet
(context_token is populated from inbound messages). Usually resolves after the first
message exchange.

### Bot receives messages but doesn't reply
- Check LLM provider config (api_key, model)
- Check logs for agent errors
- Try `/reset` to clear session state

## File Locations

| File | Purpose |
|------|---------|
| `rayclaw.config.yaml` | Main config (bot_token goes here) |
| `rayclaw.data/weixin_credentials.json` | Saved QR login credentials |
| `rayclaw.data/weixin_sync.json` | Long-poll cursor (get_updates_buf) |
| `rayclaw.data/runtime/db.sqlite` | Message history, sessions |
