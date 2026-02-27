#!/usr/bin/env bash
set -euo pipefail

# -------------------------------------------------------------------
# RayClaw installer
#
# Usage:
#   curl -fsSL https://rayclaw.ai/install.sh | bash
#
# Environment variables:
#   RAYCLAW_REPO         Override GitHub repo (default: stevensu1977/rayclaw)
#   RAYCLAW_INSTALL_DIR  Override install directory
#   RAYCLAW_VERSION      Install a specific version tag (e.g. v0.1.0)
# -------------------------------------------------------------------

REPO="${RAYCLAW_REPO:-stevensu1977/rayclaw}"
BIN_NAME="rayclaw"

log() { printf '%s\n' "$*"; }
err() { printf 'Error: %s\n' "$*" >&2; }
need_cmd() { command -v "$1" >/dev/null 2>&1; }

detect_os() {
  case "$(uname -s)" in
    Darwin) echo "darwin" ;;
    Linux)  echo "linux"  ;;
    *)
      err "Unsupported OS: $(uname -s)"
      exit 1
      ;;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64)  echo "x86_64"  ;;
    arm64|aarch64) echo "aarch64" ;;
    *)
      err "Unsupported architecture: $(uname -m)"
      exit 1
      ;;
  esac
}

detect_install_dir() {
  if [ -n "${RAYCLAW_INSTALL_DIR:-}" ]; then
    echo "$RAYCLAW_INSTALL_DIR"
    return
  fi
  if [ -w "/usr/local/bin" ]; then
    echo "/usr/local/bin"
    return
  fi
  if [ -d "$HOME/.local/bin" ] || mkdir -p "$HOME/.local/bin" 2>/dev/null; then
    echo "$HOME/.local/bin"
    return
  fi
  echo "/usr/local/bin"
}

fetch() {
  if need_cmd curl; then
    curl -fsSL "$1"
  elif need_cmd wget; then
    wget -qO- "$1"
  else
    err "Neither curl nor wget is available"
    exit 1
  fi
}

download_file() {
  local url="$1" output="$2"
  if need_cmd curl; then
    curl -fL --progress-bar "$url" -o "$output"
  else
    wget --show-progress -O "$output" "$url"
  fi
}

get_release_url() {
  if [ -n "${RAYCLAW_VERSION:-}" ]; then
    local tag="${RAYCLAW_VERSION}"
    [[ "$tag" == v* ]] || tag="v$tag"
    echo "https://api.github.com/repos/${REPO}/releases/tags/${tag}"
  else
    echo "https://api.github.com/repos/${REPO}/releases/latest"
  fi
}

extract_asset_url() {
  local release_json="$1"
  local os="$2"
  local arch="$3"
  local os_regex arch_regex

  case "$os" in
    darwin) os_regex="apple-darwin|darwin" ;;
    linux)  os_regex="unknown-linux-gnu|unknown-linux-musl|linux" ;;
    *)
      err "Unsupported OS for release matching: $os"
      return 1
      ;;
  esac

  case "$arch" in
    x86_64)  arch_regex="x86_64|amd64" ;;
    aarch64) arch_regex="aarch64|arm64" ;;
    *)
      err "Unsupported architecture for release matching: $arch"
      return 1
      ;;
  esac

  printf '%s\n' "$release_json" \
    | grep -Eo 'https://[^"]+' \
    | grep '/releases/download/' \
    | grep -E "/${BIN_NAME}-v?[0-9]+\.[0-9]+\.[0-9]+-.*(apple-darwin|unknown-linux-gnu|unknown-linux-musl|pc-windows-msvc)\.(tar\.gz|zip)$" \
    | grep -Ei "(${arch_regex}).*(${os_regex})|(${os_regex}).*(${arch_regex})" \
    | head -n1
}

install_from_archive() {
  local archive="$1"
  local install_dir="$2"
  local tmpdir="$3"
  local extracted=0

  case "$archive" in
    *.tar.gz|*.tgz)
      tar -xzf "$archive" -C "$tmpdir"
      extracted=1
      ;;
    *.zip)
      if ! need_cmd unzip; then
        err "unzip is required to extract zip archives"
        return 1
      fi
      unzip -q "$archive" -d "$tmpdir"
      extracted=1
      ;;
  esac

  if [ "$extracted" -eq 0 ]; then
    if tar -tzf "$archive" >/dev/null 2>&1; then
      tar -xzf "$archive" -C "$tmpdir"
      extracted=1
    elif need_cmd unzip && unzip -tq "$archive" >/dev/null 2>&1; then
      unzip -q "$archive" -d "$tmpdir"
      extracted=1
    fi
  fi

  if [ "$extracted" -eq 0 ]; then
    err "Unknown archive format: $archive"
    return 1
  fi

  local bin_path
  bin_path="$(find "$tmpdir" -type f -name "$BIN_NAME" | head -n1)"
  if [ -z "$bin_path" ]; then
    err "Could not find '$BIN_NAME' in archive"
    return 1
  fi

  chmod +x "$bin_path"
  if [ -w "$install_dir" ]; then
    cp "$bin_path" "$install_dir/$BIN_NAME"
  else
    if need_cmd sudo; then
      log "Installing to $install_dir (requires sudo)"
      sudo cp "$bin_path" "$install_dir/$BIN_NAME"
    else
      err "No write permission for $install_dir and sudo not available"
      return 1
    fi
  fi
}

main() {
  local os arch install_dir release_json asset_url version_tag

  os="$(detect_os)"
  arch="$(detect_arch)"
  install_dir="$(detect_install_dir)"

  log ""
  log "  RayClaw Installer"
  log "  ==================="
  log "  Repo:    ${REPO}"
  log "  OS:      ${os}"
  log "  Arch:    ${arch}"
  log "  Install: ${install_dir}"

  local api_url
  api_url="$(get_release_url)"
  release_json="$(fetch "$api_url")"

  # Extract version from release JSON
  version_tag="$(printf '%s\n' "$release_json" | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | grep -o '"v[^"]*"' | tr -d '"' || true)"
  if [ -n "$version_tag" ]; then
    log "  Version: ${version_tag}"
  fi
  log ""

  asset_url="$(extract_asset_url "$release_json" "$os" "$arch" || true)"
  if [ -z "$asset_url" ]; then
    err "No prebuilt binary found for ${os}/${arch} in release ${version_tag:-latest}."
    err ""
    err "Alternative install methods:"
    err "  Build from source: git clone https://github.com/${REPO} && cd rayclaw && cargo build --release"
    exit 1
  fi

  local tmpdir archive asset_filename
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT

  asset_filename="${asset_url##*/}"
  asset_filename="${asset_filename%%\?*}"
  if [ -z "$asset_filename" ] || [ "$asset_filename" = "$asset_url" ]; then
    asset_filename="${BIN_NAME}.archive"
  fi
  archive="$tmpdir/$asset_filename"

  download_file "$asset_url" "$archive"
  install_from_archive "$archive" "$install_dir" "$tmpdir"

  log ""
  log "  Installed ${BIN_NAME} to ${install_dir}/${BIN_NAME}"

  # PATH guidance for ~/.local/bin
  if [ "$install_dir" = "$HOME/.local/bin" ]; then
    if ! echo "$PATH" | tr ':' '\n' | grep -q "^${install_dir}$"; then
      log ""
      log "  Add to your PATH:"
      log "    export PATH=\"\$HOME/.local/bin:\$PATH\""
    fi
  fi

  log ""
  log "  Get started:"
  log "    ${BIN_NAME} setup    # Interactive configuration wizard"
  log "    ${BIN_NAME} start    # Start the bot"
  log "    ${BIN_NAME} help     # Show all commands"
  log ""
}

main "$@"
