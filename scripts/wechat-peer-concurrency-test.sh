#!/usr/bin/env bash
set -euo pipefail

# Concurrent same-peer send test against daemon /api/send.
# Usage example:
#   scripts/wechat-peer-concurrency-test.sh \
#     --to o9cq80-xxx@im.wechat \
#     --message "并发验证" \
#     --total 30 \
#     --concurrency 8

API_ADDR_DEFAULT="127.0.0.1:18011"
TOTAL=20
CONCURRENCY=6
TO=""
MESSAGE="并发测试"
API_ADDR="${MAGICLAW_API_ADDR:-$API_ADDR_DEFAULT}"
API_TOKEN="${MAGICLAW_API_TOKEN:-}"
TIMEOUT_SECS=20

print_usage() {
  cat <<'USAGE'
Usage:
  scripts/wechat-peer-concurrency-test.sh --to <peer_id> [options]

Options:
  --to <peer_id>          WeChat peer id (required)
  --message <text>        Base message text (default: 并发测试)
  --total <n>             Total sends (default: 20)
  --concurrency <n>       Parallel workers (default: 6)
  --addr <host:port>      Daemon API addr (default: 127.0.0.1:18011)
  --token <token>         Bearer token (default: MAGICLAW_API_TOKEN)
  --timeout <secs>        Per request timeout (default: 20)
  -h, --help              Show this help
USAGE
}

json_escape() {
  local s="$1"
  s=${s//\\/\\\\}
  s=${s//"/\\"}
  s=${s//$'\n'/\\n}
  printf '%s' "$s"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --to)
      TO="${2:-}"
      shift 2
      ;;
    --message)
      MESSAGE="${2:-}"
      shift 2
      ;;
    --total)
      TOTAL="${2:-}"
      shift 2
      ;;
    --concurrency)
      CONCURRENCY="${2:-}"
      shift 2
      ;;
    --addr)
      API_ADDR="${2:-}"
      shift 2
      ;;
    --token)
      API_TOKEN="${2:-}"
      shift 2
      ;;
    --timeout)
      TIMEOUT_SECS="${2:-}"
      shift 2
      ;;
    -h|--help)
      print_usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      print_usage
      exit 1
      ;;
  esac
done

if [[ -z "$TO" ]]; then
  echo "--to is required" >&2
  print_usage
  exit 1
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "curl not found" >&2
  exit 1
fi

if ! [[ "$TOTAL" =~ ^[0-9]+$ ]] || ! [[ "$CONCURRENCY" =~ ^[0-9]+$ ]] || ! [[ "$TIMEOUT_SECS" =~ ^[0-9]+$ ]]; then
  echo "--total, --concurrency, --timeout must be integers" >&2
  exit 1
fi

if [[ "$TOTAL" -le 0 || "$CONCURRENCY" -le 0 ]]; then
  echo "--total and --concurrency must be > 0" >&2
  exit 1
fi

TMP_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

AUTH_HEADER=()
if [[ -n "$API_TOKEN" ]]; then
  AUTH_HEADER=(-H "Authorization: Bearer ${API_TOKEN}")
fi

run_one() {
  local idx="$1"
  local text
  text="${MESSAGE} #${idx}"

  local payload
  payload=$(printf '{"to":"%s","text":"%s"}' "$(json_escape "$TO")" "$(json_escape "$text")")

  local body_file="$TMP_DIR/body_${idx}.json"
  local code_file="$TMP_DIR/code_${idx}.txt"
  local err_file="$TMP_DIR/err_${idx}.txt"

  if curl -sS -m "$TIMEOUT_SECS" \
    -H "Content-Type: application/json" \
    "${AUTH_HEADER[@]}" \
    -o "$body_file" \
    -w "%{http_code}" \
    -X POST "http://${API_ADDR}/api/send" \
    -d "$payload" >"$code_file" 2>"$err_file"; then
    :
  else
    echo "curl_failed" >"$code_file"
  fi
}

echo "[peer-concurrency] begin total=${TOTAL} concurrency=${CONCURRENCY} to=${TO} addr=${API_ADDR}"
echo "[peer-concurrency] tip: set MAGICLAW_API_SEND_DEBUG=1 when starting daemon to include diagnostics in /api/send response"

idx=1
while [[ "$idx" -le "$TOTAL" ]]; do
  while [[ "$(jobs -rp | wc -l | tr -d ' ')" -ge "$CONCURRENCY" ]]; do
    sleep 0.05
  done
  run_one "$idx" &
  idx=$((idx + 1))
done
wait

ok=0
fail=0
curl_fail=0
http_200=0
http_other=0
diag_present=0

for i in $(seq 1 "$TOTAL"); do
  code="$(cat "$TMP_DIR/code_${i}.txt" 2>/dev/null || true)"
  body="$(cat "$TMP_DIR/body_${i}.json" 2>/dev/null || true)"

  if [[ "$code" == "curl_failed" ]]; then
    curl_fail=$((curl_fail + 1))
    fail=$((fail + 1))
    continue
  fi

  if [[ "$code" == "200" ]]; then
    http_200=$((http_200 + 1))
  else
    http_other=$((http_other + 1))
  fi

  if printf '%s' "$body" | grep -Eq '"ok"[[:space:]]*:[[:space:]]*true'; then
    ok=$((ok + 1))
  else
    fail=$((fail + 1))
  fi

  if printf '%s' "$body" | grep -Eq '"diagnostics"[[:space:]]*:'; then
    diag_present=$((diag_present + 1))
  fi
done

success_rate="$(awk -v o="$ok" -v t="$TOTAL" 'BEGIN{ if (t==0) print "0.00"; else printf "%.2f", (o/t)*100 }')"

echo "[peer-concurrency] done total=${TOTAL} ok=${ok} fail=${fail} success_rate=${success_rate}% http200=${http_200} http_other=${http_other} curl_fail=${curl_fail} diagnostics=${diag_present}"

echo "[peer-concurrency] sample failures (up to 10):"
shown=0
for i in $(seq 1 "$TOTAL"); do
  code="$(cat "$TMP_DIR/code_${i}.txt" 2>/dev/null || true)"
  body="$(cat "$TMP_DIR/body_${i}.json" 2>/dev/null || true)"
  if [[ "$code" == "curl_failed" ]]; then
    err="$(cat "$TMP_DIR/err_${i}.txt" 2>/dev/null || true)"
    echo "  [case ${i}] curl_failed err=${err}"
    shown=$((shown + 1))
  elif ! printf '%s' "$body" | grep -Eq '"ok"[[:space:]]*:[[:space:]]*true'; then
    echo "  [case ${i}] http=${code} body=${body}"
    shown=$((shown + 1))
  fi

  if [[ "$shown" -ge 10 ]]; then
    break
  fi
done
