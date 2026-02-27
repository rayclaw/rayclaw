#!/bin/bash
set -euo pipefail

# -------------------------------------------------------------------
# release_homebrew.sh
#
# 1. Bump patch version in Cargo.toml
# 2. Build web/dist
# 3. cargo build --release
# 4. Create a tar.gz of the binary
# 5. Create a GitHub release and upload the tarball
# 6. Update the homebrew-tap Formula with new version + sha256
# 7. Push tap changes
# -------------------------------------------------------------------

ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
REPO_DIR="$ROOT_DIR"
TAP_DIR_DEFAULT="$ROOT_DIR/../../github/homebrew-tap"
TAP_DIR="${TAP_DIR:-$TAP_DIR_DEFAULT}"
TAP_REPO="everettjf/homebrew-tap"
FORMULA_PATH="Formula/rayclaw.rb"
GITHUB_REPO="rayclaw/rayclaw"

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64) echo "x86_64" ;;
    arm64|aarch64) echo "aarch64" ;;
    *)
      echo "Unsupported architecture: $(uname -m)" >&2
      exit 1
      ;;
  esac
}

latest_release_tag() {
  git tag --list 'v*' --sort=-version:refname | head -n1
}

contains_digit_four() {
  [[ "$1" == *4* ]]
}

build_release_notes() {
  local prev_tag="$1"
  local new_tag="$2"
  local out_file="$3"
  local compare_url="https://github.com/$GITHUB_REPO/compare"
  local changes

  if [ -n "$prev_tag" ]; then
    changes="$(git log --no-merges --pretty=format:'%s' "$prev_tag..HEAD" \
      | grep -vE '^bump version to ' \
      | head -n 30 || true)"
  else
    changes="$(git log --no-merges --pretty=format:'%s' \
      | grep -vE '^bump version to ' \
      | head -n 30 || true)"
  fi

  {
    echo "RayClaw $new_tag"
    echo
    echo "## Change log"
    if [ -n "$changes" ]; then
      while IFS= read -r line; do
        [ -n "$line" ] && echo "- $line"
      done <<< "$changes"
    else
      echo "- Internal maintenance and release packaging updates"
    fi
    echo
    echo "## Compare"
    if [ -n "$prev_tag" ]; then
      echo "$compare_url/$prev_tag...$new_tag"
    else
      echo "N/A (first tagged release)"
    fi
  } > "$out_file"
}

require_cmd cargo
require_cmd git
require_cmd gh
require_cmd shasum
require_cmd tar
require_cmd npm

if ! gh auth status >/dev/null 2>&1; then
  echo "GitHub CLI not authenticated. Run: gh auth login" >&2
  exit 1
fi

cd "$REPO_DIR"

# --- Build web assets (embedded via include_dir! in src/web.rs) ---
if [ -f "web/package.json" ]; then
  echo "Building web assets..."
  if [ -f "web/package-lock.json" ]; then
    npm --prefix web ci
  else
    npm --prefix web install
  fi
  npm --prefix web run build
  test -f "web/dist/index.html" || {
    echo "web/dist/index.html is missing after web build" >&2
    exit 1
  }
  test -f "web/dist/icon.png" || {
    echo "web/dist/icon.png is missing after web build" >&2
    exit 1
  }
  if ! ls web/dist/assets/*.js >/dev/null 2>&1; then
    echo "web/dist/assets/*.js is missing after web build" >&2
    exit 1
  fi
fi

# --- Bump patch version in Cargo.toml ---
PREV_TAG="$(latest_release_tag)"
CURRENT_VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"

NEW_MAJOR="$MAJOR"
NEW_MINOR="$MINOR"

while contains_digit_four "$NEW_MAJOR"; do
  NEW_MAJOR=$((NEW_MAJOR + 1))
  NEW_MINOR=0
  PATCH=0
done

while contains_digit_four "$NEW_MINOR"; do
  NEW_MINOR=$((NEW_MINOR + 1))
  PATCH=0
done

NEW_PATCH=$((PATCH + 1))
NEW_VERSION="$NEW_MAJOR.$NEW_MINOR.$NEW_PATCH"
while contains_digit_four "$NEW_VERSION"; do
  NEW_PATCH=$((NEW_PATCH + 1))
  NEW_VERSION="$NEW_MAJOR.$NEW_MINOR.$NEW_PATCH"
done
TAG="v$NEW_VERSION"

if [ "$PREV_TAG" = "$TAG" ]; then
  PREV_TAG="$(git tag --list 'v*' --sort=-version:refname | sed -n '2p')"
fi

sed -i '' "s/^version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" Cargo.toml
echo "Version bumped: $CURRENT_VERSION -> $NEW_VERSION"
if [ -n "$PREV_TAG" ]; then
  echo "Previous tag: $PREV_TAG"
else
  echo "Previous tag: (none)"
fi

# --- Build release binary ---
echo "Building release binary..."
cargo build --release

BINARY="target/release/rayclaw"
if [ ! -f "$BINARY" ]; then
  echo "Binary not found: $BINARY" >&2
  exit 1
fi

# --- Create tarball ---
ARCH="$(detect_arch)"
TARBALL_NAME="rayclaw-$NEW_VERSION-${ARCH}-apple-darwin.tar.gz"
TARBALL_PATH="target/release/$TARBALL_NAME"

tar -czf "$TARBALL_PATH" -C target/release rayclaw
echo "Created tarball: $TARBALL_PATH"

SHA256=$(shasum -a 256 "$TARBALL_PATH" | awk '{print $1}')
echo "SHA256: $SHA256"

# --- Git commit + push ---
git add .
git commit -m "bump version to $NEW_VERSION"
git push

echo "Release commit pushed: $(git rev-parse HEAD)"

# --- Finalize release (blocking) ---
"$ROOT_DIR/scripts/release_finalize.sh" \
  --repo-dir "$REPO_DIR" \
  --tap-dir "$TAP_DIR" \
  --tap-repo "$TAP_REPO" \
  --formula-path "$FORMULA_PATH" \
  --github-repo "$GITHUB_REPO" \
  --new-version "$NEW_VERSION" \
  --tag "$TAG" \
  --tarball-path "$TARBALL_PATH" \
  --tarball-name "$TARBALL_NAME" \
  --sha256 "$SHA256"
