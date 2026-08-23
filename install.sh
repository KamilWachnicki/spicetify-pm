#!/usr/bin/env bash
# spice-pm installer for Linux and macOS.
#
# Usage:
#   ./install.sh [--version vX.Y.Z] [--dir PATH]
#
# Downloads the latest (or given) release from GitHub, verifies its
# checksum when a .sha256 sidecar is published, and installs the binary
# into --dir (default: ~/.local/bin).

set -euo pipefail

REPO="KamilWachnicki/spicetify-pm"
BIN="spice-pm"
INSTALL_DIR="${SPICEPM_INSTALL_DIR:-$HOME/.local/bin}"
TARGET=""

info() { printf '\033[32m%s\033[0m\n' "$*"; }
warn() { printf '\033[33m%s\033[0m\n' "$*"; }
err()  { printf '\033[31m%s\033[0m\n' "$*" >&2; }

usage() {
  cat <<EOF
Usage: $(basename "$0") [--version vX.Y.Z] [--dir PATH]

  --version vX.Y.Z   Install a specific release tag (default: latest)
  --dir PATH         Installation directory (default: ~/.local/bin,
                     overridable with SPICEPM_INSTALL_DIR)
  -h, --help         Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) TARGET="${2:?--version needs a value}"; shift 2 ;;
    --dir)     INSTALL_DIR="${2:?--dir needs a value}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) err "unknown argument: $1"; usage; exit 1 ;;
  esac
done

for cmd in curl tar; do
  command -v "$cmd" >/dev/null 2>&1 || { err "$cmd is required"; exit 1; }
done

case "$(uname -s)" in
  Linux)  os="linux" ;;
  Darwin) os="macos" ;;
  *) err "unsupported OS: $(uname -s) (this script covers Linux and macOS)"; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64|amd64)  arch="x86_64" ;;
  aarch64|arm64) arch="aarch64" ;;
  *) err "unsupported architecture: $(uname -m)"; exit 1 ;;
esac

if [[ -z "$TARGET" ]]; then
  info "fetching the latest release"
  TARGET=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" |
    sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p') || true
  if [[ -z "$TARGET" ]]; then
    err "no release found on $REPO"
    err "build from source instead:  cargo install --path ."
    exit 1
  fi
fi

asset="$BIN-$TARGET-$arch-$os.tar.gz"
url="https://github.com/$REPO/releases/download/$TARGET/$asset"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

info "downloading $asset"
curl -fL --progress-bar -o "$tmp/$asset" "$url"

# verify when the release publishes a .sha256 sidecar
if curl -fsSL -o "$tmp/$asset.sha256" "$url.sha256" 2>/dev/null; then
  info "verifying checksum"
  expected=$(cut -d' ' -f1 "$tmp/$asset.sha256")
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$asset" | cut -d' ' -f1)
  else
    actual=$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)
  fi
  if [[ "$expected" != "$actual" ]]; then
    err "checksum mismatch: expected $expected, got $actual"
    exit 1
  fi
else
  warn "release has no checksum sidecar; skipping verification"
fi

tar -xzf "$tmp/$asset" -C "$tmp"
[[ -f "$tmp/$BIN" ]] || { err "archive did not contain $BIN"; exit 1; }

mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmp/$BIN" "$INSTALL_DIR/$BIN"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    warn "$INSTALL_DIR is not on your PATH"
    printf 'add it to your shell config:\n  export PATH="%s:$PATH"\n' "$INSTALL_DIR"
    ;;
esac

info "installed $BIN $TARGET -> $INSTALL_DIR/$BIN"
info "next steps:"
info "  - spicetify must be installed (https://spicetify.app)"
info "  - export GITHUB_TOKEN=... to avoid API rate limits"
command -v "$BIN" >/dev/null 2>&1 && "$BIN" --version || true
