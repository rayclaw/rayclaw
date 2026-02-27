#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")" && pwd)"

usage() {
  cat <<'EOF'
Usage:
  ./deploy.sh

Workflow:
  1. Run cargo clippy
  2. Bump version, build web + binary, create tarball
  3. Push commit, wait for CI, create GitHub Release with asset
  4. install.sh fetches the release asset for Linux/macOS
EOF
}

case "${1:-}" in
  -h|--help|help)
    usage
    exit 0
    ;;
esac

cd "$ROOT_DIR"

if [ ! -x "$ROOT_DIR/scripts/release.sh" ]; then
  echo "Missing executable: scripts/release.sh" >&2
  exit 1
fi

echo "Running pre-deploy checks..."
cargo clippy --all-targets -- -D warnings

echo "Starting release..."
"$ROOT_DIR/scripts/release.sh"

echo "Deploy complete."
