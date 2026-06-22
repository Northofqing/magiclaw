#!/usr/bin/env bash
# Unify daemon startup and API-token issuance against a single SQLite database.
#
# Problem this solves: the daemon validates incoming bearer tokens against the
# ApiClientRegistry table in MAGICLAW_DB_PATH, while `magiclaw auth issue` writes
# tokens into MAGICLAW_DB_PATH, and the `send` CLI authenticates with
# MAGICLAW_API_TOKEN. If any of these point at different databases / tokens you
# get 401 unauthorized. This script pins all three to one DB and one freshly
# issued token, persists them into .env (auto-loaded by the binary on startup),
# then launches the daemon.
#
# Usage:
#   scripts/daemon-up.sh                 # issue token + start daemon
#   DB_PATH=/abs/path.db scripts/daemon-up.sh
#   TTL_SECS=604800 scripts/daemon-up.sh # 7-day token
#   REUSE_TOKEN=1 scripts/daemon-up.sh   # keep existing .env token, just start
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

DB_PATH="${DB_PATH:-${REPO_ROOT}/data/magiclaw.db}"
TTL_SECS="${TTL_SECS:-604800}"   # default 7 days
PROJECT="${PROJECT:-local}"
CLIENT_NAME="${CLIENT_NAME:-local-daemon}"
SCOPES="${SCOPES:-send,window_status}"
ENV_FILE="${REPO_ROOT}/.env"
BIN="${REPO_ROOT}/target/release/magiclaw"

mkdir -p "$(dirname "${DB_PATH}")"

echo "[daemon-up] repo_root=${REPO_ROOT}"
echo "[daemon-up] db_path=${DB_PATH}"

echo "[daemon-up] building release binary ..."
cargo build --release --quiet

# Upsert a KEY=VALUE pair into .env (create file if missing).
upsert_env() {
  local key="$1" value="$2"
  touch "${ENV_FILE}"
  if grep -qE "^${key}=" "${ENV_FILE}"; then
    # Use a temp file to stay portable across BSD/GNU sed.
    grep -vE "^${key}=" "${ENV_FILE}" > "${ENV_FILE}.tmp"
    mv "${ENV_FILE}.tmp" "${ENV_FILE}"
  fi
  printf '%s=%s\n' "${key}" "${value}" >> "${ENV_FILE}"
}

if [[ "${REUSE_TOKEN:-0}" == "1" ]]; then
  TOKEN="$(grep -E '^MAGICLAW_API_TOKEN=' "${ENV_FILE}" 2>/dev/null | head -n1 | cut -d= -f2-)"
  if [[ -z "${TOKEN}" ]]; then
    echo "[daemon-up] REUSE_TOKEN=1 but no MAGICLAW_API_TOKEN found in .env" >&2
    exit 1
  fi
  echo "[daemon-up] reusing existing token from .env"
else
  echo "[daemon-up] issuing token (project=${PROJECT} name=${CLIENT_NAME} scopes=${SCOPES} ttl=${TTL_SECS}s) ..."
  ISSUE_OUT="$(MAGICLAW_DB_PATH="${DB_PATH}" "${BIN}" auth issue \
    --project "${PROJECT}" \
    --name "${CLIENT_NAME}" \
    --scopes "${SCOPES}" \
    --ttl-secs "${TTL_SECS}")"
  TOKEN="$(printf '%s\n' "${ISSUE_OUT}" | grep -E '^token=' | head -n1 | cut -d= -f2-)"
  if [[ -z "${TOKEN}" ]]; then
    echo "[daemon-up] failed to parse issued token from:" >&2
    printf '%s\n' "${ISSUE_OUT}" >&2
    exit 1
  fi
fi

upsert_env "MAGICLAW_DB_PATH" "${DB_PATH}"
upsert_env "MAGICLAW_API_TOKEN" "${TOKEN}"
echo "[daemon-up] .env updated (MAGICLAW_DB_PATH, MAGICLAW_API_TOKEN)"
echo "[daemon-up] token=${TOKEN}"
echo "[daemon-up] starting daemon — send with: cargo run -- send --message '...'"

# Daemon mode is the no-argument default (there is no `daemon` subcommand).
exec env MAGICLAW_DB_PATH="${DB_PATH}" MAGICLAW_API_TOKEN="${TOKEN}" "${BIN}"
