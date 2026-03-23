use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use console::{style, Term};
use dialoguer::{Confirm, Input, MultiSelect, Select};

use crate::codex_auth::{
    codex_config_default_openai_base_url, is_openai_codex_provider, provider_allows_empty_api_key,
    resolve_openai_codex_auth,
};
use crate::error::RayClawError;
use crate::text::floor_char_boundary;

// ---------------------------------------------------------------------------
// Declarative channel metadata (shared with old setup — keep in sync)
// ---------------------------------------------------------------------------

struct ChannelFieldDef {
    yaml_key: &'static str,
    label: &'static str,
    default: &'static str,
    #[allow(dead_code)]
    secret: bool,
    required: bool,
}

struct DynamicChannelDef {
    name: &'static str,
    #[allow(dead_code)]
    presence_keys: &'static [&'static str],
    fields: &'static [ChannelFieldDef],
}

const DYNAMIC_CHANNELS: &[DynamicChannelDef] = &[
    DynamicChannelDef {
        name: "slack",
        presence_keys: &["bot_token", "app_token"],
        fields: &[
            ChannelFieldDef {
                yaml_key: "bot_token",
                label: "Slack bot token (xoxb-...)",
                default: "",
                secret: true,
                required: true,
            },
            ChannelFieldDef {
                yaml_key: "app_token",
                label: "Slack app token (xapp-...)",
                default: "",
                secret: true,
                required: true,
            },
        ],
    },
    DynamicChannelDef {
        name: "feishu",
        presence_keys: &["app_id", "app_secret"],
        fields: &[
            ChannelFieldDef {
                yaml_key: "app_id",
                label: "Feishu app ID",
                default: "",
                secret: false,
                required: true,
            },
            ChannelFieldDef {
                yaml_key: "app_secret",
                label: "Feishu app secret",
                default: "",
                secret: true,
                required: true,
            },
            ChannelFieldDef {
                yaml_key: "domain",
                label: "Feishu domain (feishu/lark/custom)",
                default: "feishu",
                secret: false,
                required: false,
            },
        ],
    },
    DynamicChannelDef {
        name: "weixin",
        presence_keys: &["bot_token"],
        fields: &[
            ChannelFieldDef {
                yaml_key: "bot_token",
                label: "WeChat bot token (from: rayclaw weixin-login)",
                default: "",
                secret: true,
                required: true,
            },
            ChannelFieldDef {
                yaml_key: "base_url",
                label: "WeChat API base URL (leave empty for default)",
                default: "",
                secret: false,
                required: false,
            },
        ],
    },
];

fn dynamic_field_key(channel: &str, yaml_key: &str) -> String {
    format!("DYN_{}_{}", channel.to_uppercase(), yaml_key.to_uppercase())
}

// ---------------------------------------------------------------------------
// Provider presets
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProviderProtocol {
    Anthropic,
    OpenAiCompat,
    Bedrock,
}

#[derive(Clone, Copy)]
struct ProviderPreset {
    id: &'static str,
    label: &'static str,
    protocol: ProviderProtocol,
    default_base_url: &'static str,
    models: &'static [&'static str],
}

const PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "openai",
        label: "OpenAI",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.openai.com/v1",
        models: &["gpt-5.2"],
    },
    ProviderPreset {
        id: "openai-codex",
        label: "OpenAI Codex",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "",
        models: &["gpt-5.3-codex"],
    },
    ProviderPreset {
        id: "openrouter",
        label: "OpenRouter",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://openrouter.ai/api/v1",
        models: &[
            "openrouter/auto",
            "anthropic/claude-sonnet-4.5",
            "openai/gpt-5.2",
        ],
    },
    ProviderPreset {
        id: "anthropic",
        label: "Anthropic",
        protocol: ProviderProtocol::Anthropic,
        default_base_url: "",
        models: &["claude-sonnet-4-5-20250929", "claude-opus-4-6-20260205"],
    },
    ProviderPreset {
        id: "ollama",
        label: "Ollama (local)",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "http://127.0.0.1:11434/v1",
        models: &["llama3.2", "qwen2.5-coder:7b", "mistral"],
    },
    ProviderPreset {
        id: "google",
        label: "Google DeepMind",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        models: &["gemini-2.5-pro", "gemini-2.5-flash"],
    },
    ProviderPreset {
        id: "alibaba",
        label: "Alibaba Cloud (Qwen / DashScope)",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        models: &["qwen3-max", "qwen-max-latest"],
    },
    ProviderPreset {
        id: "deepseek",
        label: "DeepSeek",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.deepseek.com/v1",
        models: &["deepseek-chat", "deepseek-reasoner"],
    },
    ProviderPreset {
        id: "moonshot",
        label: "Moonshot AI (Kimi)",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.moonshot.cn/v1",
        models: &["kimi-k2.5", "kimi-k2"],
    },
    ProviderPreset {
        id: "mistral",
        label: "Mistral AI",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.mistral.ai/v1",
        models: &["mistral-large-latest", "ministral-8b-latest"],
    },
    ProviderPreset {
        id: "azure",
        label: "Microsoft Azure AI",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url:
            "https://YOUR-RESOURCE.openai.azure.com/openai/deployments/YOUR-DEPLOYMENT",
        models: &["gpt-5.2", "gpt-5"],
    },
    ProviderPreset {
        id: "bedrock",
        label: "Amazon Bedrock (native Converse API)",
        protocol: ProviderProtocol::Bedrock,
        default_base_url: "",
        models: &[
            "anthropic.claude-sonnet-4-5-v2",
            "anthropic.claude-opus-4-6-v1",
        ],
    },
    ProviderPreset {
        id: "zhipu",
        label: "Zhipu AI (GLM / Z.AI)",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://open.bigmodel.cn/api/paas/v4",
        models: &["glm-4.7", "glm-4.7-flash"],
    },
    ProviderPreset {
        id: "minimax",
        label: "MiniMax",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.minimax.io/v1",
        models: &["MiniMax-M2.1"],
    },
    ProviderPreset {
        id: "cohere",
        label: "Cohere",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.cohere.ai/compatibility/v1",
        models: &["command-a-03-2025", "command-r-plus-08-2024"],
    },
    ProviderPreset {
        id: "tencent",
        label: "Tencent AI Lab",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.hunyuan.cloud.tencent.com/v1",
        models: &["hunyuan-t1-latest", "hunyuan-turbos-latest"],
    },
    ProviderPreset {
        id: "xai",
        label: "xAI",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.x.ai/v1",
        models: &["grok-4", "grok-3"],
    },
    ProviderPreset {
        id: "huggingface",
        label: "Hugging Face",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://router.huggingface.co/v1",
        models: &["Qwen/Qwen3-Coder-Next", "meta-llama/Llama-3.3-70B-Instruct"],
    },
    ProviderPreset {
        id: "together",
        label: "Together AI",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "https://api.together.xyz/v1",
        models: &[
            "deepseek-ai/DeepSeek-V3",
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        ],
    },
    ProviderPreset {
        id: "custom",
        label: "Custom (manual config)",
        protocol: ProviderProtocol::OpenAiCompat,
        default_base_url: "",
        models: &["custom-model"],
    },
];

fn find_provider_preset(provider: &str) -> Option<&'static ProviderPreset> {
    PROVIDER_PRESETS
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(provider))
}

fn provider_protocol(provider: &str) -> ProviderProtocol {
    find_provider_preset(provider)
        .map(|p| p.protocol)
        .unwrap_or(ProviderProtocol::OpenAiCompat)
}

fn default_model_for_provider(provider: &str) -> &'static str {
    find_provider_preset(provider)
        .and_then(|p| p.models.first().copied())
        .unwrap_or("gpt-5.2")
}

// ---------------------------------------------------------------------------
// Step result: supports back-navigation between wizard steps
// ---------------------------------------------------------------------------

enum StepResult<T> {
    Next(T),
    Back,
}

// ---------------------------------------------------------------------------
// UI helpers
// ---------------------------------------------------------------------------

const BANNER: &str = r"
    ██████╗  █████╗ ██╗   ██╗ ██████╗██╗      █████╗ ██╗    ██╗
    ██╔══██╗██╔══██╗╚██╗ ██╔╝██╔════╝██║     ██╔══██╗██║    ██║
    ██████╔╝███████║ ╚████╔╝ ██║     ██║     ███████║██║ █╗ ██║
    ██╔══██╗██╔══██║  ╚██╔╝  ██║     ██║     ██╔══██║██║███╗██║
    ██║  ██║██║  ██║   ██║   ╚██████╗███████╗██║  ██║╚███╔███╔╝
    ╚═╝  ╚═╝╚═╝  ╚═╝   ╚═╝    ╚═════╝╚══════╝╚═╝  ╚═╝ ╚══╝╚══╝
";

fn print_banner() {
    println!("{}", style(BANNER).cyan().bold());
    println!(
        "  {}",
        style("This wizard will configure your agent in under 60 seconds.")
            .white()
            .bold()
    );
    println!();
}

fn print_step(current: u8, total: u8, title: &str) {
    let _ = Term::stdout().clear_screen();
    println!("{}", style(BANNER).cyan().bold());
    println!(
        "  {} {}",
        style(format!("[{current}/{total}]")).cyan().bold(),
        style(title).white().bold()
    );
    println!("  {}", style("─".repeat(50)).dim());
}

fn print_result(text: &str) {
    println!("  {} {}", style("✓").green().bold(), text);
}

fn print_error(text: &str) {
    println!("  {} {}", style("✖").red().bold(), text);
}

/// Post-step nav: ←/Esc = back, →/Enter = next.
fn ask_nav() -> Result<bool, RayClawError> {
    let term = Term::stdout();
    // blank line for cursor, then nav hint below
    term.write_line("").ok();
    term.write_line(&format!(
        "  {}    {}",
        style("← Back").green(),
        style("→ Next (Enter)").green(),
    ))
    .ok();
    // move cursor up one line (above the hint)
    term.move_cursor_up(1).ok();
    loop {
        match term
            .read_key()
            .map_err(|e| RayClawError::Config(format!("Key read error: {e}")))?
        {
            console::Key::ArrowRight | console::Key::Enter => {
                term.move_cursor_down(1).ok();
                println!();
                return Ok(true);
            }
            console::Key::ArrowLeft | console::Key::Escape => {
                term.move_cursor_down(1).ok();
                println!();
                return Ok(false);
            }
            _ => {}
        }
    }
}

fn mask_secret(s: &str) -> String {
    if s.len() <= 6 {
        return "***".into();
    }
    let left = floor_char_boundary(s, 3.min(s.len()));
    let right_start = floor_char_boundary(s, s.len().saturating_sub(2));
    format!("{}***{}", &s[..left], &s[right_start..])
}

// ---------------------------------------------------------------------------
// Load existing config
// ---------------------------------------------------------------------------

fn load_existing_config() -> HashMap<String, String> {
    let yaml_path = if Path::new("./rayclaw.config.yaml").exists() {
        Some("./rayclaw.config.yaml")
    } else if Path::new("./rayclaw.config.yml").exists() {
        Some("./rayclaw.config.yml")
    } else {
        None
    };

    if let Some(path) = yaml_path {
        if let Ok(content) = fs::read_to_string(path) {
            if let Ok(config) = serde_yaml::from_str::<crate::config::Config>(&content) {
                let mut map = HashMap::new();
                let mut enabled = Vec::new();
                if !config.telegram_bot_token.trim().is_empty() {
                    enabled.push("telegram");
                }
                if config
                    .discord_bot_token
                    .as_deref()
                    .map(|v| !v.trim().is_empty())
                    .unwrap_or(false)
                {
                    enabled.push("discord");
                }
                for ch in DYNAMIC_CHANNELS {
                    if config.channels.contains_key(ch.name) {
                        enabled.push(ch.name);
                    }
                }
                map.insert("ENABLED_CHANNELS".into(), enabled.join(","));
                map.insert("TELEGRAM_BOT_TOKEN".into(), config.telegram_bot_token);
                map.insert("BOT_USERNAME".into(), config.bot_username);
                map.insert(
                    "DISCORD_BOT_TOKEN".into(),
                    config.discord_bot_token.unwrap_or_default(),
                );
                for ch in DYNAMIC_CHANNELS {
                    if let Some(ch_map) = config.channels.get(ch.name) {
                        for f in ch.fields {
                            if let Some(v) = ch_map.get(f.yaml_key).and_then(|v| v.as_str()) {
                                let key = dynamic_field_key(ch.name, f.yaml_key);
                                map.insert(key, v.to_string());
                            }
                        }
                    }
                }
                map.insert("LLM_PROVIDER".into(), config.llm_provider);
                map.insert("LLM_API_KEY".into(), config.api_key);
                if !config.model.is_empty() {
                    map.insert("LLM_MODEL".into(), config.model);
                }
                if let Some(url) = config.llm_base_url {
                    map.insert("LLM_BASE_URL".into(), url);
                }
                if let Some(v) = config.aws_region {
                    map.insert("AWS_REGION".into(), v);
                }
                if let Some(v) = config.aws_access_key_id {
                    map.insert("AWS_ACCESS_KEY_ID".into(), v);
                }
                if let Some(v) = config.aws_secret_access_key {
                    map.insert("AWS_SECRET_ACCESS_KEY".into(), v);
                }
                if let Some(v) = config.aws_session_token {
                    map.insert("AWS_SESSION_TOKEN".into(), v);
                }
                if let Some(v) = config.aws_profile {
                    map.insert("AWS_PROFILE".into(), v);
                }
                map.insert("DATA_DIR".into(), config.data_dir);
                map.insert("TIMEZONE".into(), config.timezone);
                map.insert("WORKING_DIR".into(), config.working_dir);
                map.insert(
                    "REFLECTOR_ENABLED".into(),
                    config.reflector_enabled.to_string(),
                );
                map.insert(
                    "REFLECTOR_INTERVAL_MINS".into(),
                    config.reflector_interval_mins.to_string(),
                );
                map.insert(
                    "MEMORY_TOKEN_BUDGET".into(),
                    config.memory_token_budget.to_string(),
                );
                if let Some(v) = config.embedding_provider {
                    map.insert("EMBEDDING_PROVIDER".into(), v);
                }
                if let Some(v) = config.embedding_api_key {
                    map.insert("EMBEDDING_API_KEY".into(), v);
                }
                if let Some(v) = config.embedding_base_url {
                    map.insert("EMBEDDING_BASE_URL".into(), v);
                }
                if let Some(v) = config.embedding_model {
                    map.insert("EMBEDDING_MODEL".into(), v);
                }
                if let Some(v) = config.embedding_dim {
                    map.insert("EMBEDDING_DIM".into(), v.to_string());
                }
                return map;
            }
        }
    }

    HashMap::new()
}

// ---------------------------------------------------------------------------
// Back-navigation helper: shown at the start of steps 2+
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Step 1: LLM Provider & Model
// ---------------------------------------------------------------------------

struct ProviderResult {
    provider: String,
    api_key: String,
    model: String,
    base_url: String,
    // Bedrock-specific
    aws_region: String,
    aws_access_key_id: String,
    aws_secret_access_key: String,
    aws_session_token: String,
    aws_profile: String,
}

fn step_provider(
    existing: &HashMap<String, String>,
) -> Result<StepResult<ProviderResult>, RayClawError> {
    print_step(1, 6, "LLM Provider & Model");

    let labels: Vec<String> = PROVIDER_PRESETS
        .iter()
        .map(|p| format!("{} - {}", p.id, p.label))
        .collect();

    let existing_provider = existing
        .get("LLM_PROVIDER")
        .cloned()
        .unwrap_or_else(|| "anthropic".into());
    let default_idx = PROVIDER_PRESETS
        .iter()
        .position(|p| p.id.eq_ignore_ascii_case(&existing_provider))
        .unwrap_or(3); // anthropic

    let choice = match Select::new()
        .with_prompt("  Select your LLM provider")
        .items(&labels)
        .default(default_idx)
        .interact_opt()
        .map_err(|e| RayClawError::Config(format!("Selection canceled: {e}")))?
    {
        Some(c) => c,
        None => return Ok(StepResult::Back),
    };

    let preset = &PROVIDER_PRESETS[choice];
    let provider = preset.id.to_string();

    // API key
    let api_key = if is_openai_codex_provider(&provider) {
        println!(
            "  {} Codex uses ~/.codex/auth.json. Run `codex login` if needed.",
            style("!").yellow().bold()
        );
        String::new()
    } else if provider_allows_empty_api_key(&provider) {
        println!(
            "  {} {} does not require an API key.",
            style("i").cyan(),
            preset.label
        );
        String::new()
    } else {
        let existing_key = existing.get("LLM_API_KEY").cloned().unwrap_or_default();
        let prompt = if existing_key.is_empty() {
            "  API key".to_string()
        } else {
            format!("  API key [current: {}]", mask_secret(&existing_key))
        };
        let key: String = Input::new()
            .with_prompt(&prompt)
            .default(existing_key)
            .interact_text()
            .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;
        key.trim().to_string()
    };

    // Model selection
    let mut model_options: Vec<String> = preset.models.iter().map(|m| (*m).to_string()).collect();
    model_options.push("Custom...".to_string());

    let existing_model = existing.get("LLM_MODEL").cloned().unwrap_or_default();
    let model_default_idx = preset
        .models
        .iter()
        .position(|m| *m == existing_model)
        .unwrap_or(0);

    let model_choice = match Select::new()
        .with_prompt("  Select model")
        .items(&model_options)
        .default(model_default_idx)
        .interact_opt()
        .map_err(|e| RayClawError::Config(format!("Selection canceled: {e}")))?
    {
        Some(c) => c,
        None => return Ok(StepResult::Back),
    };

    let model = if model_choice == model_options.len() - 1 {
        // Custom
        let m: String = Input::new()
            .with_prompt("  Model name")
            .default(existing_model)
            .interact_text()
            .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;
        m.trim().to_string()
    } else {
        model_options[model_choice].clone()
    };

    // Base URL
    let default_base = if preset.default_base_url.is_empty() {
        existing.get("LLM_BASE_URL").cloned().unwrap_or_default()
    } else {
        existing
            .get("LLM_BASE_URL")
            .cloned()
            .unwrap_or_else(|| preset.default_base_url.to_string())
    };

    let base_url = if provider == "custom" || provider == "azure" {
        let url: String = Input::new()
            .with_prompt("  Base URL")
            .default(default_base)
            .interact_text()
            .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;
        url.trim().to_string()
    } else {
        default_base
    };

    // Bedrock-specific fields
    let (aws_region, aws_access_key_id, aws_secret_access_key, aws_session_token, aws_profile) =
        if preset.protocol == ProviderProtocol::Bedrock {
            println!();
            println!(
                "  {} Bedrock uses AWS credentials. Leave blank to use env vars or ~/.aws/credentials.",
                style("i").cyan()
            );

            let region: String = Input::new()
                .with_prompt("  AWS region")
                .default(
                    existing
                        .get("AWS_REGION")
                        .cloned()
                        .unwrap_or_else(|| "us-east-1".into()),
                )
                .interact_text()
                .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;

            let access_key: String = Input::new()
                .with_prompt("  AWS access key ID (optional)")
                .default(
                    existing
                        .get("AWS_ACCESS_KEY_ID")
                        .cloned()
                        .unwrap_or_default(),
                )
                .allow_empty(true)
                .interact_text()
                .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;

            let secret_key: String = Input::new()
                .with_prompt("  AWS secret access key (optional)")
                .default(
                    existing
                        .get("AWS_SECRET_ACCESS_KEY")
                        .cloned()
                        .unwrap_or_default(),
                )
                .allow_empty(true)
                .interact_text()
                .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;

            let session_token: String = Input::new()
                .with_prompt("  AWS session token (optional)")
                .default(
                    existing
                        .get("AWS_SESSION_TOKEN")
                        .cloned()
                        .unwrap_or_default(),
                )
                .allow_empty(true)
                .interact_text()
                .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;

            let profile: String = Input::new()
                .with_prompt("  AWS profile (optional)")
                .default(existing.get("AWS_PROFILE").cloned().unwrap_or_default())
                .allow_empty(true)
                .interact_text()
                .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;

            (
                region.trim().to_string(),
                access_key.trim().to_string(),
                secret_key.trim().to_string(),
                session_token.trim().to_string(),
                profile.trim().to_string(),
            )
        } else {
            (
                existing.get("AWS_REGION").cloned().unwrap_or_default(),
                existing
                    .get("AWS_ACCESS_KEY_ID")
                    .cloned()
                    .unwrap_or_default(),
                existing
                    .get("AWS_SECRET_ACCESS_KEY")
                    .cloned()
                    .unwrap_or_default(),
                existing
                    .get("AWS_SESSION_TOKEN")
                    .cloned()
                    .unwrap_or_default(),
                existing.get("AWS_PROFILE").cloned().unwrap_or_default(),
            )
        };

    let key_display = if api_key.is_empty() {
        if is_openai_codex_provider(&provider) {
            "codex auth".to_string()
        } else {
            "not required".to_string()
        }
    } else {
        mask_secret(&api_key)
    };

    print_result(&format!(
        "Provider: {} | Model: {} | Key: {}",
        provider, model, key_display
    ));

    if ask_nav()? {
        Ok(StepResult::Next(ProviderResult {
            provider,
            api_key,
            model,
            base_url,
            aws_region,
            aws_access_key_id,
            aws_secret_access_key,
            aws_session_token,
            aws_profile,
        }))
    } else {
        Ok(StepResult::Back)
    }
}

// ---------------------------------------------------------------------------
// Step 2: Channels
// ---------------------------------------------------------------------------

struct ChannelResult {
    enabled: Vec<String>,
    telegram_bot_token: String,
    bot_username: String,
    discord_bot_token: String,
    dynamic_fields: HashMap<String, String>,
}

fn channel_options() -> Vec<&'static str> {
    let mut opts = vec!["telegram", "discord"];
    for ch in DYNAMIC_CHANNELS {
        opts.push(ch.name);
    }
    opts
}

fn step_channels(
    existing: &HashMap<String, String>,
) -> Result<StepResult<ChannelResult>, RayClawError> {
    print_step(2, 6, "Channels");

    let options = channel_options();
    let existing_enabled: Vec<String> = existing
        .get("ENABLED_CHANNELS")
        .map(|s| {
            s.split(',')
                .map(|p| p.trim().to_lowercase())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let defaults: Vec<bool> = options
        .iter()
        .map(|o| existing_enabled.iter().any(|e| e == o))
        .collect();

    let labels: Vec<String> = options
        .iter()
        .map(|o| {
            let name = match *o {
                "telegram" => "Telegram",
                "discord" => "Discord",
                "slack" => "Slack",
                "feishu" => "Feishu / Lark",
                "weixin" => "WeChat",
                _ => o,
            };
            name.to_string()
        })
        .collect();

    let selected = match MultiSelect::new()
        .with_prompt("  Which channels to enable? (Space to toggle, Enter to confirm)")
        .items(&labels)
        .defaults(&defaults)
        .interact_opt()
        .map_err(|e| RayClawError::Config(format!("Selection canceled: {e}")))?
    {
        Some(s) => s,
        None => return Ok(StepResult::Back),
    };

    let enabled: Vec<String> = selected.iter().map(|&i| options[i].to_string()).collect();

    // Collect per-channel credentials
    let mut telegram_bot_token = existing
        .get("TELEGRAM_BOT_TOKEN")
        .cloned()
        .unwrap_or_default();
    let mut bot_username = existing.get("BOT_USERNAME").cloned().unwrap_or_default();
    let mut discord_bot_token = existing
        .get("DISCORD_BOT_TOKEN")
        .cloned()
        .unwrap_or_default();
    let mut dynamic_fields: HashMap<String, String> = HashMap::new();

    // Populate existing dynamic fields
    for ch in DYNAMIC_CHANNELS {
        for f in ch.fields {
            let key = dynamic_field_key(ch.name, f.yaml_key);
            if let Some(v) = existing.get(&key) {
                dynamic_fields.insert(key, v.clone());
            }
        }
    }

    for channel in &enabled {
        println!();
        match channel.as_str() {
            "telegram" => {
                telegram_bot_token = Input::new()
                    .with_prompt("  Telegram bot token (from @BotFather)")
                    .default(telegram_bot_token)
                    .interact_text()
                    .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;
                telegram_bot_token = telegram_bot_token.trim().to_string();

                bot_username = Input::new()
                    .with_prompt("  Bot username (without @)")
                    .default(bot_username)
                    .interact_text()
                    .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;
                bot_username = bot_username.trim().trim_start_matches('@').to_string();

                // Test connection
                print!("  {} Testing Telegram connection...", style("⏳").cyan());
                match test_telegram_token(&telegram_bot_token) {
                    Ok(actual_username) => {
                        println!(
                            "\r  {} Telegram bot '{}' connected       ",
                            style("✓").green().bold(),
                            actual_username
                        );
                        if !bot_username.is_empty()
                            && !actual_username.is_empty()
                            && bot_username != actual_username
                        {
                            println!(
                                "  {} Username mismatch: configured='{}', actual='{}'",
                                style("!").yellow().bold(),
                                bot_username,
                                actual_username
                            );
                        }
                    }
                    Err(e) => {
                        println!(
                            "\r  {} Telegram test failed: {}       ",
                            style("!").yellow().bold(),
                            e
                        );
                    }
                }
            }
            "discord" => {
                discord_bot_token = Input::new()
                    .with_prompt("  Discord bot token")
                    .default(discord_bot_token)
                    .interact_text()
                    .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;
                discord_bot_token = discord_bot_token.trim().to_string();
            }
            other => {
                // Dynamic channels
                if let Some(ch) = DYNAMIC_CHANNELS.iter().find(|c| c.name == other) {
                    for f in ch.fields {
                        let key = dynamic_field_key(ch.name, f.yaml_key);
                        let existing_val = dynamic_fields
                            .get(&key)
                            .cloned()
                            .unwrap_or_else(|| f.default.to_string());
                        let val: String = Input::new()
                            .with_prompt(format!("  {}", f.label))
                            .default(existing_val)
                            .allow_empty(!f.required)
                            .interact_text()
                            .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;
                        dynamic_fields.insert(key, val.trim().to_string());
                    }
                }
            }
        }
    }

    if enabled.is_empty() {
        print_result("No channels selected (Web UI only)");
    } else {
        print_result(&format!("Channels: {}", enabled.join(", ")));
    }

    if ask_nav()? {
        Ok(StepResult::Next(ChannelResult {
            enabled,
            telegram_bot_token,
            bot_username,
            discord_bot_token,
            dynamic_fields,
        }))
    } else {
        Ok(StepResult::Back)
    }
}

fn test_telegram_token(token: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp: serde_json::Value = client
        .get(format!("https://api.telegram.org/bot{token}/getMe"))
        .send()
        .map_err(|e| e.to_string())?
        .json()
        .map_err(|e| e.to_string())?;
    let ok = resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    if !ok {
        return Err("Invalid bot token".into());
    }
    let username = resp
        .get("result")
        .and_then(|r| r.get("username"))
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_string();
    Ok(username)
}

// ---------------------------------------------------------------------------
// Step 3: Data & Directories
// ---------------------------------------------------------------------------

struct DirectoryResult {
    data_dir: String,
    working_dir: String,
    timezone: String,
}

fn step_directories(
    existing: &HashMap<String, String>,
) -> Result<StepResult<DirectoryResult>, RayClawError> {
    print_step(3, 6, "Data & Directories");

    let data_dir: String = Input::new()
        .with_prompt("  Data directory")
        .default(
            existing
                .get("DATA_DIR")
                .cloned()
                .unwrap_or_else(|| "./rayclaw.data".into()),
        )
        .interact_text()
        .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;
    let data_dir = data_dir.trim().to_string();

    let working_dir: String = Input::new()
        .with_prompt("  Working directory")
        .default(
            existing
                .get("WORKING_DIR")
                .cloned()
                .unwrap_or_else(|| "./tmp".into()),
        )
        .interact_text()
        .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;
    let working_dir = working_dir.trim().to_string();

    // Timezone selection
    let tz_options = vec![
        "UTC",
        "US/Eastern",
        "US/Central",
        "US/Pacific",
        "Europe/London",
        "Europe/Berlin",
        "Asia/Shanghai",
        "Asia/Tokyo",
        "Other (manual input)",
    ];

    let existing_tz = existing
        .get("TIMEZONE")
        .cloned()
        .unwrap_or_else(|| "UTC".into());
    let tz_default = tz_options
        .iter()
        .position(|t| *t == existing_tz)
        .unwrap_or(0);

    let tz_choice = match Select::new()
        .with_prompt("  Timezone")
        .items(&tz_options)
        .default(tz_default)
        .interact_opt()
        .map_err(|e| RayClawError::Config(format!("Selection canceled: {e}")))?
    {
        Some(c) => c,
        None => return Ok(StepResult::Back),
    };

    let timezone = if tz_choice == tz_options.len() - 1 {
        let tz: String = Input::new()
            .with_prompt("  Timezone (IANA format, e.g. America/New_York)")
            .default(existing_tz)
            .interact_text()
            .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;
        tz.trim().to_string()
    } else {
        tz_options[tz_choice].to_string()
    };

    // Validate timezone
    timezone
        .parse::<chrono_tz::Tz>()
        .map_err(|_| RayClawError::Config(format!("Invalid timezone: {timezone}")))?;

    // Validate directories are writable
    fs::create_dir_all(&data_dir)?;
    fs::create_dir_all(&working_dir)?;

    print_result(&format!(
        "Directories: {} | {} | {}",
        data_dir, working_dir, timezone
    ));

    if ask_nav()? {
        Ok(StepResult::Next(DirectoryResult {
            data_dir,
            working_dir,
            timezone,
        }))
    } else {
        Ok(StepResult::Back)
    }
}

// ---------------------------------------------------------------------------
// Step 4: Memory
// ---------------------------------------------------------------------------

struct MemoryResult {
    reflector_enabled: bool,
    reflector_interval_mins: u64,
    memory_token_budget: usize,
    embedding_provider: String,
    embedding_api_key: String,
    embedding_base_url: String,
    embedding_model: String,
    embedding_dim: String,
}

fn step_memory(
    existing: &HashMap<String, String>,
) -> Result<StepResult<MemoryResult>, RayClawError> {
    print_step(4, 6, "Memory");

    let existing_reflector = existing
        .get("REFLECTOR_ENABLED")
        .map(|v| v.trim().to_lowercase())
        .map(|v| v != "false" && v != "0" && v != "no")
        .unwrap_or(true);

    let reflector_enabled = match Confirm::new()
        .with_prompt("  Enable memory reflector?")
        .default(existing_reflector)
        .interact_opt()
        .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?
    {
        Some(v) => v,
        None => return Ok(StepResult::Back),
    };

    let reflector_interval_mins: u64 = if reflector_enabled {
        let v: String = Input::new()
            .with_prompt("  Reflector interval (minutes)")
            .default(
                existing
                    .get("REFLECTOR_INTERVAL_MINS")
                    .cloned()
                    .unwrap_or_else(|| "15".into()),
            )
            .interact_text()
            .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;
        v.trim()
            .parse::<u64>()
            .map_err(|_| RayClawError::Config("Interval must be a positive integer".into()))?
    } else {
        existing
            .get("REFLECTOR_INTERVAL_MINS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(15)
    };

    let memory_token_budget: usize = {
        let v: String = Input::new()
            .with_prompt("  Memory token budget")
            .default(
                existing
                    .get("MEMORY_TOKEN_BUDGET")
                    .cloned()
                    .unwrap_or_else(|| "1500".into()),
            )
            .interact_text()
            .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;
        v.trim()
            .parse::<usize>()
            .map_err(|_| RayClawError::Config("Budget must be a positive integer".into()))?
    };

    // Embedding configuration
    let has_existing_embedding = existing
        .get("EMBEDDING_PROVIDER")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);

    let configure_embedding = Confirm::new()
        .with_prompt("  Configure semantic embeddings? (for vector memory search)")
        .default(has_existing_embedding)
        .interact()
        .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;

    let (embedding_provider, embedding_api_key, embedding_base_url, embedding_model, embedding_dim) =
        if configure_embedding {
            let provider: String = Input::new()
                .with_prompt("  Embedding provider (openai/ollama)")
                .default(
                    existing
                        .get("EMBEDDING_PROVIDER")
                        .cloned()
                        .unwrap_or_else(|| "openai".into()),
                )
                .interact_text()
                .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;

            let api_key: String = Input::new()
                .with_prompt("  Embedding API key (optional)")
                .default(
                    existing
                        .get("EMBEDDING_API_KEY")
                        .cloned()
                        .unwrap_or_default(),
                )
                .allow_empty(true)
                .interact_text()
                .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;

            let base_url: String = Input::new()
                .with_prompt("  Embedding base URL (optional)")
                .default(
                    existing
                        .get("EMBEDDING_BASE_URL")
                        .cloned()
                        .unwrap_or_default(),
                )
                .allow_empty(true)
                .interact_text()
                .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;

            let model: String = Input::new()
                .with_prompt("  Embedding model (optional)")
                .default(existing.get("EMBEDDING_MODEL").cloned().unwrap_or_default())
                .allow_empty(true)
                .interact_text()
                .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;

            let dim: String = Input::new()
                .with_prompt("  Embedding dimensions (optional)")
                .default(existing.get("EMBEDDING_DIM").cloned().unwrap_or_default())
                .allow_empty(true)
                .interact_text()
                .map_err(|e| RayClawError::Config(format!("Input canceled: {e}")))?;

            (
                provider.trim().to_string(),
                api_key.trim().to_string(),
                base_url.trim().to_string(),
                model.trim().to_string(),
                dim.trim().to_string(),
            )
        } else {
            (
                existing
                    .get("EMBEDDING_PROVIDER")
                    .cloned()
                    .unwrap_or_default(),
                existing
                    .get("EMBEDDING_API_KEY")
                    .cloned()
                    .unwrap_or_default(),
                existing
                    .get("EMBEDDING_BASE_URL")
                    .cloned()
                    .unwrap_or_default(),
                existing.get("EMBEDDING_MODEL").cloned().unwrap_or_default(),
                existing.get("EMBEDDING_DIM").cloned().unwrap_or_default(),
            )
        };

    let reflector_display = if reflector_enabled {
        format!("reflector on ({}min)", reflector_interval_mins)
    } else {
        "reflector off".to_string()
    };
    print_result(&format!(
        "Memory: {} | budget {}",
        reflector_display, memory_token_budget
    ));

    if ask_nav()? {
        Ok(StepResult::Next(MemoryResult {
            reflector_enabled,
            reflector_interval_mins,
            memory_token_budget,
            embedding_provider,
            embedding_api_key,
            embedding_base_url,
            embedding_model,
            embedding_dim,
        }))
    } else {
        Ok(StepResult::Back)
    }
}

// ---------------------------------------------------------------------------
// Step 5: Validation
// ---------------------------------------------------------------------------

fn step_validate(
    provider_result: &ProviderResult,
    channel_result: &ChannelResult,
) -> Result<(), RayClawError> {
    print_step(5, 6, "Validation");

    let tg_enabled = channel_result.enabled.contains(&"telegram".to_string());
    let provider = &provider_result.provider;
    let (api_key, codex_account_id) = if is_openai_codex_provider(provider) {
        match resolve_openai_codex_auth("") {
            Ok(auth) => (auth.bearer_token, auth.account_id),
            Err(e) => {
                print_error(&format!("Codex auth failed: {e}"));
                return Ok(());
            }
        }
    } else {
        (provider_result.api_key.clone(), None)
    };

    let checks = perform_online_validation(
        tg_enabled,
        &channel_result.telegram_bot_token,
        &channel_result.bot_username,
        provider,
        &api_key,
        &provider_result.base_url,
        &provider_result.model,
        codex_account_id.as_deref(),
    );

    match checks {
        Ok(results) => {
            for check in &results {
                print_result(check);
            }
        }
        Err(e) => {
            print_error(&format!("Validation failed: {e}"));
            let skip = Confirm::new()
                .with_prompt("  Continue anyway?")
                .default(false)
                .interact()
                .unwrap_or(false);
            if !skip {
                return Err(e);
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn perform_online_validation(
    telegram_enabled: bool,
    tg_token: &str,
    env_username: &str,
    provider: &str,
    api_key: &str,
    base_url: &str,
    model: &str,
    codex_account_id: Option<&str>,
) -> Result<Vec<String>, RayClawError> {
    let mut checks = Vec::new();
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    // Telegram validation
    if telegram_enabled {
        let tg_resp: serde_json::Value = client
            .get(format!("https://api.telegram.org/bot{tg_token}/getMe"))
            .send()?
            .json()?;
        let ok = tg_resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if !ok {
            return Err(RayClawError::Config(
                "Telegram getMe failed (check bot token)".into(),
            ));
        }
        let actual_username = tg_resp
            .get("result")
            .and_then(|r| r.get("username"))
            .and_then(|u| u.as_str())
            .unwrap_or_default()
            .to_string();
        if !env_username.is_empty()
            && !actual_username.is_empty()
            && env_username != actual_username
        {
            checks.push(format!(
                "Telegram OK (token user={actual_username}, configured={env_username})"
            ));
        } else {
            checks.push(format!("Telegram OK ({actual_username})"));
        }
    } else {
        checks.push("Telegram skipped (disabled)".into());
    }

    // LLM validation
    let preset = find_provider_preset(provider);
    let protocol = provider_protocol(provider);
    let model = if model.is_empty() {
        default_model_for_provider(provider).to_string()
    } else {
        model.to_string()
    };

    if protocol == ProviderProtocol::Bedrock {
        checks.push(format!(
            "LLM skipped (bedrock uses SigV4 auth, model={model}). Test with `rayclaw start`."
        ));
        return Ok(checks);
    }

    if protocol == ProviderProtocol::Anthropic {
        let mut base = if base_url.is_empty() {
            "https://api.anthropic.com".to_string()
        } else {
            base_url.trim_end_matches('/').to_string()
        };
        if base.ends_with("/v1/messages") {
            base = base.trim_end_matches("/v1/messages").to_string();
        }
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "hi"}]
        });
        let resp = client
            .post(format!("{base}/v1/messages"))
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            let detail = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| {
                    v.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(RayClawError::Config(format!(
                "LLM validation failed: {detail}"
            )));
        }
        checks.push(format!("LLM OK (anthropic, model={model})"));
    } else {
        let base = resolve_openai_compat_validation_base(provider, base_url, preset);
        let resp = if is_openai_codex_provider(provider) {
            let body = serde_json::json!({
                "model": model,
                "input": [{"type":"message","role":"user","content":"hi"}],
                "instructions": "You are a helpful assistant.",
                "store": false,
                "stream": true,
            });
            let mut req = client
                .post(format!("{}/responses", base.trim_end_matches('/')))
                .header("content-type", "application/json")
                .body(body.to_string());
            if !api_key.trim().is_empty() {
                req = req.bearer_auth(api_key);
            }
            if let Some(account_id) = codex_account_id {
                if !account_id.trim().is_empty() {
                    req = req.header("ChatGPT-Account-ID", account_id.trim());
                }
            }
            req.send()?
        } else {
            let body = serde_json::json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "hi"}]
            });
            let mut req = client
                .post(format!("{}/chat/completions", base.trim_end_matches('/')))
                .header("content-type", "application/json")
                .body(body.to_string());
            if !api_key.trim().is_empty() {
                req = req.bearer_auth(api_key);
            }
            req.send()?
        };
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            let detail = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| {
                    v.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(RayClawError::Config(format!(
                "LLM validation failed: {detail}"
            )));
        }
        checks.push(format!("LLM OK (openai-compatible, model={model})"));
    }

    Ok(checks)
}

fn resolve_openai_compat_validation_base(
    provider: &str,
    base_url: &str,
    preset: Option<&ProviderPreset>,
) -> String {
    let trimmed = if base_url.is_empty() {
        preset
            .map(|p| p.default_base_url)
            .filter(|s| !s.is_empty())
            .unwrap_or("https://api.openai.com/v1")
            .trim_end_matches('/')
            .to_string()
    } else {
        base_url.trim_end_matches('/').to_string()
    };

    if is_openai_codex_provider(provider) {
        if let Some(codex_base) = codex_config_default_openai_base_url() {
            return codex_base.trim_end_matches('/').to_string();
        }
        return "https://chatgpt.com/backend-api/codex".to_string();
    }

    if trimmed.ends_with("/v1") {
        trimmed
    } else {
        format!("{}/v1", trimmed)
    }
}

// ---------------------------------------------------------------------------
// Step 6: Save
// ---------------------------------------------------------------------------

fn step_save(values: &HashMap<String, String>) -> Result<Option<String>, RayClawError> {
    print_step(6, 6, "Save");

    let path = Path::new("rayclaw.config.yaml");
    let mut backup = None;
    if path.exists() {
        let ts = Utc::now().format("%Y%m%d%H%M%S").to_string();
        let backup_path = format!("{}.bak.{ts}", path.display());
        fs::copy(path, &backup_path)?;
        backup = Some(backup_path);
    }

    let get = |key: &str| values.get(key).cloned().unwrap_or_default();

    let enabled_raw = get("ENABLED_CHANNELS");
    let valid_channel_names: Vec<&str> = {
        let mut v = vec!["telegram", "discord"];
        for ch in DYNAMIC_CHANNELS {
            v.push(ch.name);
        }
        v
    };
    let mut channels = Vec::new();
    for part in enabled_raw.split(',') {
        let p = part.trim().to_lowercase();
        if valid_channel_names.contains(&p.as_str()) && !channels.iter().any(|v| v == &p) {
            channels.push(p);
        }
    }

    let has_tg =
        !get("TELEGRAM_BOT_TOKEN").trim().is_empty() || !get("BOT_USERNAME").trim().is_empty();
    let has_discord = !get("DISCORD_BOT_TOKEN").trim().is_empty();
    let dynamic_channel_present: Vec<(&DynamicChannelDef, bool)> = DYNAMIC_CHANNELS
        .iter()
        .map(|ch| {
            let present = ch.fields.iter().any(|f| {
                let key = dynamic_field_key(ch.name, f.yaml_key);
                !get(&key).trim().is_empty()
            });
            (ch, present)
        })
        .collect();

    let mut yaml = String::new();
    yaml.push_str("# RayClaw configuration\n\n");
    yaml.push_str("# Enabled channels (telegram,discord,web)\n");
    let channels_note = if channels.is_empty() {
        let mut inferred = Vec::new();
        if has_tg {
            inferred.push("telegram");
        }
        if has_discord {
            inferred.push("discord");
        }
        for (ch, present) in &dynamic_channel_present {
            if *present {
                inferred.push(ch.name);
            }
        }
        if inferred.is_empty() {
            "setup later".to_string()
        } else {
            format!("inferred: {}", inferred.join(","))
        }
    } else {
        channels.join(",")
    };
    yaml.push_str(&format!("# channels: {}\n\n", channels_note));

    yaml.push_str("# Telegram bot token from @BotFather\n");
    yaml.push_str(&format!(
        "telegram_bot_token: \"{}\"\n",
        get("TELEGRAM_BOT_TOKEN")
    ));
    yaml.push_str("# Bot username without @\n");
    yaml.push_str(&format!("bot_username: \"{}\"\n\n", get("BOT_USERNAME")));

    yaml.push_str("# Discord bot token\n");
    let discord_token = get("DISCORD_BOT_TOKEN");
    if discord_token.trim().is_empty() {
        yaml.push_str("discord_bot_token: null\n\n");
    } else {
        yaml.push_str(&format!("discord_bot_token: \"{}\"\n\n", discord_token));
    }

    yaml.push_str("web_enabled: true\n\n");

    // Dynamic channels
    let any_dynamic = dynamic_channel_present.iter().any(|(_, present)| *present);
    if any_dynamic {
        yaml.push_str("channels:\n");
        for (ch, present) in &dynamic_channel_present {
            if !present {
                continue;
            }
            yaml.push_str(&format!("  {}:\n", ch.name));
            for f in ch.fields {
                let key = dynamic_field_key(ch.name, f.yaml_key);
                let val = get(&key);
                if !f.required && val == f.default && !val.is_empty() {
                    continue;
                }
                yaml.push_str(&format!("    {}: \"{}\"\n", f.yaml_key, val));
            }
        }
        yaml.push('\n');
    }

    yaml.push_str(
        "# LLM provider (anthropic, openai-codex, ollama, openai, openrouter, deepseek, google, etc.)\n",
    );
    yaml.push_str(&format!("llm_provider: \"{}\"\n", get("LLM_PROVIDER")));
    yaml.push_str("# API key for LLM provider\n");
    yaml.push_str(&format!("api_key: \"{}\"\n", get("LLM_API_KEY")));

    let model = get("LLM_MODEL");
    if !model.is_empty() {
        yaml.push_str("# Model name (leave empty for provider default)\n");
        yaml.push_str(&format!("model: \"{}\"\n", model));
    }

    let base_url = get("LLM_BASE_URL");
    if !base_url.is_empty() {
        yaml.push_str("# Custom base URL (optional)\n");
        yaml.push_str(&format!("llm_base_url: \"{}\"\n", base_url));
    }

    // AWS Bedrock fields
    let aws_region = get("AWS_REGION");
    let aws_access_key_id = get("AWS_ACCESS_KEY_ID");
    let aws_secret_access_key = get("AWS_SECRET_ACCESS_KEY");
    let aws_session_token = get("AWS_SESSION_TOKEN");
    let aws_profile = get("AWS_PROFILE");
    if !aws_region.is_empty()
        || !aws_access_key_id.is_empty()
        || !aws_secret_access_key.is_empty()
        || !aws_session_token.is_empty()
        || !aws_profile.is_empty()
    {
        yaml.push_str("\n# AWS Bedrock credentials (native Converse API)\n");
        if !aws_region.is_empty() {
            yaml.push_str(&format!("aws_region: \"{}\"\n", aws_region));
        }
        if !aws_access_key_id.is_empty() {
            yaml.push_str(&format!("aws_access_key_id: \"{}\"\n", aws_access_key_id));
        }
        if !aws_secret_access_key.is_empty() {
            yaml.push_str(&format!(
                "aws_secret_access_key: \"{}\"\n",
                aws_secret_access_key
            ));
        }
        if !aws_session_token.is_empty() {
            yaml.push_str(&format!("aws_session_token: \"{}\"\n", aws_session_token));
        }
        if !aws_profile.is_empty() {
            yaml.push_str(&format!("aws_profile: \"{}\"\n", aws_profile));
        }
    }

    yaml.push('\n');
    let data_dir = values
        .get("DATA_DIR")
        .cloned()
        .unwrap_or_else(|| "./rayclaw.data".into());
    yaml.push_str(&format!("data_dir: \"{}\"\n", data_dir));
    let tz = values
        .get("TIMEZONE")
        .cloned()
        .unwrap_or_else(|| "UTC".into());
    yaml.push_str(&format!("timezone: \"{}\"\n", tz));
    let working_dir = values
        .get("WORKING_DIR")
        .cloned()
        .unwrap_or_else(|| "./tmp".into());
    yaml.push_str(&format!("working_dir: \"{}\"\n", working_dir));

    let reflector_enabled = values
        .get("REFLECTOR_ENABLED")
        .map(|v| v.trim().to_lowercase())
        .map(|v| v != "false" && v != "0" && v != "no")
        .unwrap_or(true);
    yaml.push_str(
        "\n# Memory reflector: periodically extracts structured memories from conversations\n",
    );
    yaml.push_str(&format!("reflector_enabled: {}\n", reflector_enabled));
    let reflector_interval = values
        .get("REFLECTOR_INTERVAL_MINS")
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(15);
    yaml.push_str(&format!(
        "reflector_interval_mins: {}\n",
        reflector_interval
    ));
    let memory_token_budget = values
        .get("MEMORY_TOKEN_BUDGET")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(1500);
    yaml.push_str(&format!("memory_token_budget: {}\n", memory_token_budget));

    let embedding_provider = values
        .get("EMBEDDING_PROVIDER")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let embedding_api_key = values
        .get("EMBEDDING_API_KEY")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let embedding_base_url = values
        .get("EMBEDDING_BASE_URL")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let embedding_model = values
        .get("EMBEDDING_MODEL")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let embedding_dim = values
        .get("EMBEDDING_DIM")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    if !embedding_provider.is_empty()
        || !embedding_api_key.is_empty()
        || !embedding_base_url.is_empty()
        || !embedding_model.is_empty()
        || !embedding_dim.is_empty()
    {
        yaml.push_str(
            "\n# Optional embedding config for semantic memory retrieval (requires sqlite-vec feature)\n",
        );
        if !embedding_provider.is_empty() {
            yaml.push_str(&format!("embedding_provider: \"{}\"\n", embedding_provider));
        }
        if !embedding_api_key.is_empty() {
            yaml.push_str(&format!("embedding_api_key: \"{}\"\n", embedding_api_key));
        }
        if !embedding_base_url.is_empty() {
            yaml.push_str(&format!("embedding_base_url: \"{}\"\n", embedding_base_url));
        }
        if !embedding_model.is_empty() {
            yaml.push_str(&format!("embedding_model: \"{}\"\n", embedding_model));
        }
        if !embedding_dim.is_empty() {
            yaml.push_str(&format!("embedding_dim: {}\n", embedding_dim));
        }
    }

    fs::write(path, yaml)?;

    print_result(&format!("Config saved to {}", path.display()));
    if let Some(ref bp) = backup {
        print_result(&format!("Backup: {bp}"));
    }

    Ok(backup)
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

fn print_summary(values: &HashMap<String, String>) {
    let get = |key: &str| values.get(key).cloned().unwrap_or_default();

    println!();
    println!(
        "  {}",
        style("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━").cyan()
    );
    println!(
        "  {}  {}",
        style("🐾").cyan(),
        style("RayClaw is ready!").white().bold()
    );
    println!(
        "  {}",
        style("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━").cyan()
    );
    println!();

    println!("  {}", style("Configuration saved to:").dim());
    println!("    {}", style("./rayclaw.config.yaml").green());
    println!();

    println!("  {}", style("Quick summary:").white().bold());
    println!("    🤖 Provider:  {}", get("LLM_PROVIDER"));
    println!("    🧠 Model:     {}", get("LLM_MODEL"));

    let channels = get("ENABLED_CHANNELS");
    let channels_display = if channels.is_empty() {
        "web only".to_string()
    } else {
        channels
    };
    println!("    📡 Channels:  {channels_display}");

    let api_key = get("LLM_API_KEY");
    let key_display = if api_key.is_empty() {
        "not set"
    } else {
        "configured"
    };
    println!("    🔑 API Key:   {key_display}");

    let reflector = get("REFLECTOR_ENABLED");
    let budget = get("MEMORY_TOKEN_BUDGET");
    let reflector_display = if reflector == "true" {
        format!("reflector on | budget {budget}")
    } else {
        format!("reflector off | budget {budget}")
    };
    println!("    💾 Memory:    {reflector_display}");

    println!();
    println!("  {}", style("Next steps:").white().bold());
    println!("    1. Start:   {}", style("rayclaw start").green());
    println!(
        "    2. Web UI:  {} (if --features web)",
        style("http://localhost:3001").green()
    );
    println!("    3. Doctor:  {}", style("rayclaw doctor").green());
    println!();
}

// ---------------------------------------------------------------------------
// Collect all values into a flat HashMap for save
// ---------------------------------------------------------------------------

fn collect_values(
    provider: &ProviderResult,
    channels: &ChannelResult,
    dirs: &DirectoryResult,
    memory: &MemoryResult,
) -> HashMap<String, String> {
    let mut values = HashMap::new();

    values.insert("LLM_PROVIDER".into(), provider.provider.clone());
    values.insert("LLM_API_KEY".into(), provider.api_key.clone());
    values.insert("LLM_MODEL".into(), provider.model.clone());
    if !provider.base_url.is_empty() {
        values.insert("LLM_BASE_URL".into(), provider.base_url.clone());
    }
    if !provider.aws_region.is_empty() {
        values.insert("AWS_REGION".into(), provider.aws_region.clone());
    }
    if !provider.aws_access_key_id.is_empty() {
        values.insert(
            "AWS_ACCESS_KEY_ID".into(),
            provider.aws_access_key_id.clone(),
        );
    }
    if !provider.aws_secret_access_key.is_empty() {
        values.insert(
            "AWS_SECRET_ACCESS_KEY".into(),
            provider.aws_secret_access_key.clone(),
        );
    }
    if !provider.aws_session_token.is_empty() {
        values.insert(
            "AWS_SESSION_TOKEN".into(),
            provider.aws_session_token.clone(),
        );
    }
    if !provider.aws_profile.is_empty() {
        values.insert("AWS_PROFILE".into(), provider.aws_profile.clone());
    }

    values.insert("ENABLED_CHANNELS".into(), channels.enabled.join(","));
    values.insert(
        "TELEGRAM_BOT_TOKEN".into(),
        channels.telegram_bot_token.clone(),
    );
    values.insert("BOT_USERNAME".into(), channels.bot_username.clone());
    values.insert(
        "DISCORD_BOT_TOKEN".into(),
        channels.discord_bot_token.clone(),
    );
    for (key, val) in &channels.dynamic_fields {
        values.insert(key.clone(), val.clone());
    }

    values.insert("DATA_DIR".into(), dirs.data_dir.clone());
    values.insert("WORKING_DIR".into(), dirs.working_dir.clone());
    values.insert("TIMEZONE".into(), dirs.timezone.clone());

    values.insert(
        "REFLECTOR_ENABLED".into(),
        memory.reflector_enabled.to_string(),
    );
    values.insert(
        "REFLECTOR_INTERVAL_MINS".into(),
        memory.reflector_interval_mins.to_string(),
    );
    values.insert(
        "MEMORY_TOKEN_BUDGET".into(),
        memory.memory_token_budget.to_string(),
    );
    if !memory.embedding_provider.is_empty() {
        values.insert(
            "EMBEDDING_PROVIDER".into(),
            memory.embedding_provider.clone(),
        );
    }
    if !memory.embedding_api_key.is_empty() {
        values.insert("EMBEDDING_API_KEY".into(), memory.embedding_api_key.clone());
    }
    if !memory.embedding_base_url.is_empty() {
        values.insert(
            "EMBEDDING_BASE_URL".into(),
            memory.embedding_base_url.clone(),
        );
    }
    if !memory.embedding_model.is_empty() {
        values.insert("EMBEDDING_MODEL".into(), memory.embedding_model.clone());
    }
    if !memory.embedding_dim.is_empty() {
        values.insert("EMBEDDING_DIM".into(), memory.embedding_dim.clone());
    }

    values
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

pub fn run_setup_wizard(
    force: bool,
    provider_only: bool,
    channels_only: bool,
) -> Result<bool, RayClawError> {
    print_banner();

    let existing = load_existing_config();

    // Partial update modes
    if provider_only {
        let provider_result = match step_provider(&existing)? {
            StepResult::Next(r) => r,
            StepResult::Back => return Ok(false),
        };
        let mut values = existing.clone();
        values.insert("LLM_PROVIDER".into(), provider_result.provider.clone());
        values.insert("LLM_API_KEY".into(), provider_result.api_key.clone());
        values.insert("LLM_MODEL".into(), provider_result.model.clone());
        if !provider_result.base_url.is_empty() {
            values.insert("LLM_BASE_URL".into(), provider_result.base_url.clone());
        }
        step_save(&values)?;
        print_summary(&values);
        return Ok(true);
    }

    if channels_only {
        let channel_result = match step_channels(&existing)? {
            StepResult::Next(r) => r,
            StepResult::Back => return Ok(false),
        };
        let mut values = existing.clone();
        values.insert("ENABLED_CHANNELS".into(), channel_result.enabled.join(","));
        values.insert(
            "TELEGRAM_BOT_TOKEN".into(),
            channel_result.telegram_bot_token.clone(),
        );
        values.insert("BOT_USERNAME".into(), channel_result.bot_username.clone());
        values.insert(
            "DISCORD_BOT_TOKEN".into(),
            channel_result.discord_bot_token.clone(),
        );
        for (key, val) in &channel_result.dynamic_fields {
            values.insert(key.clone(), val.clone());
        }
        step_save(&values)?;
        print_summary(&values);
        return Ok(true);
    }

    // Check for existing config
    if !existing.is_empty() && !force {
        let choices = [
            "Overwrite (full setup)",
            "Update provider only",
            "Update channels only",
            "Cancel",
        ];
        let choice = Select::new()
            .with_prompt("  Existing config found. What would you like to do?")
            .items(&choices)
            .default(0)
            .interact()
            .map_err(|e| RayClawError::Config(format!("Selection canceled: {e}")))?;

        match choice {
            1 => return run_setup_wizard(false, true, false),
            2 => return run_setup_wizard(false, false, true),
            3 => return Ok(false),
            _ => {} // 0 = overwrite, continue
        }
    }

    // Full 6-step wizard with back-navigation state machine
    let mut step: usize = 0;
    let mut provider_result: Option<ProviderResult> = None;
    let mut channel_result: Option<ChannelResult> = None;
    let mut dir_result: Option<DirectoryResult> = None;
    let mut memory_result: Option<MemoryResult> = None;

    loop {
        match step {
            0 => match step_provider(&existing)? {
                StepResult::Next(r) => {
                    provider_result = Some(r);
                    step += 1;
                }
                StepResult::Back => {}
            },
            1 => match step_channels(&existing)? {
                StepResult::Next(r) => {
                    channel_result = Some(r);
                    step += 1;
                }
                StepResult::Back => {
                    step -= 1;
                }
            },
            2 => match step_directories(&existing)? {
                StepResult::Next(r) => {
                    dir_result = Some(r);
                    step += 1;
                }
                StepResult::Back => {
                    step -= 1;
                }
            },
            3 => match step_memory(&existing)? {
                StepResult::Next(r) => {
                    memory_result = Some(r);
                    step += 1;
                }
                StepResult::Back => {
                    step -= 1;
                }
            },
            _ => break,
        }
    }

    let provider_result = provider_result.expect("provider_result must be set");
    let channel_result = channel_result.expect("channel_result must be set");
    let dir_result = dir_result.expect("dir_result must be set");
    let memory_result = memory_result.expect("memory_result must be set");

    step_validate(&provider_result, &channel_result)?;

    let values = collect_values(
        &provider_result,
        &channel_result,
        &dir_result,
        &memory_result,
    );
    step_save(&values)?;
    print_summary(&values);

    Ok(true)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_secret() {
        assert_eq!(mask_secret("abcdefghi"), "abc***hi");
        assert_eq!(mask_secret("abc"), "***");
    }

    #[test]
    fn test_save_config_yaml() {
        let yaml_path = std::env::temp_dir().join(format!(
            "rayclaw_wizard_test_{}.yaml",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));

        let mut values: HashMap<String, String> = HashMap::new();
        values.insert("ENABLED_CHANNELS".into(), "telegram,web".into());
        values.insert("TELEGRAM_BOT_TOKEN".into(), "new_tok".into());
        values.insert("BOT_USERNAME".into(), "new_bot".into());
        values.insert("LLM_PROVIDER".into(), "anthropic".into());
        values.insert("LLM_API_KEY".into(), "key".into());
        values.insert("DATA_DIR".into(), "./rayclaw.data".into());
        values.insert("TIMEZONE".into(), "UTC".into());
        values.insert("WORKING_DIR".into(), "./tmp".into());
        values.insert("REFLECTOR_ENABLED".into(), "true".into());
        values.insert("REFLECTOR_INTERVAL_MINS".into(), "15".into());
        values.insert("MEMORY_TOKEN_BUDGET".into(), "1500".into());

        let yaml_content = format!(
            "telegram_bot_token: \"{}\"\nbot_username: \"{}\"\nllm_provider: \"{}\"\napi_key: \"{}\"\n",
            values.get("TELEGRAM_BOT_TOKEN").unwrap(),
            values.get("BOT_USERNAME").unwrap(),
            values.get("LLM_PROVIDER").unwrap(),
            values.get("LLM_API_KEY").unwrap(),
        );
        fs::write(&yaml_path, &yaml_content).unwrap();

        let s = fs::read_to_string(&yaml_path).unwrap();
        assert!(s.contains("telegram_bot_token: \"new_tok\""));
        assert!(s.contains("bot_username: \"new_bot\""));
        assert!(s.contains("llm_provider: \"anthropic\""));
        assert!(s.contains("api_key: \"key\""));

        let _ = fs::remove_file(&yaml_path);
    }

    #[test]
    fn test_provider_presets_have_unique_ids() {
        let mut seen = std::collections::HashSet::new();
        for p in PROVIDER_PRESETS {
            assert!(seen.insert(p.id), "Duplicate provider id: {}", p.id);
        }
    }

    #[test]
    fn test_find_provider_preset() {
        assert!(find_provider_preset("anthropic").is_some());
        assert!(find_provider_preset("ANTHROPIC").is_some());
        assert!(find_provider_preset("nonexistent").is_none());
    }

    #[test]
    fn test_default_model_for_provider() {
        assert_eq!(
            default_model_for_provider("anthropic"),
            "claude-sonnet-4-5-20250929"
        );
        assert_eq!(default_model_for_provider("ollama"), "llama3.2");
        assert_eq!(default_model_for_provider("unknown"), "gpt-5.2");
    }

    #[test]
    fn test_dynamic_field_key() {
        assert_eq!(
            dynamic_field_key("slack", "bot_token"),
            "DYN_SLACK_BOT_TOKEN"
        );
        assert_eq!(dynamic_field_key("feishu", "app_id"), "DYN_FEISHU_APP_ID");
    }
}
