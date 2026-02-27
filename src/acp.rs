//! ACP (Agent Client Protocol) integration for RayClaw.
//!
//! Allows RayClaw to spawn and control external Coding Agents
//! (e.g. Claude Code) as subprocesses via the ACP JSON-RPC protocol.
//!
//! MVP scope: Claude Code support only, stdio transport.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Config types — loaded from <data_root>/acp.json
// ---------------------------------------------------------------------------

fn default_prompt_timeout_secs() -> u64 {
    300
}

fn default_launch() -> String {
    "npx".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct AcpAgentConfig {
    /// Launch method: "npx" | "binary" | "uvx"
    #[serde(default = "default_launch")]
    pub launch: String,

    /// Executable or package name.
    /// npx: package spec (e.g. "@anthropic-ai/claude-code@latest")
    /// binary: absolute path to executable
    pub command: String,

    #[serde(default)]
    pub args: Vec<String>,

    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Default working directory for this agent
    #[serde(default)]
    pub workspace: Option<String>,

    /// Override the global auto_approve setting for this agent
    #[serde(default)]
    pub auto_approve: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct AcpConfig {
    /// Automatically approve tool calls from agents
    #[serde(default, alias = "defaultAutoApprove")]
    pub default_auto_approve: bool,

    /// Prompt execution timeout in seconds
    #[serde(default = "default_prompt_timeout_secs", alias = "promptTimeoutSecs")]
    pub prompt_timeout_secs: u64,

    /// Configured agents, keyed by name (e.g. "claude", "opencode")
    #[serde(default, alias = "acpAgents")]
    pub agents: HashMap<String, AcpAgentConfig>,
}

impl Default for AcpConfig {
    fn default() -> Self {
        AcpConfig {
            default_auto_approve: false,
            prompt_timeout_secs: default_prompt_timeout_secs(),
            agents: HashMap::new(),
        }
    }
}

impl AcpConfig {
    /// Load config from a JSON file. Returns default (empty) config on
    /// missing file or parse error — ACP is optional, same as MCP.
    pub fn from_file(path: &str) -> Self {
        let config_str = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return AcpConfig::default(),
        };

        match serde_json::from_str(&config_str) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to parse ACP config {path}: {e}");
                AcpConfig::default()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 types (shared with connection layer)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcMessage {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<serde_json::Value>,
    method: Option<String>,
    params: Option<serde_json::Value>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

impl JsonRpcMessage {
    /// True if this message is a response (has id + result/error, no method)
    fn is_response(&self) -> bool {
        self.id.is_some() && self.method.is_none()
    }

    /// True if this message is a notification (has method, no id)
    fn is_notification(&self) -> bool {
        self.method.is_some() && self.id.is_none()
    }

    /// True if this message is a request from the agent (has both id AND method).
    /// e.g. session/request_permission
    fn is_request(&self) -> bool {
        self.id.is_some() && self.method.is_some()
    }
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

// ---------------------------------------------------------------------------
// ACP Connection — stdio transport to a single agent process
// ---------------------------------------------------------------------------

const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;
const ACP_PROTOCOL_VERSION: u32 = 1;

struct AcpConnectionInner {
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    _child: Child,
    next_id: u64,
}

/// A connection to a single ACP agent process over stdio JSON-RPC.
pub struct AcpConnection {
    agent_name: String,
    inner: Mutex<AcpConnectionInner>,
    request_timeout: Duration,
}

/// Build the OS command for spawning an agent process.
fn build_spawn_command(config: &AcpAgentConfig, workspace: Option<&str>) -> Command {
    let (program, base_args): (&str, Vec<&str>) = match config.launch.as_str() {
        "npx" => ("npx", vec!["-y", &config.command]),
        "uvx" => ("uvx", vec![&config.command]),
        _ => (&config.command, vec![]),
    };

    let mut cmd = Command::new(program);
    for arg in &base_args {
        cmd.arg(arg);
    }
    for arg in &config.args {
        cmd.arg(arg);
    }
    // Remove environment variables that cause nested-session detection in
    // Claude Code.  When RayClaw itself runs inside a Claude Code session
    // (e.g. as a tool), these vars are inherited and the ACP agent refuses
    // to start.
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
    cmd.env_remove("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS");
    cmd.envs(&config.env);

    if let Some(ws) = workspace.or(config.workspace.as_deref()) {
        cmd.current_dir(ws);
    }

    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    cmd
}

impl AcpConnection {
    /// Spawn an agent process and perform the ACP initialization handshake.
    pub async fn spawn(
        agent_name: &str,
        config: &AcpAgentConfig,
        workspace: Option<&str>,
        request_timeout: Duration,
    ) -> Result<Self, String> {
        let mut cmd = build_spawn_command(config, workspace);

        info!(
            "ACP: spawning agent '{agent_name}' ({} {})",
            config.launch, config.command
        );

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn ACP agent '{agent_name}': {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("ACP agent '{agent_name}': failed to capture stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("ACP agent '{agent_name}': failed to capture stdout"))?;

        // Spawn a task to drain stderr to tracing::debug
        if let Some(stderr) = child.stderr.take() {
            let name = agent_name.to_string();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let trimmed = line.trim();
                            if !trimmed.is_empty() {
                                debug!("ACP [{name}] stderr: {trimmed}");
                            }
                        }
                    }
                }
            });
        }

        let conn = AcpConnection {
            agent_name: agent_name.to_string(),
            inner: Mutex::new(AcpConnectionInner {
                stdin,
                stdout: BufReader::new(stdout),
                _child: child,
                next_id: 1,
            }),
            request_timeout,
        };

        // Perform initialization handshake
        conn.initialize().await?;

        Ok(conn)
    }

    /// Send the `initialize` request and `notifications/initialized` notification.
    async fn initialize(&self) -> Result<(), String> {
        let params = serde_json::json!({
            "protocolVersion": ACP_PROTOCOL_VERSION,
            "clientCapabilities": {
                "fs": {
                    "readTextFile": false,
                    "writeTextFile": false
                },
                "terminal": false
            },
            "clientInfo": {
                "name": "rayclaw",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let result = self.send_request("initialize", Some(params)).await?;

        let server_version = result
            .get("protocolVersion")
            .map(|v| match v {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_else(|| "unknown".to_string());
        let server_name = result
            .get("serverInfo")
            .or_else(|| result.get("agentInfo"))
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        info!(
            "ACP [{}]: initialized (agent={server_name}, protocol={server_version})",
            self.agent_name
        );

        // Send the notifications/initialized notification (ACP spec).
        // Some agents (e.g. Zed claude-agent-acp) don't implement this
        // notification and return Method-not-found; that's harmless — just log it.
        if let Err(e) = self
            .send_notification("notifications/initialized", None)
            .await
        {
            debug!(
                "ACP [{}]: notifications/initialized not supported ({e}), continuing",
                self.agent_name
            );
        }

        Ok(())
    }

    /// Send a JSON-RPC request and wait for the matching response.
    /// Notifications received while waiting are logged and discarded.
    pub async fn send_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let mut inner = self.inner.lock().await;
        let id = inner.next_id;
        inner.next_id += 1;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: method.to_string(),
            params,
        };

        let mut json = serde_json::to_string(&request).map_err(|e| e.to_string())?;
        json.push('\n');

        inner
            .stdin
            .write_all(json.as_bytes())
            .await
            .map_err(|e| format!("ACP [{}] write error: {e}", self.agent_name))?;
        inner
            .stdin
            .flush()
            .await
            .map_err(|e| format!("ACP [{}] flush error: {e}", self.agent_name))?;

        // Read lines until we get the matching response
        let deadline = tokio::time::Instant::now() + self.request_timeout;
        let mut line = String::new();

        loop {
            line.clear();
            let read_result =
                tokio::time::timeout_at(deadline, inner.stdout.read_line(&mut line)).await;

            match read_result {
                Err(_) => {
                    return Err(format!(
                        "ACP [{}] request '{}' timed out ({:?})",
                        self.agent_name, method, self.request_timeout
                    ));
                }
                Ok(Err(e)) => {
                    return Err(format!("ACP [{}] read error: {e}", self.agent_name));
                }
                Ok(Ok(0)) => {
                    return Err(format!("ACP [{}] agent closed connection", self.agent_name));
                }
                Ok(Ok(_)) => {}
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let msg: JsonRpcMessage = match serde_json::from_str(trimmed) {
                Ok(m) => m,
                Err(_) => {
                    debug!(
                        "ACP [{}] ignoring non-JSON line: {}",
                        self.agent_name,
                        &trimmed[..trimmed.len().min(200)]
                    );
                    continue;
                }
            };

            if msg.is_notification() {
                // Discard notifications during simple request/response
                debug!(
                    "ACP [{}] notification during '{}': {:?}",
                    self.agent_name, method, msg.method
                );
                continue;
            }

            if msg.is_response() {
                let matches = match &msg.id {
                    Some(serde_json::Value::Number(n)) => n.as_u64() == Some(id),
                    _ => true, // best effort
                };
                if !matches {
                    continue;
                }
                if let Some(err) = msg.error {
                    return Err(format!(
                        "ACP [{}] error ({}): {}",
                        self.agent_name, err.code, err.message
                    ));
                }
                return Ok(msg.result.unwrap_or(serde_json::Value::Null));
            }
        }
    }

    /// Send a JSON-RPC notification (no response expected).
    pub async fn send_notification(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: method.to_string(),
            params,
        };
        let mut json = serde_json::to_string(&request).map_err(|e| e.to_string())?;
        json.push('\n');
        inner
            .stdin
            .write_all(json.as_bytes())
            .await
            .map_err(|e| format!("ACP [{}] write error: {e}", self.agent_name))?;
        inner
            .stdin
            .flush()
            .await
            .map_err(|e| format!("ACP [{}] flush error: {e}", self.agent_name))?;
        Ok(())
    }

    /// Send `session/prompt` and collect the notification stream until the
    /// response arrives. During execution, permission requests are auto-resolved
    /// according to `auto_approve`. Returns `AcpPromptResult` with all
    /// collected messages, tool calls, and file changes.
    pub async fn prompt_streaming(
        &self,
        params: serde_json::Value,
        auto_approve: bool,
        timeout: Duration,
    ) -> Result<AcpPromptResult, String> {
        let started = std::time::Instant::now();
        let mut inner = self.inner.lock().await;
        let id = inner.next_id;
        inner.next_id += 1;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: "session/prompt".to_string(),
            params: Some(params),
        };
        let mut json = serde_json::to_string(&request).map_err(|e| e.to_string())?;
        json.push('\n');

        inner
            .stdin
            .write_all(json.as_bytes())
            .await
            .map_err(|e| format!("ACP [{}] write error: {e}", self.agent_name))?;
        inner
            .stdin
            .flush()
            .await
            .map_err(|e| format!("ACP [{}] flush error: {e}", self.agent_name))?;

        let mut result = AcpPromptResult {
            messages: Vec::new(),
            tool_calls: Vec::new(),
            files_changed: Vec::new(),
            completed: false,
            duration_ms: 0,
        };
        // Buffer for accumulating streamed message chunks
        let mut message_buffer = String::new();

        let deadline = tokio::time::Instant::now() + timeout;
        let mut line = String::new();

        loop {
            line.clear();
            let read_result =
                tokio::time::timeout_at(deadline, inner.stdout.read_line(&mut line)).await;

            match read_result {
                Err(_) => {
                    result.duration_ms = started.elapsed().as_millis();
                    return Err(format!(
                        "ACP [{}] prompt timed out after {timeout:?}",
                        self.agent_name
                    ));
                }
                Ok(Err(e)) => {
                    result.duration_ms = started.elapsed().as_millis();
                    return Err(format!("ACP [{}] read error: {e}", self.agent_name));
                }
                Ok(Ok(0)) => {
                    result.duration_ms = started.elapsed().as_millis();
                    return Err(format!(
                        "ACP [{}] agent closed connection during prompt",
                        self.agent_name
                    ));
                }
                Ok(Ok(_)) => {}
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let msg: JsonRpcMessage = match serde_json::from_str(trimmed) {
                Ok(m) => m,
                Err(_) => {
                    debug!(
                        "ACP [{}] ignoring non-JSON: {}",
                        self.agent_name,
                        &trimmed[..trimmed.len().min(200)]
                    );
                    continue;
                }
            };

            // Handle the final response to our session/prompt request
            if msg.is_response() {
                let matches = match &msg.id {
                    Some(serde_json::Value::Number(n)) => n.as_u64() == Some(id),
                    _ => true,
                };
                if !matches {
                    continue;
                }
                if let Some(err) = msg.error {
                    result.duration_ms = started.elapsed().as_millis();
                    return Err(format!(
                        "ACP [{}] prompt error ({}): {}",
                        self.agent_name, err.code, err.message
                    ));
                }

                // Flush any remaining message buffer
                if !message_buffer.is_empty() {
                    result.messages.push(std::mem::take(&mut message_buffer));
                }

                // Extract stopReason from response if available
                if let Some(res) = &msg.result {
                    if let Some(reason) = res.get("stopReason").and_then(|v| v.as_str()) {
                        debug!("ACP [{}] prompt stopReason: {reason}", self.agent_name);
                    }
                }

                result.completed = true;
                result.duration_ms = started.elapsed().as_millis();
                return Ok(result);
            }

            // Handle requests from agent (e.g. session/request_permission)
            if msg.is_request() {
                let method = msg.method.as_deref().unwrap_or("");
                let request_id = &msg.id;
                info!(
                    "ACP [{}] agent request: method={method} params={}",
                    self.agent_name,
                    msg.params
                        .as_ref()
                        .map(|p| {
                            let s = p.to_string();
                            s[..s.len().min(300)].to_string()
                        })
                        .unwrap_or_default()
                );

                if method == "session/request_permission" {
                    // Permission request: agent wants approval for a tool call
                    let params = msg.params.as_ref();
                    let options = params
                        .and_then(|p| p.get("options"))
                        .and_then(|o| o.as_array());
                    // Find an "allow" option (prefer allow_always, then allow_once)
                    let allow_option_id = options
                        .and_then(|arr| {
                            arr.iter()
                                .find(|opt| {
                                    opt.get("kind")
                                        .and_then(|k| k.as_str())
                                        .map(|k| k == "allow_always")
                                        .unwrap_or(false)
                                })
                                .or_else(|| {
                                    arr.iter().find(|opt| {
                                        opt.get("kind")
                                            .and_then(|k| k.as_str())
                                            .map(|k| k.starts_with("allow"))
                                            .unwrap_or(false)
                                    })
                                })
                        })
                        .and_then(|opt| opt.get("optionId"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("allow");

                    if auto_approve {
                        // Send JSON-RPC response approving the permission
                        let response = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": request_id,
                            "result": {
                                "outcome": {
                                    "outcome": "selected",
                                    "optionId": allow_option_id
                                }
                            }
                        });
                        let mut resp_json = serde_json::to_string(&response).unwrap_or_default();
                        resp_json.push('\n');
                        let _ = inner.stdin.write_all(resp_json.as_bytes()).await;
                        let _ = inner.stdin.flush().await;
                        info!(
                            "ACP [{}] auto-approved permission (optionId={})",
                            self.agent_name, allow_option_id
                        );
                    } else {
                        // Reject by sending cancelled outcome
                        let response = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": request_id,
                            "result": {
                                "outcome": {
                                    "outcome": "cancelled"
                                }
                            }
                        });
                        let mut resp_json = serde_json::to_string(&response).unwrap_or_default();
                        resp_json.push('\n');
                        let _ = inner.stdin.write_all(resp_json.as_bytes()).await;
                        let _ = inner.stdin.flush().await;
                        debug!(
                            "ACP [{}] rejected permission request (auto_approve=false)",
                            self.agent_name
                        );
                    }
                } else {
                    debug!(
                        "ACP [{}] unhandled agent request: {method}",
                        self.agent_name
                    );
                }
                continue;
            }

            // Handle notifications (session/update)
            if msg.is_notification() {
                let method = msg.method.as_deref().unwrap_or("");
                let params = msg.params.as_ref();

                match method {
                    "session/update" => {
                        // Parse the update type from params.update.sessionUpdate
                        let update = params.and_then(|p| p.get("update"));
                        let update_type = update
                            .and_then(|u| u.get("sessionUpdate"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("");

                        match update_type {
                            "agent_message_chunk" => {
                                // Extract text from content block
                                let text = update
                                    .and_then(|u| u.get("content"))
                                    .and_then(|c| c.get("text"))
                                    .and_then(|t| t.as_str());
                                if let Some(text) = text {
                                    message_buffer.push_str(text);
                                }
                            }
                            "agent_thought_chunk" => {
                                // Agent thinking — log but don't include in output
                                let text = update
                                    .and_then(|u| u.get("content"))
                                    .and_then(|c| c.get("text"))
                                    .and_then(|t| t.as_str());
                                if let Some(text) = text {
                                    debug!(
                                        "ACP [{}] thought: {}",
                                        self.agent_name,
                                        &text[..text.len().min(100)]
                                    );
                                }
                            }
                            "tool_call" => {
                                let title = update
                                    .and_then(|u| u.get("title"))
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("unknown")
                                    .to_string();
                                let raw_input = update
                                    .and_then(|u| u.get("rawInput"))
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null);
                                result.tool_calls.push(ToolCallInfo {
                                    name: title,
                                    input: raw_input,
                                });
                                // Flush message buffer before tool calls
                                if !message_buffer.is_empty() {
                                    result.messages.push(std::mem::take(&mut message_buffer));
                                }
                            }
                            "tool_call_update" => {
                                let tool_id = update
                                    .and_then(|u| u.get("toolCallId"))
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("?");
                                let status = update
                                    .and_then(|u| u.get("status"))
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("?");
                                debug!(
                                    "ACP [{}] tool update: id={tool_id} status={status}",
                                    self.agent_name
                                );
                                // Capture rawOutput (e.g. command stdout)
                                if let Some(raw) = update.and_then(|u| u.get("rawOutput")) {
                                    let output_str = match raw {
                                        serde_json::Value::String(s) => s.clone(),
                                        other => other.to_string(),
                                    };
                                    if !output_str.is_empty() {
                                        result.messages.push(output_str);
                                    }
                                }
                                // Capture content blocks (terminal output, diffs, etc.)
                                if let Some(content_arr) = update
                                    .and_then(|u| u.get("content"))
                                    .and_then(|c| c.as_array())
                                {
                                    for item in content_arr {
                                        let content_type =
                                            item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                        if content_type == "content" {
                                            // Inline text content
                                            if let Some(text) = item
                                                .get("content")
                                                .and_then(|c| c.get("text"))
                                                .and_then(|t| t.as_str())
                                            {
                                                if !text.is_empty() {
                                                    result.messages.push(text.to_string());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            "plan" => {
                                let entries = update
                                    .and_then(|u| u.get("entries"))
                                    .and_then(|e| e.as_array());
                                if let Some(entries) = entries {
                                    debug!(
                                        "ACP [{}] plan update: {} entries",
                                        self.agent_name,
                                        entries.len()
                                    );
                                }
                            }
                            _ => {
                                debug!(
                                    "ACP [{}] unhandled session/update type: {update_type}",
                                    self.agent_name
                                );
                            }
                        }
                    }
                    _ => {
                        debug!("ACP [{}] unhandled notification: {method}", self.agent_name);
                    }
                }
            }
        }
    }

    /// Gracefully shut down the agent process.
    pub async fn shutdown(&self) -> Result<(), String> {
        info!("ACP [{}]: shutting down", self.agent_name);

        // Try sending session/end (best effort)
        let _ = self.send_request("shutdown", None).await;

        // Kill the child process
        let mut inner = self.inner.lock().await;
        let _ = inner._child.kill().await;
        info!("ACP [{}]: process terminated", self.agent_name);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Session & prompt result types
// ---------------------------------------------------------------------------

/// Summary info returned after creating a session
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub agent_id: String,
    pub workspace: String,
}

/// Status of an ACP session
#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    Active,
    Prompting,
    Ended,
}

/// Record of a single tool call made by the agent during prompt execution
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub name: String,
    pub input: serde_json::Value,
}

/// Result of an ACP prompt execution
#[derive(Debug, Clone)]
pub struct AcpPromptResult {
    /// Text messages emitted by the agent
    pub messages: Vec<String>,
    /// Tool calls executed by the agent
    pub tool_calls: Vec<ToolCallInfo>,
    /// Files changed during execution
    pub files_changed: Vec<String>,
    /// Whether the prompt completed normally (vs timeout/cancel)
    pub completed: bool,
    /// Wall-clock execution time in milliseconds
    pub duration_ms: u128,
}

/// An active ACP agent session with its connection
pub struct AcpSession {
    pub id: String,
    pub agent_id: String,
    pub workspace: String,
    pub auto_approve: bool,
    pub status: SessionStatus,
    pub acp_session_id: Option<String>,
    pub connection: AcpConnection,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ---------------------------------------------------------------------------
// AcpManager — global session lifecycle manager
// ---------------------------------------------------------------------------

pub struct AcpManager {
    pub config: AcpConfig,
    sessions: RwLock<HashMap<String, Mutex<AcpSession>>>,
    /// Map chat_id → session_id for command-based ACP routing
    chat_sessions: RwLock<HashMap<i64, String>>,
}

impl AcpManager {
    /// Create an AcpManager from a config file path.
    /// Does NOT spawn any agents — they are created on demand via tools.
    pub fn from_config_file(path: &str) -> Self {
        Self::from_config(AcpConfig::from_file(path))
    }

    /// Create an AcpManager from an already-parsed config.
    /// Does NOT spawn any agents — they are created on demand via tools.
    pub fn from_config(config: AcpConfig) -> Self {
        if !config.agents.is_empty() {
            info!(
                "ACP config loaded: {} agent(s) configured ({})",
                config.agents.len(),
                config.agents.keys().cloned().collect::<Vec<_>>().join(", ")
            );
        }
        AcpManager {
            config,
            sessions: RwLock::new(HashMap::new()),
            chat_sessions: RwLock::new(HashMap::new()),
        }
    }

    /// List configured agent names
    pub fn available_agents(&self) -> Vec<String> {
        self.config.agents.keys().cloned().collect()
    }

    /// Check if a given agent name is configured
    pub fn has_agent(&self, name: &str) -> bool {
        self.config.agents.contains_key(name)
    }

    /// Get agent config by name
    pub fn agent_config(&self, name: &str) -> Option<&AcpAgentConfig> {
        self.config.agents.get(name)
    }

    /// Spawn a new agent process, perform ACP handshake, and create a session.
    pub async fn new_session(
        &self,
        agent_id: &str,
        workspace: Option<&str>,
        auto_approve: Option<bool>,
    ) -> Result<SessionInfo, String> {
        let agent_config = self
            .config
            .agents
            .get(agent_id)
            .ok_or_else(|| format!("ACP agent '{agent_id}' not configured"))?
            .clone();

        let effective_auto_approve = auto_approve
            .or(agent_config.auto_approve)
            .unwrap_or(self.config.default_auto_approve);

        let effective_workspace = workspace
            .map(|s| s.to_string())
            .or_else(|| agent_config.workspace.clone())
            .unwrap_or_else(|| ".".to_string());

        let request_timeout = Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS);
        let connection = AcpConnection::spawn(
            agent_id,
            &agent_config,
            Some(&effective_workspace),
            request_timeout,
        )
        .await?;

        // Create an ACP-level session with workspace as cwd
        let cwd = std::path::Path::new(&effective_workspace)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(&effective_workspace));
        let acp_session_id = match connection
            .send_request(
                "session/new",
                Some(serde_json::json!({
                    "cwd": cwd.to_string_lossy(),
                    "mcpServers": []
                })),
            )
            .await
        {
            Ok(result) => result
                .get("sessionId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            Err(e) => {
                warn!(
                    "ACP [{}]: session/new failed ({e}), continuing without ACP session ID",
                    agent_id
                );
                None
            }
        };

        let session_id = uuid::Uuid::new_v4().to_string();
        let info = SessionInfo {
            session_id: session_id.clone(),
            agent_id: agent_id.to_string(),
            workspace: effective_workspace.clone(),
        };

        let session = AcpSession {
            id: session_id.clone(),
            agent_id: agent_id.to_string(),
            workspace: effective_workspace,
            auto_approve: effective_auto_approve,
            status: SessionStatus::Active,
            acp_session_id,
            connection,
            created_at: chrono::Utc::now(),
        };

        self.sessions
            .write()
            .await
            .insert(session_id, Mutex::new(session));

        info!(
            "ACP session created: {} (agent={agent_id}, auto_approve={effective_auto_approve})",
            info.session_id
        );
        Ok(info)
    }

    /// Send a prompt to an existing session and wait for completion.
    pub async fn prompt(
        &self,
        session_id: &str,
        message: &str,
        timeout_secs: Option<u64>,
    ) -> Result<AcpPromptResult, String> {
        let sessions = self.sessions.read().await;
        let session_mutex = sessions
            .get(session_id)
            .ok_or_else(|| format!("ACP session '{session_id}' not found"))?;

        let mut session = session_mutex.lock().await;
        if session.status == SessionStatus::Ended {
            return Err(format!("ACP session '{session_id}' has ended"));
        }
        session.status = SessionStatus::Prompting;

        let timeout = Duration::from_secs(timeout_secs.unwrap_or(self.config.prompt_timeout_secs));

        let acp_sid = session
            .acp_session_id
            .as_deref()
            .ok_or_else(|| format!("ACP session '{session_id}' has no ACP session ID"))?;
        let params = serde_json::json!({
            "sessionId": acp_sid,
            "prompt": [{"type": "text", "text": message}]
        });

        let result = session
            .connection
            .prompt_streaming(params, session.auto_approve, timeout)
            .await;

        session.status = SessionStatus::Active;

        match result {
            Ok(r) => {
                info!(
                    "ACP [{}] prompt completed in {}ms ({} messages, {} tool calls, {} files)",
                    session.agent_id,
                    r.duration_ms,
                    r.messages.len(),
                    r.tool_calls.len(),
                    r.files_changed.len()
                );
                Ok(r)
            }
            Err(e) => {
                error!("ACP [{}] prompt failed: {e}", session.agent_id);
                Err(e)
            }
        }
    }

    /// End a session and terminate the agent process.
    pub async fn end_session(&self, session_id: &str) -> Result<(), String> {
        let session_mutex = {
            let mut sessions = self.sessions.write().await;
            sessions
                .remove(session_id)
                .ok_or_else(|| format!("ACP session '{session_id}' not found"))?
        };

        let mut session = session_mutex.lock().await;

        // Send session/end to agent (best effort)
        if let Some(acp_sid) = &session.acp_session_id {
            let _ = session
                .connection
                .send_request(
                    "session/end",
                    Some(serde_json::json!({"sessionId": acp_sid})),
                )
                .await;
        }

        session.connection.shutdown().await?;
        session.status = SessionStatus::Ended;

        // Unbind any chats referencing this session
        let mut chat_sessions = self.chat_sessions.write().await;
        chat_sessions.retain(|_, sid| sid != session_id);

        info!("ACP session ended: {session_id}");
        Ok(())
    }

    /// List all active sessions.
    pub async fn list_sessions(&self) -> Vec<SessionSummary> {
        let sessions = self.sessions.read().await;
        let mut summaries = Vec::new();
        for (id, session_mutex) in sessions.iter() {
            let session = session_mutex.lock().await;
            summaries.push(SessionSummary {
                session_id: id.clone(),
                agent_id: session.agent_id.clone(),
                workspace: session.workspace.clone(),
                status: session.status.clone(),
                created_at: session.created_at.to_rfc3339(),
            });
        }
        summaries
    }

    // -----------------------------------------------------------------------
    // Chat-to-session binding (for command-based ACP)
    // -----------------------------------------------------------------------

    /// Bind a chat to an ACP session. Messages in this chat will be routed
    /// to the ACP agent instead of the LLM.
    pub async fn bind_chat(&self, chat_id: i64, session_id: &str) {
        self.chat_sessions
            .write()
            .await
            .insert(chat_id, session_id.to_string());
        debug!("ACP: bound chat {chat_id} to session {session_id}");
    }

    /// Unbind a chat from its ACP session.
    pub async fn unbind_chat(&self, chat_id: i64) {
        self.chat_sessions.write().await.remove(&chat_id);
        debug!("ACP: unbound chat {chat_id}");
    }

    /// Get the session_id bound to a chat, if any.
    pub async fn chat_session(&self, chat_id: i64) -> Option<String> {
        self.chat_sessions.read().await.get(&chat_id).cloned()
    }

    /// End the session bound to a chat and unbind it. Returns Ok if a session
    /// existed and was ended, Err if no session was bound.
    pub async fn end_chat_session(&self, chat_id: i64) -> Result<(), String> {
        let session_id = self
            .chat_sessions
            .read()
            .await
            .get(&chat_id)
            .cloned()
            .ok_or_else(|| "No active ACP session in this chat".to_string())?;

        self.end_session(&session_id).await?;
        self.unbind_chat(chat_id).await;
        Ok(())
    }

    /// Cleanup all sessions (called on process shutdown).
    pub async fn cleanup(&self) {
        let session_ids: Vec<String> = {
            let sessions = self.sessions.read().await;
            sessions.keys().cloned().collect()
        };

        for id in &session_ids {
            if let Err(e) = self.end_session(id).await {
                warn!("ACP cleanup: failed to end session {id}: {e}");
            }
        }

        if !session_ids.is_empty() {
            info!(
                "ACP manager cleanup: terminated {} session(s)",
                session_ids.len()
            );
        }
    }
}

/// Summary of an active session (for listing)
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub agent_id: String,
    pub workspace: String,
    pub status: SessionStatus,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_config_defaults() {
        let config = AcpConfig::default();
        assert!(!config.default_auto_approve);
        assert_eq!(config.prompt_timeout_secs, 300);
        assert!(config.agents.is_empty());
    }

    #[test]
    fn test_config_parse_full() {
        let json = r#"{
            "defaultAutoApprove": true,
            "promptTimeoutSecs": 600,
            "acpAgents": {
                "claude": {
                    "launch": "npx",
                    "command": "@anthropic-ai/claude-code@latest",
                    "args": ["--acp"],
                    "env": {"ANTHROPIC_API_KEY": "sk-test"},
                    "workspace": "/tmp/test",
                    "auto_approve": true
                }
            }
        }"#;

        let config: AcpConfig = serde_json::from_str(json).unwrap();
        assert!(config.default_auto_approve);
        assert_eq!(config.prompt_timeout_secs, 600);
        assert_eq!(config.agents.len(), 1);

        let claude = config.agents.get("claude").unwrap();
        assert_eq!(claude.launch, "npx");
        assert_eq!(claude.command, "@anthropic-ai/claude-code@latest");
        assert_eq!(claude.args, vec!["--acp"]);
        assert_eq!(claude.workspace.as_deref(), Some("/tmp/test"));
        assert_eq!(claude.auto_approve, Some(true));
    }

    #[test]
    fn test_config_parse_minimal() {
        let json = r#"{
            "acpAgents": {
                "claude": {
                    "command": "@anthropic-ai/claude-code@latest"
                }
            }
        }"#;

        let config: AcpConfig = serde_json::from_str(json).unwrap();
        assert!(!config.default_auto_approve);
        assert_eq!(config.prompt_timeout_secs, 300);

        let claude = config.agents.get("claude").unwrap();
        assert_eq!(claude.launch, "npx");
        assert!(claude.args.is_empty());
        assert!(claude.env.is_empty());
        assert!(claude.workspace.is_none());
        assert!(claude.auto_approve.is_none());
    }

    #[test]
    fn test_config_parse_snake_case_aliases() {
        let json = r#"{
            "default_auto_approve": true,
            "prompt_timeout_secs": 120,
            "agents": {
                "claude": {
                    "command": "@anthropic-ai/claude-code@latest"
                }
            }
        }"#;

        let config: AcpConfig = serde_json::from_str(json).unwrap();
        assert!(config.default_auto_approve);
        assert_eq!(config.prompt_timeout_secs, 120);
        assert_eq!(config.agents.len(), 1);
    }

    #[test]
    fn test_missing_file_returns_default() {
        let config = AcpConfig::from_file("/nonexistent/acp.json");
        assert!(config.agents.is_empty());
    }

    #[test]
    fn test_manager_from_config() {
        let manager = AcpManager::from_config_file("/nonexistent/acp.json");
        assert!(manager.available_agents().is_empty());
        assert!(!manager.has_agent("claude"));
    }

    #[test]
    fn test_build_spawn_command_npx() {
        let config = AcpAgentConfig {
            launch: "npx".to_string(),
            command: "@anthropic-ai/claude-code@latest".to_string(),
            args: vec!["--acp".to_string()],
            env: HashMap::from([("FOO".to_string(), "bar".to_string())]),
            workspace: Some("/tmp/ws".to_string()),
            auto_approve: None,
        };

        let cmd = build_spawn_command(&config, None);
        let prog = cmd.as_std().get_program();
        assert_eq!(prog, "npx");

        let args: Vec<&std::ffi::OsStr> = cmd.as_std().get_args().collect();
        assert_eq!(
            args,
            vec!["-y", "@anthropic-ai/claude-code@latest", "--acp"]
        );
    }

    #[test]
    fn test_build_spawn_command_binary() {
        let config = AcpAgentConfig {
            launch: "binary".to_string(),
            command: "/usr/bin/opencode".to_string(),
            args: vec!["acp".to_string()],
            env: HashMap::new(),
            workspace: None,
            auto_approve: None,
        };

        let cmd = build_spawn_command(&config, Some("/home/user/project"));
        let prog = cmd.as_std().get_program();
        assert_eq!(prog, "/usr/bin/opencode");

        let args: Vec<&std::ffi::OsStr> = cmd.as_std().get_args().collect();
        assert_eq!(args, vec!["acp"]);

        assert_eq!(
            cmd.as_std().get_current_dir(),
            Some(std::path::Path::new("/home/user/project"))
        );
    }

    #[test]
    fn test_build_spawn_command_workspace_override() {
        let config = AcpAgentConfig {
            launch: "npx".to_string(),
            command: "agent".to_string(),
            args: vec![],
            env: HashMap::new(),
            workspace: Some("/default/ws".to_string()),
            auto_approve: None,
        };

        // Explicit workspace overrides config default
        let cmd = build_spawn_command(&config, Some("/override/ws"));
        assert_eq!(
            cmd.as_std().get_current_dir(),
            Some(std::path::Path::new("/override/ws"))
        );

        // Falls back to config default
        let cmd2 = build_spawn_command(&config, None);
        assert_eq!(
            cmd2.as_std().get_current_dir(),
            Some(std::path::Path::new("/default/ws"))
        );
    }

    #[test]
    fn test_jsonrpc_message_classification() {
        // Response
        let resp: JsonRpcMessage =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#).unwrap();
        assert!(resp.is_response());
        assert!(!resp.is_notification());

        // Error response
        let err: JsonRpcMessage = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":2,"error":{"code":-1,"message":"fail"}}"#,
        )
        .unwrap();
        assert!(err.is_response());
        assert!(!err.is_notification());

        // Notification
        let notif: JsonRpcMessage = serde_json::from_str(
            r#"{"jsonrpc":"2.0","method":"messages/create","params":{"text":"hi"}}"#,
        )
        .unwrap();
        assert!(!notif.is_response());
        assert!(notif.is_notification());
    }

    #[test]
    fn test_prompt_result_default() {
        let result = AcpPromptResult {
            messages: vec!["hello".to_string()],
            tool_calls: vec![ToolCallInfo {
                name: "bash".to_string(),
                input: serde_json::json!({"command": "ls"}),
            }],
            files_changed: vec!["foo.rs".to_string()],
            completed: true,
            duration_ms: 1234,
        };

        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "bash");
        assert!(result.completed);
        assert_eq!(result.duration_ms, 1234);
    }

    #[tokio::test]
    async fn test_manager_new_session_unknown_agent() {
        let manager = AcpManager::from_config_file("/nonexistent/acp.json");
        let result = manager.new_session("nonexistent", None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not configured"));
    }

    #[tokio::test]
    async fn test_manager_list_sessions_empty() {
        let manager = AcpManager::from_config_file("/nonexistent/acp.json");
        let sessions = manager.list_sessions().await;
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_manager_end_session_not_found() {
        let manager = AcpManager::from_config_file("/nonexistent/acp.json");
        let result = manager.end_session("nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_manager_prompt_not_found() {
        let manager = AcpManager::from_config_file("/nonexistent/acp.json");
        let result = manager.prompt("nonexistent", "hello", None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    // -----------------------------------------------------------------------
    // Phase 7.1: Additional config parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_config_parse_multiple_agents() {
        let json = r#"{
            "acpAgents": {
                "claude": {
                    "command": "@anthropic-ai/claude-code@latest",
                    "workspace": "/tmp/claude"
                },
                "opencode": {
                    "launch": "binary",
                    "command": "/usr/bin/opencode",
                    "args": ["acp"]
                },
                "gemini": {
                    "launch": "npx",
                    "command": "@google/gemini-cli@latest",
                    "args": ["--experimental-acp"],
                    "env": {"GEMINI_API_KEY": "test-key"}
                }
            }
        }"#;

        let config: AcpConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.agents.len(), 3);

        let claude = config.agents.get("claude").unwrap();
        assert_eq!(claude.launch, "npx"); // default
        assert_eq!(claude.workspace.as_deref(), Some("/tmp/claude"));

        let opencode = config.agents.get("opencode").unwrap();
        assert_eq!(opencode.launch, "binary");
        assert_eq!(opencode.command, "/usr/bin/opencode");
        assert_eq!(opencode.args, vec!["acp"]);
        assert!(opencode.workspace.is_none());

        let gemini = config.agents.get("gemini").unwrap();
        assert_eq!(gemini.launch, "npx");
        assert_eq!(gemini.env.get("GEMINI_API_KEY").unwrap(), "test-key");
    }

    #[test]
    fn test_config_parse_invalid_json_returns_default() {
        // AcpConfig::from_file should return defaults on parse failure.
        // We can't easily test from_file with bad content without a temp file,
        // but we can verify serde_json rejects garbage.
        let result: Result<AcpConfig, _> = serde_json::from_str("NOT JSON");
        assert!(result.is_err());
    }

    #[test]
    fn test_config_parse_empty_object() {
        let config: AcpConfig = serde_json::from_str("{}").unwrap();
        assert!(!config.default_auto_approve);
        assert_eq!(config.prompt_timeout_secs, 300);
        assert!(config.agents.is_empty());
    }

    #[test]
    fn test_config_parse_agent_env_propagated() {
        let config = AcpAgentConfig {
            launch: "npx".to_string(),
            command: "agent".to_string(),
            args: vec![],
            env: HashMap::from([
                ("KEY1".to_string(), "val1".to_string()),
                ("KEY2".to_string(), "val2".to_string()),
            ]),
            workspace: None,
            auto_approve: None,
        };

        let cmd = build_spawn_command(&config, None);
        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        // Verify our env vars are set (they should be among the envs)
        let has_key1 = envs.iter().any(|(k, v)| {
            k == &std::ffi::OsStr::new("KEY1") && v == &Some(std::ffi::OsStr::new("val1"))
        });
        let has_key2 = envs.iter().any(|(k, v)| {
            k == &std::ffi::OsStr::new("KEY2") && v == &Some(std::ffi::OsStr::new("val2"))
        });
        assert!(has_key1, "KEY1 env var should be set");
        assert!(has_key2, "KEY2 env var should be set");
    }

    #[test]
    fn test_build_spawn_command_uvx() {
        let config = AcpAgentConfig {
            launch: "uvx".to_string(),
            command: "some-agent".to_string(),
            args: vec!["--flag".to_string()],
            env: HashMap::new(),
            workspace: None,
            auto_approve: None,
        };

        let cmd = build_spawn_command(&config, None);
        let prog = cmd.as_std().get_program();
        assert_eq!(prog, "uvx");

        let args: Vec<&std::ffi::OsStr> = cmd.as_std().get_args().collect();
        assert_eq!(args, vec!["some-agent", "--flag"]);
    }

    #[test]
    fn test_manager_from_config_direct() {
        let config = AcpConfig {
            default_auto_approve: true,
            prompt_timeout_secs: 60,
            agents: HashMap::from([(
                "test".to_string(),
                AcpAgentConfig {
                    launch: "binary".to_string(),
                    command: "/usr/bin/test".to_string(),
                    args: vec![],
                    env: HashMap::new(),
                    workspace: None,
                    auto_approve: None,
                },
            )]),
        };

        let manager = AcpManager::from_config(config);
        assert!(manager.has_agent("test"));
        assert!(!manager.has_agent("other"));
        assert_eq!(manager.available_agents(), vec!["test"]);
        assert!(manager.config.default_auto_approve);
        assert_eq!(manager.config.prompt_timeout_secs, 60);
    }

    #[test]
    fn test_agent_config_method() {
        let config = AcpConfig {
            default_auto_approve: false,
            prompt_timeout_secs: 300,
            agents: HashMap::from([(
                "claude".to_string(),
                AcpAgentConfig {
                    launch: "npx".to_string(),
                    command: "@anthropic-ai/claude-code@latest".to_string(),
                    args: vec!["--acp".to_string()],
                    env: HashMap::new(),
                    workspace: Some("/tmp/ws".to_string()),
                    auto_approve: Some(true),
                },
            )]),
        };

        let manager = AcpManager::from_config(config);
        let agent_cfg = manager.agent_config("claude");
        assert!(agent_cfg.is_some());
        let cfg = agent_cfg.unwrap();
        assert_eq!(cfg.command, "@anthropic-ai/claude-code@latest");
        assert_eq!(cfg.workspace.as_deref(), Some("/tmp/ws"));
        assert_eq!(cfg.auto_approve, Some(true));

        assert!(manager.agent_config("nonexistent").is_none());
    }

    #[test]
    fn test_session_status_equality() {
        assert_eq!(SessionStatus::Active, SessionStatus::Active);
        assert_eq!(SessionStatus::Prompting, SessionStatus::Prompting);
        assert_eq!(SessionStatus::Ended, SessionStatus::Ended);
        assert_ne!(SessionStatus::Active, SessionStatus::Ended);
        assert_ne!(SessionStatus::Active, SessionStatus::Prompting);
    }

    #[test]
    fn test_jsonrpc_request_serialization() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(42),
            method: "test/method".to_string(),
            params: Some(serde_json::json!({"key": "value"})),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":42"));
        assert!(json.contains("\"method\":\"test/method\""));
        assert!(json.contains("\"key\":\"value\""));
    }

    #[test]
    fn test_jsonrpc_notification_serialization() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: "notifications/initialized".to_string(),
            params: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("\"id\""), "Notification should not have id");
    }

    #[test]
    fn test_jsonrpc_error_response_parsing() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": 5,
            "error": {
                "code": -32601,
                "message": "Method not found"
            }
        }"#;
        let msg: JsonRpcMessage = serde_json::from_str(json).unwrap();
        assert!(msg.is_response());
        assert!(msg.error.is_some());
        let err = msg.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }
}
