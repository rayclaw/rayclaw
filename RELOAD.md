# RayClaw 热更新方案 (Hot-Reload Plan)

> 目标：从飞书对话中触发 bug 修复 → 编译 → 重启，实现「自我修复」闭环。
>
> ⚠️ 本文档为设计方案，**不修改任何现有代码**。待确认后分阶段实施。

---

## 一、当前状态分析

### 已有基础设施 ✅

| 组件 | 状态 | 路径 |
|------|------|------|
| systemd 服务 | ✅ active, Restart=always | `/etc/systemd/system/rayclaw.service` |
| sudoers 免密 | ✅ restart/stop/start | `/etc/sudoers.d/rayclaw` |
| hot-reload.sh | ✅ 4种模式 | `scripts/hot-reload.sh` |
| Release 二进制 | ✅ 37MB | `target/release/rayclaw` |
| 优雅关闭 | ✅ SIGTERM/SIGHUP/Ctrl-C + 2s drain | `src/runtime.rs` |
| SQLite WAL | ✅ 崩溃安全 | 数据目录 |
| 飞书 WebSocket | ✅ 长连接 + 重连 | `src/channels/feishu.rs` |
| Rust 工具链 | ✅ rustc 1.93.1 | `/usr/local/bin/cargo` |

### 缺失的部分 ❌

| 缺失 | 说明 |
|------|------|
| 飞书侧触发机制 | Bot 无法从对话中识别 "修bug + 重启" 指令 |
| Claude Code 集成 | hot-reload.sh 有 `--claude` 模式，但 Bot 侧无法调用 |
| 自愈链路 | 如果重启后 Bot 挂了，没有自动回滚 |
| 状态持久化 | 重启前后的对话上下文连续性（已有 SQLite，但需验证） |
| 健康检查 | 重启后无主动确认机制 |

---

## 二、整体架构

```
飞书对话
  │
  ├── 用户: "@bot 修一下 XXX bug"
  │
  ▼
RayClaw Bot (agent_engine)
  │
  ├── 1. 识别为「热更新」意图
  ├── 2. 调用 bash 工具执行 hot-reload.sh
  │     ├── Phase A: Claude Code 修 bug（可选）
  │     ├── Phase B: cargo build --release
  │     ├── Phase C: 备份旧二进制
  │     └── Phase D: systemctl restart
  │
  ├── 3. Bot 进程被 SIGTERM 终止（优雅关闭，2s drain）
  │
  ▼
systemd 检测到进程退出
  │
  ├── Restart=always, RestartSec=3
  │
  ▼
新二进制启动
  │
  ├── 加载 SQLite（会话/记忆/定时任务全部恢复）
  ├── 飞书 WebSocket 重新连接
  │
  ▼
Bot 上线，主动发消息: "✅ 热更新完成，版本: xxx"
```

### 关键时序（预估）

| 阶段 | 耗时 | 说明 |
|------|------|------|
| Claude Code 修 bug | 30-120s | 取决于 bug 复杂度 |
| cargo build --release | 60-180s | 增量编译通常 60-90s |
| 优雅关闭 | 2s | SIGTERM + drain |
| systemd 重启间隔 | 3s | RestartSec=3 |
| Bot 初始化 + WS 连接 | 3-5s | DB + 飞书握手 |
| **总断线时间** | **~8s** | 不含编译时间（编译期间 Bot 仍在运行） |

---

## 三、分阶段实施计划

### Phase 1: 基础自重启（最小可用）

**改动范围：** 无需改 Rust 代码，只需确认现有 bash 工具权限。

**流程：**
```
用户 → "@bot 重启一下"
Bot  → 通过 bash 工具执行:
         sudo systemctl restart rayclaw
Bot  → 进程终止（SIGTERM → 2s drain → exit）
systemd → 3s 后重启新进程
新 Bot  → 飞书重连，发送确认消息
```

**验证清单：**
- [ ] Bot 的 bash 工具可以执行 `sudo systemctl restart rayclaw`
- [ ] 重启后 SQLite 数据完整（会话、记忆、定时任务）
- [ ] 飞书 WebSocket 在新进程中自动重连
- [ ] 重启后 Bot 能主动发消息确认上线

**风险：低** — 利用已有的 systemd + sudoers，不碰代码。

---

### Phase 2: 编译 + 重启

**改动范围：** 仍然不改 Rust 代码，利用现有 hot-reload.sh。

**流程：**
```
用户 → "@bot 编译重启"
Bot  → bash: cd <project-dir> && scripts/hot-reload.sh
       ├── 备份 target/release/rayclaw → rayclaw.bak
       ├── cargo build --release
       │   └── 编译期间 Bot 仍在运行，可回复消息
       ├── sudo systemctl restart rayclaw
       └── 新进程启动
新 Bot → 确认消息
```

**回滚策略：**
```bash
# 如果新二进制启动失败（5s 内 systemd 检测到退出）
# systemd Restart=always 会尝试重启
# 但重启的还是有 bug 的新二进制

# 需要手动回滚：
cp target/release/rayclaw.bak target/release/rayclaw
sudo systemctl restart rayclaw
```

**验证清单：**
- [ ] `cargo build --release` 增量编译耗时可接受（< 3min）
- [ ] 编译期间 Bot 正常响应（CPU 占用测试）
- [ ] 编译失败时自动恢复 backup
- [ ] hot-reload.sh 的退出码正确传递

**风险：中低** — 编译可能消耗 CPU 影响 Bot 响应。

---

### Phase 3: Claude Code 修 Bug + 编译 + 重启（完整链路）

**改动范围：** 需要确认 `claude` CLI 已安装并可用。

**前置条件：**
```bash
# 确认 Claude Code CLI 可用
which claude
claude --version
# 需要有效的 API key 或 session
```

**流程：**
```
用户 → "@bot 修一下这个 bug: 定时任务时区计算有问题"
Bot  → 1. 解析用户意图，构造 Claude Code prompt
     → 2. bash: scripts/hot-reload.sh --claude "修复 scheduler.rs 中的时区计算 bug，
            确保 Asia/Shanghai 的 cron 触发时间正确"
       ├── Claude Code 分析代码、生成补丁
       ├── cargo build --release（含新补丁）
       ├── 编译成功 → systemctl restart
       └── 编译失败 → 恢复 backup，报错给用户
新 Bot → "✅ Bug 已修复并部署：
         - 修改文件: src/scheduler.rs
         - 变更摘要: xxx
         - 编译耗时: 72s
         - 当前版本: abc1234"
```

**安全约束：**
```
⚠️ Claude Code 使用 --dangerously-skip-permissions 模式
   - 仅允许修改 <project-dir>/src/ 下的文件
   - 不允许执行网络请求、安装系统包
   - 不允许修改 .env / config / 密钥文件
   
建议：Claude Code prompt 中明确限制范围：
   "只修改 src/ 目录下的 .rs 文件，不要修改 Cargo.toml、配置文件或脚本"
```

**验证清单：**
- [ ] `claude` CLI 已安装且能正常工作
- [ ] Claude Code 能正确理解项目结构
- [ ] Claude Code 修改后 `cargo build` 能通过
- [ ] 修改历史可追溯（git diff / commit）

**风险：中** — Claude Code 修改可能引入新 bug。

---

### Phase 4: 自愈与高级功能（可选增强）

**需要修改 Rust 代码。**

#### 4.1 启动时自动发送上线通知

```rust
// src/runtime.rs — 在所有 channel 启动后
// 检查是否是热更新重启（通过检查 /tmp/rayclaw-reload.log 的时间戳）
// 如果是，向触发热更新的 chat_id 发送确认消息
```

**实现思路：**
```
1. hot-reload.sh 重启前写入 /tmp/rayclaw-reload-trigger.json:
   {"chat_id": 1, "timestamp": "...", "reason": "bug fix"}

2. 新进程启动后读取该文件
3. 向 chat_id 发送确认消息
4. 删除该文件
```

#### 4.2 自动回滚

```bash
# 在 hot-reload.sh 中增加健康检查：
# 重启后 10s 内检查服务状态
# 如果不健康，自动回滚到 .bak 并再次重启

sleep 10
if ! systemctl is-active --quiet rayclaw; then
    log "❌ 新版本启动失败，自动回滚"
    cp "$BACKUP" "$BINARY"
    sudo systemctl restart rayclaw
fi
```

#### 4.3 Git 集成

```bash
# 每次 Claude Code 修改后自动 commit
cd "$PROJECT_DIR"
git add -A
git commit -m "hotfix: $CLAUDE_PROMPT (auto-applied by hot-reload)"
```

#### 4.4 编译缓存优化

```toml
# .cargo/config.toml — 加速增量编译
[build]
incremental = true

[profile.release]
incremental = true      # release 也开增量
lto = "thin"           # 用 thin LTO 替代 full LTO，编译更快
```

---

## 四、安全与风险控制

### 权限矩阵

| 操作 | 谁能触发 | 限制 |
|------|----------|------|
| 仅重启 | Bot bash 工具 | sudoers 已配置 |
| 编译+重启 | Bot bash 工具 | 仅在项目目录内 |
| Claude Code 修 bug | Bot bash 工具 | `--dangerously-skip-permissions` |
| 回滚 | 手动 / 自动 | 从 .bak 恢复 |

### 风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|----------|
| 编译失败 | 中 | 无影响（旧进程继续运行） | hot-reload.sh 已有 backup 恢复逻辑 |
| 新版本启动崩溃 | 低 | Bot 短暂离线 | systemd Restart=always + Phase 4.2 自动回滚 |
| Claude Code 引入新 bug | 中 | 可能更严重的 bug | git commit 可追溯 + .bak 可回滚 |
| 编译期间 CPU 打满 | 中 | Bot 响应变慢 | 可用 `nice -n 19 cargo build` 降低优先级 |
| 重启丢失进行中的请求 | 低 | 一两条消息无响应 | 2s drain + 用户可重发 |
| 数据库损坏 | 极低 | 数据丢失 | SQLite WAL 模式 + 定期备份 |

### 绝对不能做的事 🚫

1. **不能在 Claude Code prompt 中包含密钥/token**
2. **不能让 Claude Code 修改配置文件**（`rayclaw.config.yaml`, `.env`）
3. **不能在 Bot 响应中泄露编译日志中的路径/密钥**
4. **不能在未备份时直接覆盖二进制**

---

## 五、使用方式（实施后）

### 日常操作

```
# 场景1: 简单重启
用户: "@bot 重启一下"
Bot:  "🔄 正在重启..."
      (8秒后)
Bot:  "✅ 已重新上线，运行正常"

# 场景2: 编译最新代码并重启
用户: "@bot 编译重启"
Bot:  "🔨 开始编译... (预计60-90秒)"
Bot:  "✅ 编译完成 (38MB, 72s)，正在重启..."
      (8秒后)
Bot:  "✅ 已重新上线，版本: abc1234"

# 场景3: 修 bug 并部署
用户: "@bot 修一下这个 bug: 定时任务在北京时间7点没触发"
Bot:  "🤖 正在让 Claude Code 分析问题..."
Bot:  "📝 Claude Code 修改了 src/scheduler.rs:
       - 修复了 UTC→Asia/Shanghai 的时区转换
       - 增量编译中..."
Bot:  "✅ 编译完成，正在重启..."
      (8秒后)
Bot:  "✅ Bug 已修复并部署！"
```

### 紧急恢复（手动）

```bash
# 如果 Bot 完全无法启动
ssh ubuntu@server

# 查看日志
tail -100 /var/log/rayclaw.log

# 回滚到上一个工作版本
cd /opt/rayclaw
cp target/release/rayclaw.bak target/release/rayclaw
sudo systemctl restart rayclaw

# 如果 .bak 也坏了，从 git 恢复
git stash        # 保存 Claude Code 的修改
cargo build --release
sudo systemctl restart rayclaw
```

---

## 六、实施优先级建议

```
Phase 1 (基础自重启)     ← 建议先做，零风险，30分钟搞定
  ↓
Phase 2 (编译+重启)      ← 紧接着做，验证完整编译流程
  ↓
Phase 3 (Claude Code)   ← 确认 claude CLI 可用后实施
  ↓
Phase 4 (自愈/高级)      ← 按需，可以慢慢来
```

**Phase 1+2 可以在 1 小时内完成，不需要改任何 Rust 代码。**

---

*文档版本: v1.0 | 2025-02-21*
*项目: RayClaw*
*当前二进制: 37MB release, rustc 1.93.1*
