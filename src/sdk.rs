//! SDK entry point for embedding RayClaw as a library.
//!
//! `RayClawAgent` is a facade that initializes the agent engine, LLM provider,
//! tool registry, and persistence layer without starting any channel adapters,
//! schedulers, or signal handlers. Third-party applications (Tauri desktop,
//! custom web services, CLI tools, etc.) use this to run agent conversations
//! programmatically.
//!
//! # Example
//!
//! ```rust,no_run
//! use rayclaw::config::Config;
//! use rayclaw::sdk::RayClawAgent;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = Config::load()?;
//!     let agent = RayClawAgent::new(config).await?;
//!     let reply = agent.process_message(1, "Hello!").await?;
//!     println!("{reply}");
//!     Ok(())
//! }
//! ```

use std::path::Path;
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;
use tracing::info;

use crate::agent_engine::{self, AgentEvent, AgentRequestContext};
use crate::channel_adapter::ChannelRegistry;
use crate::config::Config;
use crate::db::Database;
use crate::error::RayClawError;
use crate::memory::MemoryManager;
use crate::runtime::{self, AppState};
use crate::skills::SkillManager;

/// A self-contained agent handle for library / SDK usage.
///
/// Wraps an `AppState` initialized without channel adapters or background tasks.
/// All interactions happen through explicit method calls.
pub struct RayClawAgent {
    state: Arc<AppState>,
}

impl RayClawAgent {
    /// Build a new agent from the given config.
    ///
    /// This initializes the database, memory manager, skill manager, MCP/ACP
    /// integrations, and tool registry. No channels, schedulers, or signal
    /// handlers are started.
    pub async fn new(mut config: Config) -> Result<Self, RayClawError> {
        config.validate_for_sdk()?;

        let data_root_dir = config.data_root_dir();
        let runtime_data_dir = config.runtime_data_dir();
        let skills_data_dir = config.skills_data_dir();

        crate::builtin_skills::ensure_builtin_skills(&data_root_dir)?;
        crate::builtin_skills::ensure_default_soul(&data_root_dir)?;

        let db = Arc::new(Database::new(&runtime_data_dir)?);
        info!("Database initialized");

        let memory = MemoryManager::new(&runtime_data_dir);
        info!("Memory manager initialized");

        let skill_manager = SkillManager::from_skills_dir(&skills_data_dir);
        let discovered = skill_manager.discover_skills();
        info!(
            "Skill manager initialized ({} skills discovered)",
            discovered.len()
        );

        let mcp_config_path = data_root_dir.join("mcp.json").to_string_lossy().to_string();
        let mcp_manager = crate::mcp::McpManager::from_config_file(&mcp_config_path).await;
        let mcp_tool_count = mcp_manager.all_tools().len();
        if mcp_tool_count > 0 {
            info!("MCP initialized: {} tools available", mcp_tool_count);
        }

        let acp_config_path = data_root_dir.join("acp.json").to_string_lossy().to_string();
        let acp_manager = crate::acp::AcpManager::from_config_file(&acp_config_path);
        if !acp_manager.available_agents().is_empty() {
            info!(
                "ACP initialized: {} agent(s) configured",
                acp_manager.available_agents().len()
            );
        }

        // Migrate legacy data layout if needed
        let runtime_dir = Path::new(&runtime_data_dir);
        if std::fs::create_dir_all(runtime_dir).is_ok() {
            // runtime dir ready
        }

        let mut runtime_config = config.clone();
        runtime_config.data_dir = runtime_data_dir;

        let channel_registry = Arc::new(ChannelRegistry::new());
        let state = runtime::create_app_state(
            runtime_config,
            db,
            channel_registry,
            memory,
            skill_manager,
            mcp_manager,
            acp_manager,
            true, // use SDK tools (no send_message, no schedule)
        )
        .await
        .map_err(|e| RayClawError::Config(format!("Failed to initialize agent: {e}")))?;

        Ok(RayClawAgent { state })
    }

    /// Process a single message synchronously (waits for the full response).
    pub async fn process_message(
        &self,
        chat_id: i64,
        user_text: &str,
    ) -> Result<String, RayClawError> {
        let context = AgentRequestContext {
            caller_channel: "sdk",
            chat_id,
            chat_type: "private",
        };
        self.store_user_message(chat_id, user_text);
        agent_engine::process_with_agent(&self.state, context, Some(user_text), None)
            .await
            .map_err(|e| RayClawError::Agent(e.to_string()))
    }

    /// Process a message with streaming events pushed to `event_tx`.
    pub async fn process_message_stream(
        &self,
        chat_id: i64,
        user_text: &str,
        event_tx: UnboundedSender<AgentEvent>,
    ) -> Result<String, RayClawError> {
        let context = AgentRequestContext {
            caller_channel: "sdk",
            chat_id,
            chat_type: "private",
        };
        self.store_user_message(chat_id, user_text);
        agent_engine::process_with_agent_with_events(
            &self.state,
            context,
            Some(user_text),
            None,
            Some(&event_tx),
        )
        .await
        .map_err(|e| RayClawError::Agent(e.to_string()))
    }

    /// Clear the conversation session for the given chat_id.
    pub fn reset_session(&self, chat_id: i64) -> Result<(), RayClawError> {
        self.state.db.delete_session(chat_id)?;
        Ok(())
    }

    /// Retrieve stored messages for a chat.
    pub fn get_messages(
        &self,
        chat_id: i64,
        limit: usize,
    ) -> Result<Vec<crate::db::StoredMessage>, RayClawError> {
        self.state.db.get_recent_messages(chat_id, limit)
    }

    /// Get a reference to the underlying `AppState` for advanced usage.
    pub fn state(&self) -> &Arc<AppState> {
        &self.state
    }

    fn store_user_message(&self, chat_id: i64, text: &str) {
        let _ = self.state.db.upsert_chat(chat_id, None, "private");
        let msg = crate::db::StoredMessage {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id,
            sender_name: "user".to_string(),
            content: text.to_string(),
            is_from_bot: false,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        let _ = self.state.db.store_message(&msg);
    }
}
