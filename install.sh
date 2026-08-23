#!/usr/bin/env bash
# spice-pm installer for Linux and macOS.
#
# Usage:
#   ./install.sh [--version vX.Y.Z] [--dir PATH] [--allow-root]
#
# Downloads the latest (or given) release from GitHub, verifies its
# checksum when a .sha256 sidecar is published, checks the binary runs on
# this system, and installs it into --dir (default: ~/.local/bin).

set -euo pipefail

REPO="KamilWachnicki/spicetify-pm"
BIN="spice-pm"
INSTALL_DIR="${SPICEPM_INSTALL_DIR:-$HOME/.local/bin}"
TARGET=""
ALLOW_ROOT=false

# colors only when attached to a terminal; plain text when piped/CI
if [ -t 1 ]; then
  info() { printf '\033[32m%s\033[0m\n' "$*"; }
  warn() { printf '\033[33m%s\033[0m\n' "$*"; }
else
  info() { printf '%s\n' "$*"; }
  warn() { printf '%s\n' "$*"; }
fi
err() { printf '%s\n' "$*" >&2; }

usage() {
  cat <<EOF
Usage: $(basename "$0") [--version vX.Y.Z] [--dir PATH] [--allow-root]

  --version vX.Y.Z   Install a specific release tag (default: latest;
                     a leading "v" is added if missing)
  --dir PATH         Installation directory (default: ~/.local/bin,
                     overridable with SPICEPM_INSTALL_DIR)
  --allow-root       Permit running as root/sudo (SPICEPM_ALLOW_ROOT=1
                     works too; you probably do not want this)
  -h, --help         Show this help

Environment:
  SPICEPM_GITHUB_TOKEN / GITHUB_TOKEN / GH_TOKEN
                     Sent to the GitHub API to avoid unauthenticated
                     rate limits during install.
EOF
}

need_value() { # $1 = option name, $2 = remaining arg count, $3 = next arg
  if [[ "$2" -lt 2 || -z "${3:-}" ]]; then
    err "option $1 needs a value"
    usage
    exit 1
  fi
}

print_path_instructions() {
  # SHELL is the user's login shell, which is the best signal available when
  # the installer itself is being piped to bash.
  local shell_name="${SHELL:-}"
  shell_name="${shell_name##*/}"

  warn "$INSTALL_DIR is not on your PATH"
  case "$shell_name" in
    bash)
      local config="$HOME/.bashrc"
      # Terminal.app starts a login bash on older macOS releases.
      [[ "$os" == macos ]] && config="$HOME/.bash_profile"
      printf 'add this line to %s, then open a new terminal:\n  export PATH="%s:$PATH"\n' \
        "$config" "$INSTALL_DIR"
      ;;
    zsh)
      printf 'add this line to ~/.zshrc, then open a new terminal:\n  export PATH="%s:$PATH"\n' \
        "$INSTALL_DIR"
      ;;
    fish)
      printf 'run this from fish to add it permanently:\n  fish_add_path "%s"\n' \
        "$INSTALL_DIR"
      ;;
    nu)
      local nu_config="${XDG_CONFIG_HOME:-$HOME/.config}/nushell/config.nu"
      printf 'add this line to %s, then start a new Nushell session:\n  $env.PATH = ($env.PATH | prepend "%s")\n' \
        "$nu_config" "$INSTALL_DIR"
      ;;
    csh|tcsh)
      printf 'add this line to ~/.tcshrc, then start a new session:\n  setenv PATH "%s:$PATH"\n' \
        "$INSTALL_DIR"
      ;;
    sh|dash|ksh)
      printf 'add this line to ~/.profile, then start a new session:\n  export PATH="%s:$PATH"\n' \
        "$INSTALL_DIR"
      ;;
    *)
      printf 'add this line to your shell startup file, then start a new session:\n  export PATH="%s:$PATH"\n' \
        "$INSTALL_DIR"
      ;;
  esac
}

path_contains_install_dir() {
  local entry
  local IFS=:
  local entries=()
  read -r -a entries <<< "${PATH:-}"
  for entry in "${entries[@]}"; do
    [[ "$entry" == "$INSTALL_DIR" ]] && return 0
  done
  return 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      need_value --version "$#" "${2:-}"
      TARGET="$2"; shift 2 ;;
    --dir)
      need_value --dir "$#" "${2:-}"
      INSTALL_DIR="$2"; shift 2 ;;
    --allow-root) ALLOW_ROOT=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) err "unknown argument: $1"; usage; exit 1 ;;
  esac
done

# A relative PATH entry only works from the directory where it was installed.
# Make an explicit --dir relative to the caller's current directory instead.
if [[ "$INSTALL_DIR" != /* ]]; then
  INSTALL_DIR="$(pwd -P)/$INSTALL_DIR"
fi

if [[ ${EUID:-$(id -u)} -eq 0 && "$ALLOW_ROOT" == false && "${SPICEPM_ALLOW_ROOT:-}" != "1" ]]; then
  err "refusing to run as root:"
  err "the binary would land in root's home directory and break future"
  err "self-updates. Run this as your normal user instead."
  err "to override deliberately: pass --allow-root or set SPICEPM_ALLOW_ROOT=1"
  exit 1
fi

for cmd in curl tar install; do
  command -v "$cmd" >/dev/null || { err "$cmd is required"; exit 1; }
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

if [[ "$os" == linux ]] && command -v ldd >/dev/null && ldd --version 2>&1 | grep -qi musl; then
  warn "musl libc detected: the published binaries are built against glibc"
  warn "and may fail to load. If 'spice-pm' errors after installing, build"
  warn "from source instead: cargo install --git https://github.com/$REPO"
fi

CURL_RETRY=(--retry 2 --retry-delay 1)

# Fetch the latest release tag from the GitHub API. Uses GITHUB_TOKEN
# (or SPICEPM_GITHUB_TOKEN / GH_TOKEN) as a bearer token when set, so
# installs don't trip over unauthenticated rate limits.
fetch_latest_tag() {
  local url="https://api.github.com/repos/$REPO/releases/latest"
  local token="${SPICEPM_GITHUB_TOKEN:-${GITHUB_TOKEN:-${GH_TOKEN:-}}}"
  # ${auth[@]+...} guard: empty arrays must not expand under bash 3.2 -u
  local auth_args=()
  [[ -n "$token" ]] && auth_args=(-H "Authorization: Bearer $token")
  # no -f: we want the body + status code even on failure, for messages
  curl ${auth_args[@]+"${auth_args[@]}"} -sS --retry 2 --retry-delay 1 \
    -w '\n%{http_code}' "$url"
}

if [[ -z "$TARGET" ]]; then
  info "fetching the latest release"
  response=$(fetch_latest_tag) || {
    err "could not reach api.github.com; check your connection"
    exit 1
  }
  status=${response##*$'\n'}
  body=${response%$'\n'*}
  TARGET=$(printf '%s' "$body" |
    sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')
  if [[ -z "$TARGET" ]]; then
    case "$status" in
      200) err "no releases published on $REPO yet"
           err "build from source instead:  cargo install --git https://github.com/$REPO" ;;
      403|429) err "rate limited by the GitHub API while looking up the latest release"
               err "set SPICEPM_GITHUB_TOKEN (or GITHUB_TOKEN/GH_TOKEN) and re-run" ;;
      *) err "GitHub API returned HTTP $status while looking up the latest release" ;;
    esac
    exit 1
  fi
fi

# tolerate bare versions: --version 0.4.0 means v0.4.0
[[ "$TARGET" == v* ]] || TARGET="v$TARGET"

asset="$BIN-$TARGET-$arch-$os.tar.gz"
url="https://github.com/$REPO/releases/download/$TARGET/$asset"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

info "downloading $asset"
status=$(curl -sSL --retry 2 --retry-delay 1 --progress-bar \
  -o "$tmp/$asset" -w '%{http_code}' "$url") || {
  err "download failed: $url"
  exit 1
}
if [[ "$status" != 200 ]]; then
  err "download failed with HTTP $status: $url"
  err "check that release $TARGET exists and has a build for $arch-$os"
  exit 1
fi

# Verify when the release publishes a .sha256 sidecar. A missing sidecar is
# acceptable; a network/server failure is not, because silently bypassing an
# expected verification would make a transient outage look safe.
checksum_status=$(curl -sSL ${CURL_RETRY[@]+"${CURL_RETRY[@]}"} \
  -o "$tmp/$asset.sha256" -w '%{http_code}' "$url.sha256") || {
  err "could not download the release checksum: $url.sha256"
  exit 1
}
if [[ "$checksum_status" == 200 ]]; then
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
elif [[ "$checksum_status" == 404 ]]; then
  warn "release has no checksum sidecar; skipping verification"
else
  err "could not download the release checksum (HTTP $checksum_status): $url.sha256"
  exit 1
fi

# Extract only the expected executable. This avoids unpacking README files or
# any unexpected archive entries, including paths outside the temp directory.
# --no-same-owner prevents ownership changes during a deliberate root install.
tar --no-same-owner -xzf "$tmp/$asset" -C "$tmp" "$BIN"
[[ -f "$tmp/$BIN" && ! -L "$tmp/$BIN" ]] || {
  err "archive did not contain a regular $BIN executable"
  exit 1
}

# run the binary once from the temp dir so loader problems (too-old
# glibc, musl systems, broken architectures) surface BEFORE anything is
# installed; bytes are already checksum-verified at this point
if ! "$tmp/$BIN" --version >/dev/null; then
  err "the downloaded $BIN does not run on this system"
  err "(usually glibc too old for these builds, or a musl-based distro)"
  err "alternatives: upgrade your system, or build from source:"
  err "  cargo install --git https://github.com/$REPO"
  exit 1
fi

if [[ -x "$INSTALL_DIR/$BIN" ]]; then
  old_version=$("$INSTALL_DIR/$BIN" --version 2>/dev/null | tail -1 || true)
  if [[ -n "$old_version" ]]; then
    info "upgrading ($old_version -> $TARGET)"
  else
    info "replacing existing installation at $INSTALL_DIR/$BIN"
  fi
else
  info "fresh install into $INSTALL_DIR"
fi

mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmp/$BIN" "$INSTALL_DIR/$BIN"

if ! path_contains_install_dir; then
  print_path_instructions
fi

info "installed $BIN $TARGET -> $INSTALL_DIR/$BIN"
info "next steps:"
info "  - spicetify must be installed (https://spicetify.app)"
info "  - set GITHUB_TOKEN in the enviroment to avoid API rate limits (optional)"
"$INSTALL_DIR/$BIN" --version || true
