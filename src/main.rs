use rayclaw::config::Config;
use rayclaw::error::RayClawError;
use rayclaw::{
    acp, builtin_skills, db, doctor, gateway, logging, mcp, memory, runtime, setup, skills,
};
use std::path::Path;
use tracing::info;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!(
        r#"RayClaw v{VERSION} — multi-channel agentic runtime

Usage:
  rayclaw <command>

Commands:
  start      Launch the agent runtime (all enabled channels)
  setup      Interactive setup wizard (creates rayclaw.config.yaml)
  doctor     Run preflight environment checks
  gateway    Service lifecycle (install / start / stop / status / logs)
  version    Print version and exit
  help       Show this message

Getting started:
  rayclaw setup      Configure provider, channels, and options
  rayclaw doctor     Verify environment is ready
  rayclaw start      Start serving on configured channels

At least one channel must be enabled (Telegram, Discord, Slack, Feishu, or Web UI).

Docs & source:
  https://rayclaw.ai"#
    );
}

fn print_version() {
    println!("rayclaw {VERSION}");
}

fn move_path(src: &Path, dst: &Path) -> std::io::Result<()> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }

    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let child_src = entry.path();
            let child_dst = dst.join(entry.file_name());
            move_path(&child_src, &child_dst)?;
        }
        std::fs::remove_dir_all(src)?;
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
        std::fs::remove_file(src)?;
    }

    Ok(())
}

fn migrate_legacy_runtime_layout(data_root: &Path, runtime_dir: &Path) {
    if std::fs::create_dir_all(runtime_dir).is_err() {
        return;
    }

    let entries = match std::fs::read_dir(data_root) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if name_str == "skills" || name_str == "runtime" || name_str == "mcp.json" {
            continue;
        }
        let src = entry.path();
        let dst = runtime_dir.join(name_str);
        if dst.exists() {
            continue;
        }
        if let Err(e) = move_path(&src, &dst) {
            tracing::warn!(
                "Failed to migrate legacy data '{}' -> '{}': {}",
                src.display(),
                dst.display(),
                e
            );
        } else {
            tracing::info!(
                "Migrated legacy runtime data '{}' -> '{}'",
                src.display(),
                dst.display()
            );
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str());

    match command {
        Some("start") => {}
        Some("gateway") => {
            gateway::handle_gateway_cli(&args[2..])?;
            return Ok(());
        }
        Some("setup") => {
            let saved = setup::run_setup_wizard()?;
            if saved {
                println!("Setup saved to rayclaw.config.yaml");
            } else {
                println!("Setup canceled");
            }
            return Ok(());
        }
        Some("doctor") => {
            doctor::run_cli(&args[2..])?;
            return Ok(());
        }
        Some("version" | "--version" | "-V") => {
            print_version();
            return Ok(());
        }
        Some("help" | "--help" | "-h") | None => {
            print_help();
            return Ok(());
        }
        Some(unknown) => {
            eprintln!("Unknown command: {unknown}\n");
            print_help();
            std::process::exit(1);
        }
    }

    let config = match Config::load() {
        Ok(c) => c,
        Err(RayClawError::Config(e)) => {
            eprintln!("Config missing/invalid: {e}");
            eprintln!("Launching setup wizard...");
            let saved = setup::run_setup_wizard()?;
            if !saved {
                return Err(anyhow::anyhow!(
                    "setup canceled and config is still incomplete"
                ));
            }
            Config::load()?
        }
        Err(e) => return Err(e.into()),
    };
    info!("RayClaw runtime starting...");

    let data_root_dir = config.data_root_dir();
    let runtime_data_dir = config.runtime_data_dir();
    let skills_data_dir = config.skills_data_dir();
    migrate_legacy_runtime_layout(&data_root_dir, Path::new(&runtime_data_dir));
    builtin_skills::ensure_builtin_skills(&data_root_dir)?;
    builtin_skills::ensure_default_soul(&data_root_dir)?;

    if std::env::var("RAYCLAW_GATEWAY").is_ok() {
        logging::init_logging(&runtime_data_dir)?;
    } else {
        logging::init_console_logging();
    }

    let db = db::Database::new(&runtime_data_dir)?;
    info!("Database initialized");

    let memory_manager = memory::MemoryManager::new(&runtime_data_dir);
    info!("Memory manager initialized");

    let skill_manager = skills::SkillManager::from_skills_dir(&skills_data_dir);
    let discovered = skill_manager.discover_skills();
    info!(
        "Skill manager initialized ({} skills discovered)",
        discovered.len()
    );

    // Initialize MCP servers (optional, configured via <data_root>/mcp.json)
    let mcp_config_path = data_root_dir.join("mcp.json").to_string_lossy().to_string();
    let mcp_manager = mcp::McpManager::from_config_file(&mcp_config_path).await;
    let mcp_tool_count: usize = mcp_manager.all_tools().len();
    if mcp_tool_count > 0 {
        info!("MCP initialized: {} tools available", mcp_tool_count);
    }

    // Initialize ACP agent manager (optional, configured via <data_root>/acp.json)
    let acp_config_path = data_root_dir.join("acp.json").to_string_lossy().to_string();
    let acp_manager = acp::AcpManager::from_config_file(&acp_config_path);
    if !acp_manager.available_agents().is_empty() {
        info!(
            "ACP initialized: {} agent(s) configured",
            acp_manager.available_agents().len()
        );
    }

    let mut runtime_config = config.clone();
    runtime_config.data_dir = runtime_data_dir;

    runtime::run(
        runtime_config,
        db,
        memory_manager,
        skill_manager,
        mcp_manager,
        acp_manager,
    )
    .await?;

    Ok(())
}
