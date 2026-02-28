use std::path::Path;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO: &str = "rayclaw/rayclaw";

pub async fn run_update(args: &[String]) -> anyhow::Result<()> {
    let check_only = args.first().map(|s| s.as_str()) == Some("check");

    println!("Current version: v{VERSION}");
    println!("Checking for updates...");

    let (latest_tag, assets) = fetch_latest_release().await?;
    let latest_version = latest_tag.strip_prefix('v').unwrap_or(&latest_tag);

    if latest_version == VERSION {
        println!("Already up to date (v{VERSION})");
        return Ok(());
    }

    println!("New version available: v{latest_version}");

    if check_only {
        return Ok(());
    }

    let (os_target, arch_target) = detect_platform()?;
    let asset_name = format!("rayclaw-v{latest_version}-{arch_target}-{os_target}.tar.gz");

    let download_url = assets
        .iter()
        .find_map(|a| {
            let name = a.get("name")?.as_str()?;
            if name == asset_name {
                a.get("browser_download_url")?
                    .as_str()
                    .map(|s| s.to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| {
            let available: Vec<String> = assets
                .iter()
                .filter_map(|a| a.get("name")?.as_str().map(|s| s.to_string()))
                .collect();
            anyhow::anyhow!(
                "No matching asset found for {asset_name}\nAvailable assets:\n  {}",
                available.join("\n  ")
            )
        })?;

    println!("Downloading {asset_name}...");

    let tmp_dir = std::env::temp_dir().join(format!("rayclaw-update-{latest_version}"));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir)?;

    let tarball_path = tmp_dir.join(&asset_name);
    download_file(&download_url, &tarball_path).await?;

    println!("Extracting...");
    let status = std::process::Command::new("tar")
        .args([
            "xzf",
            &tarball_path.to_string_lossy(),
            "-C",
            &tmp_dir.to_string_lossy(),
        ])
        .status()?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        anyhow::bail!("Failed to extract tarball");
    }

    let new_binary = tmp_dir.join("rayclaw");
    if !new_binary.exists() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        anyhow::bail!("Extracted archive does not contain 'rayclaw' binary");
    }

    let current_exe = std::env::current_exe()?;
    replace_binary(&current_exe, &new_binary)?;

    let _ = std::fs::remove_dir_all(&tmp_dir);
    println!("Updated rayclaw: v{VERSION} → v{latest_version}");

    Ok(())
}

async fn fetch_latest_release() -> anyhow::Result<(String, Vec<serde_json::Value>)> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("rayclaw/{VERSION}"))
        .build()?;

    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp: serde_json::Value = client.get(&url).send().await?.json().await?;

    let tag = resp
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Failed to parse release tag from GitHub API"))?
        .to_string();

    let assets = resp
        .get("assets")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    Ok((tag, assets))
}

fn detect_platform() -> anyhow::Result<(&'static str, &'static str)> {
    let os = match std::env::consts::OS {
        "linux" => "unknown-linux-gnu",
        "macos" => "apple-darwin",
        other => anyhow::bail!("Unsupported OS: {other}"),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => anyhow::bail!("Unsupported architecture: {other}"),
    };
    Ok((os, arch))
}

async fn download_file(url: &str, dest: &Path) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .user_agent(format!("rayclaw/{VERSION}"))
        .build()?;

    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("Download failed: HTTP {}", resp.status());
    }

    let bytes = resp.bytes().await?;
    std::fs::write(dest, &bytes)?;
    Ok(())
}

fn replace_binary(current: &Path, new_binary: &Path) -> anyhow::Result<()> {
    let backup = current.with_extension("bak");

    // Rename current → .bak
    std::fs::rename(current, &backup).map_err(|e| {
        anyhow::anyhow!(
            "Cannot rename current binary (permission denied?): {e}\n\
             Try: sudo rayclaw update"
        )
    })?;

    // Copy new binary into place
    if let Err(e) = std::fs::copy(new_binary, current) {
        // Restore backup
        let _ = std::fs::rename(&backup, current);
        anyhow::bail!("Failed to install new binary: {e}");
    }

    // Set executable permission on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(current, std::fs::Permissions::from_mode(0o755));
    }

    // Remove backup
    let _ = std::fs::remove_file(&backup);

    Ok(())
}
