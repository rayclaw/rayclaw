use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

use crate::db::{call_blocking, Database};
use crate::llm_types::ToolDefinition;
use crate::memory_quality;

use super::{auth_context_from_input, authorize_chat_access, schema_object, Tool, ToolResult};

pub struct ReadMemoryTool {
    groups_dir: PathBuf,
}

impl ReadMemoryTool {
    pub fn new(data_dir: &str) -> Self {
        ReadMemoryTool {
            groups_dir: PathBuf::from(data_dir).join("groups"),
        }
    }
}

#[async_trait]
impl Tool for ReadMemoryTool {
    fn name(&self) -> &str {
        "read_memory"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read_memory".into(),
            description: "Read the AGENTS.md memory file. Use scope 'global' for memories shared across all chats, or 'chat' for chat-specific memories.".into(),
            input_schema: schema_object(
                json!({
                    "scope": {
                        "type": "string",
                        "description": "Memory scope: 'global' or 'chat'",
                        "enum": ["global", "chat"]
                    },
                    "chat_id": {
                        "type": "integer",
                        "description": "Chat ID (required for scope 'chat')"
                    }
                }),
                &["scope"],
            ),
        }
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let scope = match input.get("scope").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::error("Missing 'scope' parameter".into()),
        };

        let path = match scope {
            "global" => self.groups_dir.join("AGENTS.md"),
            "chat" => {
                let chat_id = match input.get("chat_id").and_then(|v| v.as_i64()) {
                    Some(id) => id,
                    None => return ToolResult::error("Missing 'chat_id' for chat scope".into()),
                };
                if let Err(e) = authorize_chat_access(&input, chat_id) {
                    return ToolResult::error(e);
                }
                self.groups_dir.join(chat_id.to_string()).join("AGENTS.md")
            }
            _ => return ToolResult::error("scope must be 'global' or 'chat'".into()),
        };

        info!("Reading memory: {}", path.display());

        match std::fs::read_to_string(&path) {
            Ok(content) => {
                if content.trim().is_empty() {
                    ToolResult::success("Memory file is empty.".into())
                } else {
                    ToolResult::success(content)
                }
            }
            Err(_) => ToolResult::success("No memory file found (not yet created).".into()),
        }
    }
}

pub struct WriteMemoryTool {
    groups_dir: PathBuf,
    db: Arc<Database>,
}

impl WriteMemoryTool {
    pub fn new(data_dir: &str, db: Arc<Database>) -> Self {
        WriteMemoryTool {
            groups_dir: PathBuf::from(data_dir).join("groups"),
            db,
        }
    }
}

#[async_trait]
impl Tool for WriteMemoryTool {
    fn name(&self) -> &str {
        "write_memory"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "write_memory".into(),
            description: "Write to the AGENTS.md memory file. Use this to remember important information about the user or conversation. Use scope 'global' for memories shared across all chats, or 'chat' for chat-specific memories.".into(),
            input_schema: schema_object(
                json!({
                    "scope": {
                        "type": "string",
                        "description": "Memory scope: 'global' or 'chat'",
                        "enum": ["global", "chat"]
                    },
                    "chat_id": {
                        "type": "integer",
                        "description": "Chat ID (required for scope 'chat')"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write to the memory file (replaces existing content)"
                    }
                }),
                &["scope", "content"],
            ),
        }
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let scope = match input.get("scope").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::error("Missing 'scope' parameter".into()),
        };
        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return ToolResult::error("Missing 'content' parameter".into()),
        };

        let (path, memory_chat_id) = match scope {
            "global" => {
                if let Some(auth) = auth_context_from_input(&input) {
                    if !auth.is_control_chat() {
                        return ToolResult::error(format!(
                            "Permission denied: chat {} cannot write global memory",
                            auth.caller_chat_id
                        ));
                    }
                }
                (self.groups_dir.join("AGENTS.md"), None)
            }
            "chat" => {
                let chat_id = match input.get("chat_id").and_then(|v| v.as_i64()) {
                    Some(id) => id,
                    None => return ToolResult::error("Missing 'chat_id' for chat scope".into()),
                };
                if let Err(e) = authorize_chat_access(&input, chat_id) {
                    return ToolResult::error(e);
                }
                (
                    self.groups_dir.join(chat_id.to_string()).join("AGENTS.md"),
                    Some(chat_id),
                )
            }
            _ => return ToolResult::error("scope must be 'global' or 'chat'".into()),
        };

        info!("Writing memory: {}", path.display());

        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolResult::error(format!("Failed to create directory: {e}"));
            }
        }

        match std::fs::write(&path, content) {
            Ok(()) => {
                let memory_content = content.trim().to_string();
                if !memory_content.is_empty() {
                    if let Some(normalized) =
                        memory_quality::normalize_memory_content(&memory_content, 180)
                    {
                        if memory_quality::memory_quality_ok(&normalized) {
                            let chat_id = memory_chat_id;
                            let _ = call_blocking(self.db.clone(), move |db| {
                                db.insert_memory_with_metadata(
                                    chat_id,
                                    &normalized,
                                    "KNOWLEDGE",
                                    "write_memory_tool",
                                    0.85,
                                )
                                .map(|_| ())
                            })
                            .await;
                        }
                    }
                }

                ToolResult::success(format!("Memory saved to {} scope.", scope))
            }
            Err(e) => ToolResult::error(format!("Failed to write memory: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    use crate::db::Database;

    fn test_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("rayclaw_memtool_{}", uuid::Uuid::new_v4()))
    }

    fn test_db(dir: &std::path::Path) -> Arc<Database> {
        let runtime = dir.join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        Arc::new(Database::new(runtime.to_str().unwrap()).unwrap())
    }

    #[tokio::test]
    async fn test_read_memory_global_not_exists() {
        let dir = test_dir();
        let tool = ReadMemoryTool::new(dir.to_str().unwrap());
        let result = tool.execute(json!({"scope": "global"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("No memory file found"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_write_and_read_memory_global() {
        let dir = test_dir();
        let db = test_db(&dir);
        let write_tool = WriteMemoryTool::new(dir.to_str().unwrap(), db.clone());
        let read_tool = ReadMemoryTool::new(dir.to_str().unwrap());

        let result = write_tool
            .execute(json!({"scope": "global", "content": "user prefers Rust"}))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("Memory saved"));

        let result = read_tool.execute(json!({"scope": "global"})).await;
        assert!(!result.is_error);
        assert_eq!(result.content, "user prefers Rust");
        let mems = db.get_all_memories_for_chat(None).unwrap();
        assert_eq!(mems.len(), 1);
        assert_eq!(mems[0].content, "user prefers Rust");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_write_and_read_memory_chat() {
        let dir = test_dir();
        let db = test_db(&dir);
        let write_tool = WriteMemoryTool::new(dir.to_str().unwrap(), db.clone());
        let read_tool = ReadMemoryTool::new(dir.to_str().unwrap());

        let result = write_tool
            .execute(json!({"scope": "chat", "chat_id": 42, "content": "chat 42 notes"}))
            .await;
        assert!(!result.is_error);

        let result = read_tool
            .execute(json!({"scope": "chat", "chat_id": 42}))
            .await;
        assert!(!result.is_error);
        assert_eq!(result.content, "chat 42 notes");
        let mems = db.get_all_memories_for_chat(Some(42)).unwrap();
        assert_eq!(mems.len(), 1);
        assert_eq!(mems[0].content, "chat 42 notes");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_read_memory_chat_missing_chat_id() {
        let dir = test_dir();
        let tool = ReadMemoryTool::new(dir.to_str().unwrap());
        let result = tool.execute(json!({"scope": "chat"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing 'chat_id'"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_write_memory_missing_scope() {
        let dir = test_dir();
        let db = test_db(&dir);
        let tool = WriteMemoryTool::new(dir.to_str().unwrap(), db);
        let result = tool.execute(json!({"content": "data"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing 'scope'"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_read_memory_invalid_scope() {
        let dir = test_dir();
        let tool = ReadMemoryTool::new(dir.to_str().unwrap());
        let result = tool.execute(json!({"scope": "invalid"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("must be 'global' or 'chat'"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_read_memory_empty_file() {
        let dir = test_dir();
        let db = test_db(&dir);
        let write_tool = WriteMemoryTool::new(dir.to_str().unwrap(), db);
        let read_tool = ReadMemoryTool::new(dir.to_str().unwrap());

        write_tool
            .execute(json!({"scope": "global", "content": "   "}))
            .await;

        let result = read_tool.execute(json!({"scope": "global"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("empty"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_write_memory_global_denied_for_non_control_chat() {
        let dir = test_dir();
        let db = test_db(&dir);
        let tool = WriteMemoryTool::new(dir.to_str().unwrap(), db);
        let result = tool
            .execute(json!({
                "scope": "global",
                "content": "secret",
                "__rayclaw_auth": {
                    "caller_chat_id": 100,
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_write_memory_global_allowed_for_control_chat() {
        let dir = test_dir();
        let db = test_db(&dir);
        let tool = WriteMemoryTool::new(dir.to_str().unwrap(), db);
        let result = tool
            .execute(json!({
                "scope": "global",
                "content": "global ok",
                "__rayclaw_auth": {
                    "caller_chat_id": 100,
                    "control_chat_ids": [100]
                }
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);
        let content = std::fs::read_to_string(dir.join("groups").join("AGENTS.md")).unwrap();
        assert_eq!(content, "global ok");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_read_memory_chat_permission_denied() {
        let dir = test_dir();
        let tool = ReadMemoryTool::new(dir.to_str().unwrap());
        let result = tool
            .execute(json!({
                "scope": "chat",
                "chat_id": 200,
                "__rayclaw_auth": {
                    "caller_chat_id": 100,
                    "control_chat_ids": []
                }
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_read_memory_chat_allowed_for_control_chat_cross_chat() {
        let dir = test_dir();
        let db = test_db(&dir);
        let write_tool = WriteMemoryTool::new(dir.to_str().unwrap(), db);
        let read_tool = ReadMemoryTool::new(dir.to_str().unwrap());
        write_tool
            .execute(json!({"scope": "chat", "chat_id": 200, "content": "chat200"}))
            .await;
        let result = read_tool
            .execute(json!({
                "scope": "chat",
                "chat_id": 200,
                "__rayclaw_auth": {
                    "caller_chat_id": 100,
                    "control_chat_ids": [100]
                }
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(result.content, "chat200");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
