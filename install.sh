#!/usr/bin/env bash
# mem install script — one command, fully wired.
# Usage: curl -fsSL https://raw.githubusercontent.com/HugoLopes45/mem/main/install.sh | bash
set -euo pipefail

REPO="HugoLopes45/mem"
BIN_NAME="mem"
INSTALL_DIR="${MEM_INSTALL_DIR:-$HOME/.cargo/bin}"

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
  *) error "Unsupported architecture: $ARCH. Build from source: cargo install --git https://github.com/$REPO" ;;
esac

case "$OS" in
  linux)  TARGET="${ARCH}-unknown-linux-musl" ;;
  darwin) TARGET="${ARCH}-apple-darwin" ;;
  *) error "Unsupported OS: $OS. Build from source: cargo install --git https://github.com/$REPO" ;;
esac

# ── Fetch latest release ──────────────────────────────────────────────────────
if ! command -v curl >/dev/null 2>&1 && ! command -v wget >/dev/null 2>&1; then
  error "curl or wget required"
fi

info "Fetching latest release..."
LATEST=""
if command -v curl >/dev/null 2>&1; then
  LATEST=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep '"tag_name"' | sed 's/.*"tag_name": *"\(.*\)".*/\1/' || true)
else
  LATEST=$(wget -qO- "https://api.github.com/repos/$REPO/releases/latest" \
    | grep '"tag_name"' | sed 's/.*"tag_name": *"\(.*\)".*/\1/' || true)
fi

INSTALLED_VIA=""

install_from_source() {
  warn "${1:-Falling back to cargo install from source...}"
  if ! command -v cargo >/dev/null 2>&1; then
    error "cargo not found. Install Rust: https://rustup.rs"
  fi
  cargo install --git "https://github.com/$REPO" --locked
  INSTALLED_VIA="cargo"
}

if [ -z "$LATEST" ]; then
  install_from_source "No pre-built release found (network issue or no release yet)."
else
  ARCHIVE="${BIN_NAME}-${LATEST}-${TARGET}.tar.gz"
  URL="https://github.com/$REPO/releases/download/${LATEST}/${ARCHIVE}"

  info "Downloading $BIN_NAME $LATEST for $TARGET..."
  TMP=$(mktemp -d)
  trap 'rm -rf "$TMP"' EXIT

  downloaded=0
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$TMP/$ARCHIVE" "$URL" && downloaded=1 || warn "curl download failed: $URL"
  else
    wget -q -O "$TMP/$ARCHIVE" "$URL" && downloaded=1 || warn "wget download failed: $URL"
  fi

  if [ "$downloaded" = "1" ] && tar -xzf "$TMP/$ARCHIVE" -C "$TMP"; then
    mkdir -p "$INSTALL_DIR"
    mv "$TMP/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"
    chmod +x "$INSTALL_DIR/$BIN_NAME"
    INSTALLED_VIA="binary"
  else
    install_from_source "Pre-built binary unavailable for $TARGET — falling back to source."
  fi
fi

# ── PATH check ────────────────────────────────────────────────────────────────
if ! command -v "$BIN_NAME" >/dev/null 2>&1; then
  warn "$INSTALL_DIR is not in your PATH."
  warn "Add to your shell profile: export PATH=\"\$PATH:$INSTALL_DIR\""
  warn "Then re-run: mem init"
  success "Installed $BIN_NAME ($INSTALLED_VIA) — add to PATH then run: mem init"
  exit 0
fi

success "Installed $BIN_NAME ($INSTALLED_VIA)"

# ── Wire hooks — the whole point ──────────────────────────────────────────────
info "Wiring Claude Code hooks..."
"$BIN_NAME" init

success "Done. mem is fully wired."
echo ""
echo -e "  ${BOLD}mem status${RESET}   — verify installation"
echo -e "  ${BOLD}mem search${RESET}   — search your session memories"
echo -e "  ${BOLD}mem gain${RESET}     — token analytics"
