use std::sync::Arc;

use async_trait::async_trait;

use crate::llm_types::ToolDefinition;
use crate::mcp::{McpServer, McpToolInfo};

use super::{Tool, ToolResult};

pub struct McpTool {
    server: Arc<McpServer>,
    tool_info: McpToolInfo,
    qualified_name: String,
}

impl McpTool {
    pub fn new(server: Arc<McpServer>, tool_info: McpToolInfo) -> Self {
        // Namespaced name: mcp_{server}_{tool} to avoid conflicts with built-in tools
        let qualified_name = format!("mcp_{}_{}", tool_info.server_name, tool_info.name);
        // Sanitize: tool names must match [a-zA-Z0-9_-]{1,64}
        let qualified_name: String = qualified_name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .take(64)
            .collect();

        McpTool {
            server,
            tool_info,
            qualified_name,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.qualified_name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.qualified_name.clone(),
            description: format!(
                "[MCP:{}] {}",
                self.tool_info.server_name, self.tool_info.description
            ),
            input_schema: self.tool_info.input_schema.clone(),
        }
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        match self.server.call_tool(&self.tool_info.name, input).await {
            Ok(output) => ToolResult::success(output),
            Err(e) => {
                ToolResult::error(format!("MCP tool error: {e}")).with_error_type("mcp_error")
            }
        }
    }
}
