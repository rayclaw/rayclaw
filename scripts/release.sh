#!/bin/bash
set -euo pipefail

# -------------------------------------------------------------------
# release.sh
#
# 1. Bump patch version in Cargo.toml
# 2. Build web/dist
# 3. cargo build --release
# 4. Create a tar.gz of the binary
# 5. Git commit + push
# 6. Wait for CI
# 7. Create git tag + GitHub Release with asset
# -------------------------------------------------------------------

ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
GITHUB_REPO="${RAYCLAW_GITHUB_REPO:-stevensu1977/rayclaw}"

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

detect_os() {
  case "$(uname -s)" in
    Darwin) echo "apple-darwin" ;;
    Linux)  echo "unknown-linux-gnu" ;;
    *)
      echo "Unsupported OS: $(uname -s)" >&2
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

wait_for_ci_success() {
  local commit_sha="$1"
  local timeout_seconds="${CI_WAIT_TIMEOUT_SECONDS:-1800}"
  local interval_seconds="${CI_WAIT_INTERVAL_SECONDS:-20}"
  local elapsed=0

  echo "Waiting for CI success on commit: $commit_sha"
  while [ "$elapsed" -lt "$timeout_seconds" ]; do
    local success_run_id
    success_run_id="$(
      gh run list \
        --repo "$GITHUB_REPO" \
        --workflow "CI" \
        --commit "$commit_sha" \
        --json databaseId,conclusion \
        --jq '[.[] | select(.conclusion == "success")] | first | .databaseId'
    )"

    if [ -n "$success_run_id" ] && [ "$success_run_id" != "null" ]; then
      echo "CI succeeded. Run id: $success_run_id"
      return 0
    fi

    local failed_run_url
    failed_run_url="$(
      gh run list \
        --repo "$GITHUB_REPO" \
        --workflow "CI" \
        --commit "$commit_sha" \
        --json conclusion,url \
        --jq '[.[] | select(.conclusion == "failure" or .conclusion == "cancelled" or .conclusion == "timed_out" or .conclusion == "action_required" or .conclusion == "startup_failure" or .conclusion == "stale")] | first | .url'
    )"

    if [ -n "$failed_run_url" ] && [ "$failed_run_url" != "null" ]; then
      echo "CI failed for commit $commit_sha: $failed_run_url" >&2
      return 1
    fi

    echo "CI not successful yet. Slept ${elapsed}s/${timeout_seconds}s."
    sleep "$interval_seconds"
    elapsed=$((elapsed + interval_seconds))
  done

  echo "Timed out waiting for CI success after ${timeout_seconds}s." >&2
  return 1
}

build_release_notes() {
  local prev_tag="$1"
  local new_tag="$2"
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
  echo "## Install"
  echo '```sh'
  echo "curl -fsSL https://rayclaw.ai/install.sh | bash"
  echo '```'
  echo
  echo "## Compare"
  if [ -n "$prev_tag" ]; then
    echo "$compare_url/$prev_tag...$new_tag"
  else
    echo "N/A (first tagged release)"
  fi
}

# --- Preflight ---
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

cd "$ROOT_DIR"

# --- 1. Build web assets (embedded via include_dir! in src/web.rs) ---
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
fi

# --- 2. Bump patch version in Cargo.toml ---
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

# --- 3. Build release binary ---
echo "Building release binary..."
cargo build --release

BINARY="target/release/rayclaw"
if [ ! -f "$BINARY" ]; then
  echo "Binary not found: $BINARY" >&2
  exit 1
fi

# --- 4. Create tarball ---
ARCH="$(detect_arch)"
OS="$(detect_os)"
TARBALL_NAME="rayclaw-${NEW_VERSION}-${ARCH}-${OS}.tar.gz"
TARBALL_PATH="target/release/$TARBALL_NAME"

tar -czf "$TARBALL_PATH" -C target/release rayclaw
echo "Created tarball: $TARBALL_PATH"

SHA256=$(shasum -a 256 "$TARBALL_PATH" | awk '{print $1}')
echo "SHA256: $SHA256"

# --- 5. Git commit + push ---
git add .
git commit -m "bump version to $NEW_VERSION"
git push

RELEASE_COMMIT_SHA="$(git rev-parse HEAD)"
echo "Release commit pushed: $RELEASE_COMMIT_SHA"

# --- 6. Wait for CI ---
if ! wait_for_ci_success "$RELEASE_COMMIT_SHA"; then
  exit 1
fi

# --- 7. Create git tag + GitHub Release ---
if git ls-remote --exit-code --tags origin "refs/tags/$TAG" >/dev/null 2>&1; then
  echo "Tag already exists on origin: $TAG"
else
  if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
    echo "Tag already exists locally: $TAG"
  else
    git tag "$TAG" "$RELEASE_COMMIT_SHA"
    echo "Created local tag: $TAG -> $RELEASE_COMMIT_SHA"
  fi
  git push origin "refs/tags/$TAG"
  echo "Pushed tag: $TAG"
fi

RELEASE_NOTES="$(build_release_notes "$PREV_TAG" "$TAG")"

if gh release view "$TAG" --repo "$GITHUB_REPO" >/dev/null 2>&1; then
  echo "Release $TAG exists. Uploading/overwriting asset."
  gh release upload "$TAG" "$TARBALL_PATH" --repo "$GITHUB_REPO" --clobber
else
  echo "Creating release $TAG and uploading asset."
  gh release create "$TAG" "$TARBALL_PATH" \
    --repo "$GITHUB_REPO" \
    -t "RayClaw $TAG" \
    -n "$RELEASE_NOTES"
fi

echo ""
echo "Released $TAG"
echo "  GitHub: https://github.com/$GITHUB_REPO/releases/tag/$TAG"
echo "  SHA256: $SHA256"
echo ""
echo "Users install with:"
echo "  curl -fsSL https://rayclaw.ai/install.sh | bash"
echo ""
