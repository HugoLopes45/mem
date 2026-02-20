#!/usr/bin/env bash
# mem install script — installs the binary and wires Claude Code hooks.
# Usage: curl -fsSL https://raw.githubusercontent.com/HugoLopes45/mem/main/install.sh | bash
set -euo pipefail

REPO="HugoLopes45/mem"
BIN_NAME="mem"
INSTALL_DIR="${MEM_INSTALL_DIR:-$HOME/.local/bin}"
HOOKS_DIR="${MEM_HOOKS_DIR:-$HOME/.claude/hooks}"

# ── Colors ────────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BOLD='\033[1m'; RESET='\033[0m'
info()    { echo -e "${BOLD}[mem]${RESET} $*"; }
success() { echo -e "${GREEN}[mem]${RESET} $*"; }
warn()    { echo -e "${YELLOW}[mem] warn:${RESET} $*"; }
error()   { echo -e "${RED}[mem] error:${RESET} $*" >&2; exit 1; }

# ── Detect OS / arch ──────────────────────────────────────────────────────────
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
case "$ARCH" in
  x86_64)  ARCH="x86_64" ;;
  aarch64|arm64) ARCH="aarch64" ;;
  *) error "Unsupported architecture: $ARCH. Build from source with: cargo install --git https://github.com/$REPO" ;;
esac

case "$OS" in
  linux)  TARGET="${ARCH}-unknown-linux-musl" ;;
  darwin) TARGET="${ARCH}-apple-darwin" ;;
  *) error "Unsupported OS: $OS. Build from source with: cargo install --git https://github.com/$REPO" ;;
esac

# ── Find latest release ───────────────────────────────────────────────────────
info "Fetching latest release..."
if command -v curl >/dev/null 2>&1; then
  FETCH="curl -fsSL"
elif command -v wget >/dev/null 2>&1; then
  FETCH="wget -qO-"
else
  error "curl or wget required"
fi

LATEST=$($FETCH "https://api.github.com/repos/$REPO/releases/latest" \
  | grep '"tag_name"' | sed 's/.*"tag_name": *"\(.*\)".*/\1/')

if [ -z "$LATEST" ]; then
  # No release yet — fall back to cargo install from source
  warn "No release found. Installing from source via cargo..."
  if ! command -v cargo >/dev/null 2>&1; then
    error "cargo not found. Install Rust: https://rustup.rs"
  fi
  cargo install --git "https://github.com/$REPO" --locked
  INSTALLED_VIA="cargo"
else
  ARCHIVE="${BIN_NAME}-${LATEST}-${TARGET}.tar.gz"
  URL="https://github.com/$REPO/releases/download/${LATEST}/${ARCHIVE}"

  info "Downloading $BIN_NAME $LATEST for $TARGET..."
  TMP=$(mktemp -d)
  trap 'rm -rf "$TMP"' EXIT

  install_from_source() {
    warn "Pre-built binary not found for $TARGET. Falling back to cargo install..."
    if ! command -v cargo >/dev/null 2>&1; then
      error "cargo not found. Install Rust: https://rustup.rs"
    fi
    cargo install --git "https://github.com/$REPO" --locked
    INSTALLED_VIA="cargo"
  }

  if command -v curl >/dev/null 2>&1; then
    if ! curl -fsSL -o "$TMP/$ARCHIVE" "$URL" 2>/dev/null; then
      install_from_source
    fi
  else
    if ! wget -q -O "$TMP/$ARCHIVE" "$URL" 2>/dev/null; then
      install_from_source
    fi
  fi

  if [ -z "${INSTALLED_VIA:-}" ]; then
    if ! tar -xzf "$TMP/$ARCHIVE" -C "$TMP" 2>/dev/null; then
      error "Failed to extract archive. Download may be corrupted. Try: cargo install --git https://github.com/$REPO"
    fi
    mkdir -p "$INSTALL_DIR"
    mv "$TMP/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"
    chmod +x "$INSTALL_DIR/$BIN_NAME"
    INSTALLED_VIA="binary"
  fi
fi

# ── Verify ────────────────────────────────────────────────────────────────────
if [ "$INSTALLED_VIA" = "binary" ] && ! command -v "$BIN_NAME" >/dev/null 2>&1; then
  warn "$INSTALL_DIR is not in your PATH."
  warn "Add to your shell profile: export PATH=\"\$PATH:$INSTALL_DIR\""
fi

success "Installed $BIN_NAME ($INSTALLED_VIA)"

# ── Install hook scripts ──────────────────────────────────────────────────────
info "Installing hook scripts to $HOOKS_DIR..."
mkdir -p "$HOOKS_DIR"

HOOKS_SRC="$(dirname "${BASH_SOURCE[0]}")/hooks"
if [ ! -d "$HOOKS_SRC" ]; then
  # Downloaded via curl — fetch hooks from GitHub
  for HOOK in mem-stop.sh mem-precompact.sh mem-session-start.sh; do
    HOOK_URL="https://raw.githubusercontent.com/$REPO/main/hooks/$HOOK"
    if command -v curl >/dev/null 2>&1; then
      curl -fsSL -o "$HOOKS_DIR/$HOOK" "$HOOK_URL" 2>/dev/null || \
        error "Failed to download hook $HOOK"
    else
      wget -q -O "$HOOKS_DIR/$HOOK" "$HOOK_URL" 2>/dev/null || \
        error "Failed to download hook $HOOK"
    fi
    chmod +x "$HOOKS_DIR/$HOOK"
  done
else
  cp "$HOOKS_SRC"/mem-*.sh "$HOOKS_DIR/"
  chmod +x "$HOOKS_DIR"/mem-*.sh
fi

success "Hook scripts installed to $HOOKS_DIR"

# ── Done ──────────────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}Next step:${RESET} Add hooks to ~/.claude/settings.json:"
echo ""
cat <<'HOOKS'
{
  "hooks": {
    "Stop": [{"hooks": [{"type": "command", "command": "~/.claude/hooks/mem-stop.sh"}]}],
    "PreCompact": [{"matcher": "auto", "hooks": [{"type": "command", "command": "~/.claude/hooks/mem-precompact.sh"}]}],
    "SessionStart": [{"hooks": [{"type": "command", "command": "~/.claude/hooks/mem-session-start.sh"}]}]
  }
}
HOOKS
echo ""
echo -e "Run ${BOLD}mem stats${RESET} to verify the installation."
