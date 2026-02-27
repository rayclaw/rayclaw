#!/bin/bash
# RayClaw Hot-Reload Script
# 用法:
#   ./scripts/hot-reload.sh                          # 仅编译+重启
#   ./scripts/hot-reload.sh --claude "fix prompt"    # Claude Code 修 bug + 编译 + 重启
#   ./scripts/hot-reload.sh --build-only             # 仅编译，不重启
#   ./scripts/hot-reload.sh --restart-only           # 仅重启，不编译

set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
BINARY="$PROJECT_DIR/target/release/rayclaw"
BACKUP="$PROJECT_DIR/target/release/rayclaw.bak"
LOG="/tmp/rayclaw-reload.log"
VERSION_LOG="/tmp/rayclaw-versions.log"

MODE="full"  # full | build-only | restart-only | claude

log() {
    echo "$(date '+%Y-%m-%d %H:%M:%S') $1" | tee -a "$LOG"
}

log "=== Hot-reload started ==="

# 解析参数
CLAUDE_PROMPT=""
while [[ $# -gt 0 ]]; do
    case $1 in
        --claude)
            MODE="claude"
            CLAUDE_PROMPT="$2"
            shift 2
            ;;
        --build-only)
            MODE="build-only"
            shift
            ;;
        --restart-only)
            MODE="restart-only"
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--claude \"prompt\"] [--build-only] [--restart-only]"
            exit 1
            ;;
    esac
done

# Step 0: Claude Code 修 bug（可选）
if [[ "$MODE" == "claude" ]]; then
    log "🤖 Running Claude Code: $CLAUDE_PROMPT"
    cd "$PROJECT_DIR"
    if claude --dangerously-skip-permissions -p "$CLAUDE_PROMPT" 2>&1 | tee -a "$LOG"; then
        log "✅ Claude Code finished"
    else
        log "❌ Claude Code failed"
        echo "CLAUDE_FAILED"
        exit 1
    fi
fi

# Step 1: 编译
if [[ "$MODE" != "restart-only" ]]; then
    # 备份当前二进制
    if [[ -f "$BINARY" ]]; then
        cp "$BINARY" "$BACKUP"
        log "📦 Backed up current binary"
    fi

    log "🔨 Building release..."
    cd "$PROJECT_DIR"
    if cargo build --release 2>&1 | tee -a "$LOG"; then
        NEW_SIZE=$(ls -lh "$BINARY" | awk '{print $5}')
        log "✅ Build succeeded ($NEW_SIZE)"
    else
        log "❌ Build failed! Restoring backup."
        if [[ -f "$BACKUP" ]]; then
            cp "$BACKUP" "$BINARY"
            log "♻️ Backup restored"
        fi
        echo "BUILD_FAILED"
        exit 2
    fi

    # 记录版本
    cd "$PROJECT_DIR"
    GIT_INFO=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
    GIT_MSG=$(git log -1 --oneline 2>/dev/null || echo "no git info")
    echo "$(date '+%Y-%m-%d %H:%M:%S') | $GIT_INFO | $GIT_MSG" >> "$VERSION_LOG"
fi

if [[ "$MODE" == "build-only" ]]; then
    log "✅ Build-only mode, skipping restart"
    echo "BUILD_SUCCESS"
    exit 0
fi

# Step 2: 重启服务
log "🔄 Restarting rayclaw service..."
if sudo systemctl restart rayclaw; then
    log "✅ systemctl restart succeeded"
else
    log "❌ systemctl restart failed"
    echo "RESTART_FAILED"
    exit 3
fi

# Step 3: 等待服务就绪
log "⏳ Waiting for service to be ready..."
sleep 4

if sudo systemctl is-active --quiet rayclaw; then
    PID=$(systemctl show rayclaw --property=MainPID --value)
    log "✅ RayClaw restarted successfully (PID: $PID)"
    echo "RELOAD_SUCCESS"
else
    log "❌ Service not active after restart"
    # 查看最近日志
    tail -20 /tmp/rayclaw.log >> "$LOG" 2>/dev/null
    echo "RESTART_FAILED"
    exit 4
fi
