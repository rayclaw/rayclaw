#!/usr/bin/env python3
"""Mock ACP agent for integration tests.

Reads JSON-RPC requests from stdin (one per line) and writes
JSON-RPC responses to stdout. Supports the minimal ACP protocol
flow: initialize → session/new → session/prompt → session/end → shutdown.

Uses the real ACP protocol:
- session/prompt (not prompt/start)
- session/update notifications (not messages/create)
- session/request_permission as a request (not approval/request notification)

Usage modes (set via ACP_MOCK_MODE env var):
  "normal"  (default) — completes all requests successfully
  "slow"    — adds a 5s delay to session/prompt (for timeout tests)
  "error"   — returns an error on session/prompt
"""
import json
import os
import sys
import time


def write_response(obj):
    """Write a JSON-RPC response line to stdout."""
    line = json.dumps(obj) + "\n"
    sys.stdout.write(line)
    sys.stdout.flush()


def write_notification(method, params=None):
    """Write a JSON-RPC notification to stdout."""
    msg = {"jsonrpc": "2.0", "method": method}
    if params is not None:
        msg["params"] = params
    line = json.dumps(msg) + "\n"
    sys.stdout.write(line)
    sys.stdout.flush()


def handle_request(req, mode):
    """Process a single JSON-RPC request and write response(s)."""
    method = req.get("method", "")
    req_id = req.get("id")
    params = req.get("params", {})

    if method == "initialize":
        write_response({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "protocolVersion": 1,
                "capabilities": {
                    "prompts": True,
                    "sessions": True,
                },
                "serverInfo": {
                    "name": "mock-acp-agent",
                    "version": "0.1.0",
                },
            },
        })

    elif method == "session/new":
        write_response({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "sessionId": "mock-session-001",
            },
        })

    elif method == "session/prompt":
        if mode == "error":
            write_response({
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {
                    "code": -32000,
                    "message": "Mock error: prompt execution failed",
                },
            })
            return

        if mode == "slow":
            time.sleep(5)

        # Extract message from prompt content blocks
        prompt = params.get("prompt", [])
        message = ""
        for block in prompt:
            if block.get("type") == "text":
                message = block.get("text", "")
                break
        if not message:
            message = params.get("message", "")

        session_id = params.get("sessionId", "mock-session-001")

        # Emit session/update notifications before the final response
        write_notification("session/update", {
            "sessionId": session_id,
            "update": {
                "type": "AgentMessageChunk",
                "content": {
                    "type": "text",
                    "text": f"Working on: {message}",
                },
            },
        })
        write_notification("session/update", {
            "sessionId": session_id,
            "update": {
                "type": "ToolCall",
                "toolCallId": "tc-001",
                "title": "bash",
                "kind": "command",
                "rawInput": {"command": "echo hello"},
                "content": [],
            },
        })

        # Final response
        write_response({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "stopReason": "end_turn",
            },
        })

    elif method == "session/end":
        write_response({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {"status": "ended"},
        })

    elif method == "shutdown":
        write_response({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {"status": "shutting_down"},
        })

    else:
        # Unknown method — return error
        if req_id is not None:
            write_response({
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {
                    "code": -32601,
                    "message": f"Method not found: {method}",
                },
            })


def main():
    mode = os.environ.get("ACP_MOCK_MODE", "normal")
    # Write startup message to stderr (for debugging)
    print(f"[mock-acp-agent] started in mode={mode}", file=sys.stderr)

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue

        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            continue

        handle_request(req, mode)

        # Exit after shutdown
        if req.get("method") == "shutdown":
            break


if __name__ == "__main__":
    main()
