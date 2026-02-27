use std::collections::HashMap;
use std::sync::Arc;

use anyhow::anyhow;
use tokio::sync::Mutex;
use tracing::info;
#[cfg(feature = "sqlite-vec")]
use tracing::warn;

/// Wait for any termination signal: SIGTERM, SIGHUP, or Ctrl-C.
/// Returns a human-readable label of which signal was received.
async fn shutdown_signal() -> &'static str {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm =
        signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
    let mut sighup =
        signal(SignalKind::hangup()).expect("failed to register SIGHUP handler");

    tokio::select! {
        _ = tokio::signal::ctrl_c() => "SIGINT (Ctrl-C)",
        _ = sigterm.recv() => "SIGTERM",
        _ = sighup.recv() => "SIGHUP",
    }
}

use crate::channel_adapter::ChannelRegistry;
use crate::channels::telegram::TelegramChannelConfig;
use crate::channels::{DiscordAdapter, FeishuAdapter, SlackAdapter, TelegramAdapter};
use crate::config::Config;
use crate::db::Database;
use crate::embedding::EmbeddingProvider;
use crate::llm::LlmProvider;
use crate::memory::MemoryManager;
use crate::skills::SkillManager;
use crate::tools::ToolRegistry;
use crate::web::WebAdapter;

/// Per-chat mutex map to prevent concurrent agent loops for the same chat_id.
/// When a second request arrives for a chat_id that is already processing,
/// it waits for the first to finish before starting.
pub type ChatLocks = Mutex<HashMap<i64, Arc<Mutex<()>>>>;

pub struct AppState {
    pub config: Config,
    pub channel_registry: Arc<ChannelRegistry>,
    pub db: Arc<Database>,
    pub memory: MemoryManager,
    pub skills: SkillManager,
    pub llm: Box<dyn LlmProvider>,
    pub embedding: Option<Arc<dyn EmbeddingProvider>>,
    pub tools: ToolRegistry,
    pub acp_manager: Arc<crate::acp::AcpManager>,
    /// Per-chat concurrency lock: ensures only one agent loop runs per chat_id at a time.
    pub chat_locks: ChatLocks,
}

pub async fn run(
    config: Config,
    db: Database,
    memory: MemoryManager,
    skills: SkillManager,
    mcp_manager: crate::mcp::McpManager,
    acp_manager: crate::acp::AcpManager,
) -> anyhow::Result<()> {
    let db = Arc::new(db);
    let llm = crate::llm::create_provider(&config);
    let embedding = crate::embedding::create_provider(&config);
    #[cfg(feature = "sqlite-vec")]
    {
        let dim = embedding
            .as_ref()
            .map(|e| e.dimension())
            .or(config.embedding_dim)
            .unwrap_or(1536);
        if let Err(e) = db.prepare_vector_index(dim) {
            warn!("Failed to initialize sqlite-vec index: {e}");
        }
    }

    // Build channel registry from config
    let mut registry = ChannelRegistry::new();
    let mut telegram_bot: Option<teloxide::Bot> = None;
    let mut discord_token: Option<String> = None;
    let mut has_slack = false;

    if let Some(tg_cfg) = config.channel_config::<TelegramChannelConfig>("telegram") {
        if !tg_cfg.bot_token.trim().is_empty() {
            let bot = teloxide::Bot::new(&tg_cfg.bot_token);
            telegram_bot = Some(bot.clone());
            registry.register(Arc::new(TelegramAdapter::new(bot, tg_cfg)));
        }
    }

    if let Some(dc_cfg) =
        config.channel_config::<crate::channels::discord::DiscordChannelConfig>("discord")
    {
        if !dc_cfg.bot_token.trim().is_empty() {
            discord_token = Some(dc_cfg.bot_token.clone());
            registry.register(Arc::new(DiscordAdapter::new(dc_cfg.bot_token)));
        }
    }

    if let Some(slack_cfg) =
        config.channel_config::<crate::channels::slack::SlackChannelConfig>("slack")
    {
        if !slack_cfg.bot_token.trim().is_empty() && !slack_cfg.app_token.trim().is_empty() {
            has_slack = true;
            registry.register(Arc::new(SlackAdapter::new(slack_cfg.bot_token)));
        }
    }

    let mut has_feishu = false;
    if let Some(feishu_cfg) =
        config.channel_config::<crate::channels::feishu::FeishuChannelConfig>("feishu")
    {
        if !feishu_cfg.app_id.trim().is_empty() && !feishu_cfg.app_secret.trim().is_empty() {
            has_feishu = true;
            registry.register(Arc::new(FeishuAdapter::new(
                feishu_cfg.app_id.clone(),
                feishu_cfg.app_secret.clone(),
                feishu_cfg.domain.clone(),
            )));
        }
    }

    if config.web_enabled {
        registry.register(Arc::new(WebAdapter));
    }

    let channel_registry = Arc::new(registry);

    let mut tools = ToolRegistry::new(&config, channel_registry.clone(), db.clone());

    for (server, tool_info) in mcp_manager.all_tools() {
        tools.add_tool(Box::new(crate::tools::mcp::McpTool::new(server, tool_info)));
    }

    let acp_manager = Arc::new(acp_manager);

    // Register ACP tools so the model can directly create/prompt/end coding agent sessions.
    for tool in crate::tools::acp::make_acp_tools(acp_manager.clone()) {
        tools.add_tool(tool);
    }

    let state = Arc::new(AppState {
        config,
        channel_registry,
        db,
        memory,
        skills,
        llm,
        embedding,
        tools,
        acp_manager,
        chat_locks: Mutex::new(HashMap::new()),
    });

    crate::scheduler::spawn_scheduler(state.clone());
    crate::scheduler::spawn_reflector(state.clone());

    if let Some(ref token) = discord_token {
        let discord_state = state.clone();
        let token = token.clone();
        info!("Starting Discord bot");
        tokio::spawn(async move {
            crate::discord::start_discord_bot(discord_state, &token).await;
        });
    }

    if has_slack {
        let slack_state = state.clone();
        info!("Starting Slack bot (Socket Mode)");
        tokio::spawn(async move {
            crate::channels::slack::start_slack_bot(slack_state).await;
        });
    }

    if has_feishu {
        let feishu_state = state.clone();
        info!("Starting Feishu bot");
        tokio::spawn(async move {
            crate::channels::feishu::start_feishu_bot(feishu_state).await;
        });
    }

    if state.config.web_enabled {
        let web_state = state.clone();
        info!(
            "Starting Web UI server on {}:{}",
            state.config.web_host, state.config.web_port
        );
        tokio::spawn(async move {
            crate::web::start_web_server(web_state).await;
        });
    }

    if let Some(bot) = telegram_bot {
        let result = crate::telegram::start_telegram_bot(state.clone(), bot).await;

        // Clean up ACP sessions after Telegram dispatcher exits
        info!("Cleaning up ACP sessions...");
        state.acp_manager.cleanup().await;

        result
    } else if state.config.web_enabled || discord_token.is_some() || has_slack || has_feishu {
        info!("Running without Telegram adapter; waiting for other channels");
        let sig = shutdown_signal().await;
        info!("Received {sig}, starting graceful shutdown...");

        // Graceful shutdown: give in-flight tasks a moment to complete
        info!("Allowing in-flight tasks 2s to finish...");
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // Clean up ACP sessions (terminate agent subprocesses)
        info!("Cleaning up ACP sessions...");
        state.acp_manager.cleanup().await;

        // SQLite WAL mode ensures DB consistency even on hard kill,
        // but explicit flush is good practice
        info!("Shutdown complete.");
        Ok(())
    } else {
        Err(anyhow!(
            "No channel is enabled. Configure Telegram, Discord, Slack, Feishu, or web_enabled=true."
        ))
    }
}
