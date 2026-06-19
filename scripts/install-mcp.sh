#!/usr/bin/env bash
#
# install-mcp.sh — Install the aiclaw MCP server into another project.
#
# It builds the release binary (if needed) and registers an "aiclaw" stdio
# MCP server entry into a target MCP client config. Existing entries are merged,
# not overwritten (requires jq or python3).
#
# Usage:
#   scripts/install-mcp.sh [TARGET_PROJECT_DIR] [options]
#
# Arguments:
#   TARGET_PROJECT_DIR   Project to install into. Default: current directory.
#
# Options:
#   --name <name>        MCP server key. Default: aiclaw
#   --target <kind>      Where to register: project | claude-desktop | print
#                        - project        -> <TARGET_PROJECT_DIR>/.mcp.json   (default)
#                        - claude-desktop -> Claude Desktop global config
#                        - print          -> print the JSON snippet, write nothing
#   --wechat-dir <path>  Sets WECHAT_CHANNEL_DIR env for the server.
#                        Default: <TARGET_PROJECT_DIR>/.claude/channels/wechat
#   --log <level>        RUST_LOG level. Default: info
#   --no-build           Do not build; reuse an existing release binary.
#   -h, --help           Show this help.
#
# Examples:
#   scripts/install-mcp.sh ~/code/my-app
#   scripts/install-mcp.sh ~/code/my-app --target claude-desktop
#   scripts/install-mcp.sh --target print
#
set -euo pipefail

# ── Resolve repo root (this script lives in <repo>/scripts) ──
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ── Defaults ──
SERVER_NAME="aiclaw"
TARGET_KIND="project"
TARGET_DIR="$(pwd)"
WECHAT_DIR=""
LOG_LEVEL="info"
DO_BUILD=1

# ── Parse args ──
while [[ $# -gt 0 ]]; do
  case "$1" in
    --name)        SERVER_NAME="${2:?--name needs a value}"; shift 2 ;;
    --target)      TARGET_KIND="${2:?--target needs a value}"; shift 2 ;;
    --wechat-dir)  WECHAT_DIR="${2:?--wechat-dir needs a value}"; shift 2 ;;
    --log)         LOG_LEVEL="${2:?--log needs a value}"; shift 2 ;;
    --no-build)    DO_BUILD=0; shift ;;
    -h|--help)     sed -n '2,40p' "$0"; exit 0 ;;
    -*)            echo "error: unknown option: $1" >&2; exit 2 ;;
    *)             TARGET_DIR="$1"; shift ;;
  esac
done

case "$TARGET_KIND" in
  project|claude-desktop|print) ;;
  *) echo "error: --target must be project | claude-desktop | print" >&2; exit 2 ;;
esac

# ── Normalize paths ──
if [[ "$TARGET_KIND" != "print" ]]; then
  mkdir -p "$TARGET_DIR"
  TARGET_DIR="$(cd "$TARGET_DIR" && pwd)"
fi
if [[ -z "$WECHAT_DIR" ]]; then
  WECHAT_DIR="$TARGET_DIR/.claude/channels/wechat"
fi

BIN_PATH="$REPO_ROOT/target/release/$SERVER_NAME"

# ── Build the binary ──
if [[ "$DO_BUILD" -eq 1 ]]; then
  echo ">> building release binary ..." >&2
  ( cd "$REPO_ROOT" && cargo build --release )
fi
if [[ "$TARGET_KIND" != "print" && ! -x "$BIN_PATH" ]]; then
  echo "error: binary not found at $BIN_PATH" >&2
  echo "       run without --no-build, or 'cargo build --release' first." >&2
  exit 1
fi

# ── A JSON merger: prefer jq, fall back to python3 ──
# merge_json <config_file> <server_name> <bin> <wechat_dir> <log>
# Reads existing config (or {}), inserts mcpServers[server_name], writes back.
merge_json() {
  local cfg="$1" name="$2" bin="$3" wdir="$4" log="$5"
  mkdir -p "$(dirname "$cfg")"
  [[ -f "$cfg" ]] || echo '{}' > "$cfg"

  if command -v jq >/dev/null 2>&1; then
    local tmp; tmp="$(mktemp)"
    jq \
      --arg name "$name" --arg bin "$bin" --arg wdir "$wdir" --arg log "$log" \
      '.mcpServers = (.mcpServers // {})
       | .mcpServers[$name] = {
           command: $bin,
           args: ["--mcp"],
           env: { WECHAT_CHANNEL_DIR: $wdir, RUST_LOG: $log }
         }' \
      "$cfg" > "$tmp"
    mv "$tmp" "$cfg"
  elif command -v python3 >/dev/null 2>&1; then
    name="$name" bin="$bin" wdir="$wdir" log="$log" cfg="$cfg" python3 - <<'PY'
import json, os
cfg = os.environ["cfg"]
with open(cfg) as f:
    try:
        data = json.load(f)
    except Exception:
        data = {}
if not isinstance(data, dict):
    data = {}
servers = data.setdefault("mcpServers", {})
servers[os.environ["name"]] = {
    "command": os.environ["bin"],
    "args": ["--mcp"],
    "env": {
        "WECHAT_CHANNEL_DIR": os.environ["wdir"],
        "RUST_LOG": os.environ["log"],
    },
}
with open(cfg, "w") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
  else
    echo "error: need jq or python3 to merge JSON config" >&2
    exit 1
  fi
}

print_snippet() {
  cat <<JSON
{
  "mcpServers": {
    "$SERVER_NAME": {
      "command": "$BIN_PATH",
      "args": ["--mcp"],
      "env": {
        "WECHAT_CHANNEL_DIR": "$WECHAT_DIR",
        "RUST_LOG": "$LOG_LEVEL"
      }
    }
  }
}
JSON
}

# ── Apply ──
case "$TARGET_KIND" in
  print)
    print_snippet
    ;;
  project)
    CFG="$TARGET_DIR/.mcp.json"
    merge_json "$CFG" "$SERVER_NAME" "$BIN_PATH" "$WECHAT_DIR" "$LOG_LEVEL"
    echo ">> registered '$SERVER_NAME' in $CFG" >&2
    ;;
  claude-desktop)
    case "$(uname -s)" in
      Darwin) CFG="$HOME/Library/Application Support/Claude/claude_desktop_config.json" ;;
      Linux)  CFG="$HOME/.config/Claude/claude_desktop_config.json" ;;
      *)      echo "error: unsupported OS for claude-desktop target" >&2; exit 1 ;;
    esac
    merge_json "$CFG" "$SERVER_NAME" "$BIN_PATH" "$WECHAT_DIR" "$LOG_LEVEL"
    echo ">> registered '$SERVER_NAME' in $CFG" >&2
    echo ">> restart Claude Desktop to pick up the new server." >&2
    ;;
esac

if [[ "$TARGET_KIND" != "print" ]]; then
  echo ">> binary:       $BIN_PATH" >&2
  echo ">> wechat dir:   $WECHAT_DIR" >&2
  echo ">> tools:        send (closed), list_peers / login (experimental)" >&2
  echo ">> done." >&2
fi
