use crate::config::Config;
use crate::logging;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const LINUX_SERVICE_NAME: &str = "rayclaw-gateway.service";
const MAC_LABEL: &str = "ai.rayclaw.gateway";
const LOG_STDOUT_FILE: &str = "rayclaw-gateway.log";
const LOG_STDERR_FILE: &str = "rayclaw-gateway.error.log";
const DEFAULT_LOG_LINES: usize = 200;

#[derive(Debug, Clone)]
struct ServiceContext {
    exe_path: PathBuf,
    working_dir: PathBuf,
    config_path: Option<PathBuf>,
    runtime_logs_dir: PathBuf,
}

pub fn handle_gateway_cli(args: &[String]) -> Result<()> {
    let Some(action) = args.first().map(|s| s.as_str()) else {
        print_gateway_help();
        return Ok(());
    };

    match action {
        "install" => install(),
        "uninstall" => uninstall(),
        "start" => start(),
        "stop" => stop(),
        "status" => status(),
        "logs" => logs(args.get(1).map(|s| s.as_str())),
        "help" | "--help" | "-h" => {
            print_gateway_help();
            Ok(())
        }
        _ => Err(anyhow!(
            "Unknown gateway action: {}. Use: gateway <install|uninstall|start|stop|status|logs>",
            action
        )),
    }
}

pub fn print_gateway_help() {
    println!(
        r#"Gateway service management

USAGE:
    rayclaw gateway <ACTION>

ACTIONS:
    install      Install and enable persistent gateway service
    uninstall    Disable and remove persistent gateway service
    start        Start gateway service
    stop         Stop gateway service
    status       Show gateway service status
    logs [N]     Show last N lines of gateway logs (default: 200)
    help         Show this message
"#
    );
}

fn install() -> Result<()> {
    let ctx = build_context()?;
    if cfg!(target_os = "macos") {
        install_macos(&ctx)
    } else if cfg!(target_os = "linux") {
        install_linux(&ctx)
    } else {
        Err(anyhow!(
            "Gateway service is only supported on macOS and Linux"
        ))
    }
}

fn uninstall() -> Result<()> {
    if cfg!(target_os = "macos") {
        uninstall_macos()
    } else if cfg!(target_os = "linux") {
        uninstall_linux()
    } else {
        Err(anyhow!(
            "Gateway service is only supported on macOS and Linux"
        ))
    }
}

fn start() -> Result<()> {
    if cfg!(target_os = "macos") {
        start_macos()
    } else if cfg!(target_os = "linux") {
        start_linux()
    } else {
        Err(anyhow!(
            "Gateway service is only supported on macOS and Linux"
        ))
    }
}

fn stop() -> Result<()> {
    if cfg!(target_os = "macos") {
        stop_macos()
    } else if cfg!(target_os = "linux") {
        stop_linux()
    } else {
        Err(anyhow!(
            "Gateway service is only supported on macOS and Linux"
        ))
    }
}

fn status() -> Result<()> {
    if cfg!(target_os = "macos") {
        status_macos()
    } else if cfg!(target_os = "linux") {
        status_linux()
    } else {
        Err(anyhow!(
            "Gateway service is only supported on macOS and Linux"
        ))
    }
}

fn logs(lines_arg: Option<&str>) -> Result<()> {
    let lines = parse_log_lines(lines_arg)?;
    let ctx = build_context()?;
    println!("== gateway logs: {} ==", ctx.runtime_logs_dir.display());
    let tailed = logging::read_last_lines_from_logs(&ctx.runtime_logs_dir, lines)?;
    if tailed.is_empty() {
        println!("(no log lines found)");
    } else {
        println!("{}", tailed.join("\n"));
    }
    Ok(())
}

fn parse_log_lines(lines_arg: Option<&str>) -> Result<usize> {
    match lines_arg {
        None => Ok(DEFAULT_LOG_LINES),
        Some(raw) => {
            let parsed = raw
                .parse::<usize>()
                .with_context(|| format!("Invalid log line count: {}", raw))?;
            if parsed == 0 {
                return Err(anyhow!("Log line count must be greater than 0"));
            }
            Ok(parsed)
        }
    }
}

fn build_context() -> Result<ServiceContext> {
    let exe_path = std::env::current_exe().context("Failed to resolve current binary path")?;
    let working_dir = std::env::current_dir().context("Failed to resolve current directory")?;
    let config_path = resolve_config_path(&working_dir);
    let runtime_logs_dir = resolve_runtime_logs_dir(&working_dir);

    Ok(ServiceContext {
        exe_path,
        working_dir,
        config_path,
        runtime_logs_dir,
    })
}

fn resolve_config_path(cwd: &Path) -> Option<PathBuf> {
    if let Ok(from_env) = std::env::var("RAYCLAW_CONFIG") {
        let path = PathBuf::from(from_env);
        return Some(if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        });
    }

    for candidate in ["rayclaw.config.yaml", "rayclaw.config.yml"] {
        let path = cwd.join(candidate);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn resolve_runtime_logs_dir(cwd: &Path) -> PathBuf {
    match Config::load() {
        Ok(cfg) => PathBuf::from(cfg.runtime_data_dir()).join("logs"),
        Err(_) => cwd.join("runtime").join("logs"),
    }
}

fn run_command(cmd: &str, args: &[&str]) -> Result<std::process::Output> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("Failed to execute command: {} {}", cmd, args.join(" ")))?;
    Ok(output)
}

fn ensure_success(output: std::process::Output, cmd: &str, args: &[&str]) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(anyhow!(
        "Command failed: {} {}\nstdout: {}\nstderr: {}",
        cmd,
        args.join(" "),
        stdout.trim(),
        stderr.trim()
    ))
}

fn linux_unit_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("systemd")
        .join("user")
        .join(LINUX_SERVICE_NAME))
}

fn render_linux_unit(ctx: &ServiceContext) -> String {
    let mut unit = String::new();
    unit.push_str("[Unit]\n");
    unit.push_str("Description=RayClaw Gateway Service\n");
    unit.push_str("After=network.target\n\n");
    unit.push_str("[Service]\n");
    unit.push_str("Type=simple\n");
    unit.push_str(&format!("WorkingDirectory={}\n", ctx.working_dir.display()));
    unit.push_str(&format!("ExecStart={} start\n", ctx.exe_path.display()));
    unit.push_str("Environment=RAYCLAW_GATEWAY=1\n");
    if let Some(config_path) = &ctx.config_path {
        unit.push_str(&format!(
            "Environment=RAYCLAW_CONFIG={}\n",
            config_path.display()
        ));
    }
    unit.push_str("Restart=always\n");
    unit.push_str("RestartSec=5\n\n");
    unit.push_str("[Install]\n");
    unit.push_str("WantedBy=default.target\n");
    unit
}

fn install_linux(ctx: &ServiceContext) -> Result<()> {
    let unit_path = linux_unit_path()?;
    let unit_dir = unit_path
        .parent()
        .ok_or_else(|| anyhow!("Invalid unit path"))?;
    std::fs::create_dir_all(unit_dir)
        .with_context(|| format!("Failed to create {}", unit_dir.display()))?;
    std::fs::write(&unit_path, render_linux_unit(ctx))
        .with_context(|| format!("Failed to write {}", unit_path.display()))?;

    ensure_success(
        run_command("systemctl", &["--user", "daemon-reload"])?,
        "systemctl",
        &["--user", "daemon-reload"],
    )?;
    ensure_success(
        run_command(
            "systemctl",
            &["--user", "enable", "--now", LINUX_SERVICE_NAME],
        )?,
        "systemctl",
        &["--user", "enable", "--now", LINUX_SERVICE_NAME],
    )?;

    println!(
        "Installed and started gateway service: {}",
        unit_path.display()
    );
    Ok(())
}

fn uninstall_linux() -> Result<()> {
    let _ = run_command(
        "systemctl",
        &["--user", "disable", "--now", LINUX_SERVICE_NAME],
    );
    let _ = run_command("systemctl", &["--user", "daemon-reload"]);

    let unit_path = linux_unit_path()?;
    if unit_path.exists() {
        std::fs::remove_file(&unit_path)
            .with_context(|| format!("Failed to remove {}", unit_path.display()))?;
    }
    let _ = run_command("systemctl", &["--user", "daemon-reload"]);
    println!("Uninstalled gateway service");
    Ok(())
}

fn start_linux() -> Result<()> {
    ensure_success(
        run_command("systemctl", &["--user", "start", LINUX_SERVICE_NAME])?,
        "systemctl",
        &["--user", "start", LINUX_SERVICE_NAME],
    )?;
    println!("Gateway service started");
    Ok(())
}

fn stop_linux() -> Result<()> {
    ensure_success(
        run_command("systemctl", &["--user", "stop", LINUX_SERVICE_NAME])?,
        "systemctl",
        &["--user", "stop", LINUX_SERVICE_NAME],
    )?;
    println!("Gateway service stopped");
    Ok(())
}

fn status_linux() -> Result<()> {
    let output = run_command(
        "systemctl",
        &["--user", "status", LINUX_SERVICE_NAME, "--no-pager"],
    )?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow!("Gateway service is not running"))
    }
}

fn mac_plist_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{MAC_LABEL}.plist")))
}

fn current_uid() -> Result<String> {
    if let Ok(uid) = std::env::var("UID") {
        if !uid.trim().is_empty() {
            return Ok(uid);
        }
    }
    let output = run_command("id", &["-u"])?;
    if !output.status.success() {
        return Err(anyhow!("Failed to determine user id"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn render_macos_plist(ctx: &ServiceContext) -> String {
    let mut items = vec![
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_string(),
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">".to_string(),
        "<plist version=\"1.0\">".to_string(),
        "<dict>".to_string(),
        "  <key>Label</key>".to_string(),
        format!("  <string>{MAC_LABEL}</string>"),
        "  <key>ProgramArguments</key>".to_string(),
        "  <array>".to_string(),
        format!("    <string>{}</string>", xml_escape(&ctx.exe_path.to_string_lossy())),
        "    <string>start</string>".to_string(),
        "  </array>".to_string(),
        "  <key>WorkingDirectory</key>".to_string(),
        format!(
            "  <string>{}</string>",
            xml_escape(&ctx.working_dir.to_string_lossy())
        ),
        "  <key>RunAtLoad</key>".to_string(),
        "  <true/>".to_string(),
        "  <key>KeepAlive</key>".to_string(),
        "  <true/>".to_string(),
        "  <key>StandardOutPath</key>".to_string(),
        format!(
            "  <string>{}</string>",
            xml_escape(&ctx.working_dir.join(LOG_STDOUT_FILE).to_string_lossy())
        ),
        "  <key>StandardErrorPath</key>".to_string(),
        format!(
            "  <string>{}</string>",
            xml_escape(&ctx.working_dir.join(LOG_STDERR_FILE).to_string_lossy())
        ),
    ];

    items.push("  <key>EnvironmentVariables</key>".to_string());
    items.push("  <dict>".to_string());
    items.push("    <key>RAYCLAW_GATEWAY</key>".to_string());
    items.push("    <string>1</string>".to_string());
    if let Some(config_path) = &ctx.config_path {
        items.push("    <key>RAYCLAW_CONFIG</key>".to_string());
        items.push(format!(
            "    <string>{}</string>",
            xml_escape(&config_path.to_string_lossy())
        ));
    }
    items.push("  </dict>".to_string());

    items.push("</dict>".to_string());
    items.push("</plist>".to_string());
    items.join("\n")
}

fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&apos;")
}

fn mac_target_label() -> Result<String> {
    let uid = current_uid()?;
    Ok(format!("gui/{uid}/{MAC_LABEL}"))
}

fn install_macos(ctx: &ServiceContext) -> Result<()> {
    let plist_path = mac_plist_path()?;
    let launch_agents = plist_path
        .parent()
        .ok_or_else(|| anyhow!("Invalid plist path"))?;
    std::fs::create_dir_all(launch_agents)
        .with_context(|| format!("Failed to create {}", launch_agents.display()))?;
    std::fs::write(&plist_path, render_macos_plist(ctx))
        .with_context(|| format!("Failed to write {}", plist_path.display()))?;

    let _ = stop_macos();
    start_macos()?;
    println!(
        "Installed and started gateway service: {}",
        plist_path.display()
    );
    Ok(())
}

fn uninstall_macos() -> Result<()> {
    let _ = stop_macos();
    let plist_path = mac_plist_path()?;
    if plist_path.exists() {
        std::fs::remove_file(&plist_path)
            .with_context(|| format!("Failed to remove {}", plist_path.display()))?;
    }
    println!("Uninstalled gateway service");
    Ok(())
}

fn start_macos() -> Result<()> {
    let target = mac_target_label()?;
    let plist_path = mac_plist_path()?;
    if !plist_path.exists() {
        return Err(anyhow!(
            "Service not installed. Run: rayclaw gateway install"
        ));
    }
    let gui_target = format!("gui/{}", current_uid()?);
    let plist_path_str = plist_path.to_string_lossy().to_string();
    let bootstrap = run_command("launchctl", &["bootstrap", &gui_target, &plist_path_str])?;
    if !bootstrap.status.success() {
        let stderr = String::from_utf8_lossy(&bootstrap.stderr);
        if !(stderr.contains("already loaded") || stderr.contains("already exists")) {
            return Err(anyhow!(
                "Command failed: launchctl bootstrap {} {}\nstderr: {}",
                gui_target,
                plist_path_str,
                stderr.trim()
            ));
        }
    }

    ensure_success(
        run_command("launchctl", &["kickstart", "-k", &target])?,
        "launchctl",
        &["kickstart", "-k", &target],
    )?;
    println!("Gateway service started");
    Ok(())
}

fn stop_macos() -> Result<()> {
    let target = mac_target_label()?;
    let output = run_command("launchctl", &["bootout", &target])?;
    if output.status.success() {
        println!("Gateway service stopped");
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("No such process")
        || stderr.contains("Could not find specified service")
        || stderr.contains("not found")
    {
        return Ok(());
    }

    Err(anyhow!(
        "Failed to stop service: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn status_macos() -> Result<()> {
    let target = mac_target_label()?;
    let output = run_command("launchctl", &["print", &target])?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow!("Gateway service is not running"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xml_escape() {
        let input = "a&b<c>d\"e'f";
        let escaped = xml_escape(input);
        assert_eq!(escaped, "a&amp;b&lt;c&gt;d&quot;e&apos;f");
    }

    #[test]
    fn test_render_linux_unit_contains_start_and_restart() {
        let ctx = ServiceContext {
            exe_path: PathBuf::from("/usr/local/bin/rayclaw"),
            working_dir: PathBuf::from("/tmp/rayclaw"),
            config_path: Some(PathBuf::from("/tmp/rayclaw/rayclaw.config.yaml")),
            runtime_logs_dir: PathBuf::from("/tmp/rayclaw/runtime/logs"),
        };

        let unit = render_linux_unit(&ctx);
        assert!(unit.contains("ExecStart=/usr/local/bin/rayclaw start"));
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("RAYCLAW_GATEWAY=1"));
        assert!(unit.contains("RAYCLAW_CONFIG=/tmp/rayclaw/rayclaw.config.yaml"));
    }

    #[test]
    fn test_render_macos_plist_contains_required_fields() {
        let ctx = ServiceContext {
            exe_path: PathBuf::from("/usr/local/bin/rayclaw"),
            working_dir: PathBuf::from("/tmp/rayclaw"),
            config_path: Some(PathBuf::from("/tmp/rayclaw/rayclaw.config.yaml")),
            runtime_logs_dir: PathBuf::from("/tmp/rayclaw/runtime/logs"),
        };

        let plist = render_macos_plist(&ctx);
        assert!(plist.contains("<key>Label</key>"));
        assert!(plist.contains(MAC_LABEL));
        assert!(plist.contains("<string>start</string>"));
        assert!(plist.contains("RAYCLAW_GATEWAY"));
        assert!(plist.contains("RAYCLAW_CONFIG"));
    }

    #[test]
    fn test_parse_log_lines_default_and_custom() {
        assert_eq!(parse_log_lines(None).unwrap(), DEFAULT_LOG_LINES);
        assert_eq!(parse_log_lines(Some("20")).unwrap(), 20);
        assert!(parse_log_lines(Some("0")).is_err());
        assert!(parse_log_lines(Some("abc")).is_err());
    }

    #[test]
    fn test_resolve_runtime_logs_dir_fallback() {
        let dir = resolve_runtime_logs_dir(Path::new("/tmp/rayclaw"));
        assert!(
            dir.ends_with("runtime/logs") || dir.ends_with("rayclaw.data/runtime/logs"),
            "unexpected logs dir: {}",
            dir.display()
        );
    }
}
