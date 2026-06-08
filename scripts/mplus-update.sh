#!/usr/bin/env bash
# =============================================================================
# mplus-update.sh — trigger POST /update/all on the mplus-tracker API
#
# Intended to be run by systemd on a timer (every 3 hours).
# Logs to systemd journal via stderr; use `journalctl -u mplus-update` to view.
#
# Config is read from /etc/mplus-tracker/update.conf (see setup instructions).
# =============================================================================

set -euo pipefail

# ── Config file ───────────────────────────────────────────────────────────────
CONF_FILE="${MPLUS_CONF:-/etc/mplus-tracker/update.conf}"

if [[ ! -f "$CONF_FILE" ]]; then
  echo "[ERROR] Config file not found: $CONF_FILE" >&2
  echo "[ERROR] Run the setup script or create it manually." >&2
  exit 1
fi

# shellcheck source=/dev/null
source "$CONF_FILE"

# ── Validate required vars ────────────────────────────────────────────────────
: "${MPLUS_TRACKER_URL:?MPLUS_TRACKER_URL must be set in $CONF_FILE}"
: "${MPLUS_API_TOKEN:?MPLUS_API_TOKEN must be set in $CONF_FILE}"

TRACKER_URL="${MPLUS_TRACKER_URL%/}"   # strip trailing slash

# ── Optional config with defaults ─────────────────────────────────────────────
TIMEOUT="${MPLUS_TIMEOUT:-300}"        # curl timeout in seconds (5 min default)
MAX_RETRIES="${MPLUS_MAX_RETRIES:-3}"  # retries on transient failure
RETRY_DELAY="${MPLUS_RETRY_DELAY:-30}" # seconds between retries

# ── Logging helpers ───────────────────────────────────────────────────────────
log()  { echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] [INFO]  $*"; }
warn() { echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] [WARN]  $*" >&2; }
err()  { echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] [ERROR] $*" >&2; }

# ── Health check before triggering update ────────────────────────────────────
log "Checking tracker health at ${TRACKER_URL}/health"

health_status=$(curl --silent --max-time 10 \
  -o /dev/null -w "%{http_code}" \
  "${TRACKER_URL}/health" 2>&1) || true

if [[ "$health_status" != "200" ]]; then
  err "Health check failed (HTTP ${health_status}). Aborting update."
  exit 1
fi

log "Health check OK."

# ── Trigger update/all with retry logic ──────────────────────────────────────
attempt=0
success=false

while [[ $attempt -lt $MAX_RETRIES ]]; do
  attempt=$(( attempt + 1 ))
  log "Triggering POST /update/all (attempt ${attempt}/${MAX_RETRIES})…"

  # Capture both HTTP status code and response body
  http_body=$(mktemp)
  http_code=$(curl \
    --silent \
    --show-error \
    --max-time "$TIMEOUT" \
    --write-out "%{http_code}" \
    --output "$http_body" \
    -X POST \
    -H "Authorization: Bearer ${MPLUS_API_TOKEN}" \
    -H "Content-Type: application/json" \
    "${TRACKER_URL}/update/all" 2>&1) || curl_exit=$?

  body=$(cat "$http_body")
  rm -f "$http_body"

  if [[ "$http_code" == "200" ]]; then
    # Parse summary fields from JSON response (no jq dependency)
    total=$(echo "$body"     | grep -o '"total_characters":[0-9]*'  | grep -o '[0-9]*' || echo "?")
    updated=$(echo "$body"   | grep -o '"updated_ok":[0-9]*'        | grep -o '[0-9]*' || echo "?")
    failed=$(echo "$body"    | grep -o '"failed":[0-9]*'            | grep -o '[0-9]*' || echo "?")
    pruned=$(echo "$body"    | grep -o '"pruned":[0-9]*'            | grep -o '[0-9]*' || echo "?")

    log "Update complete — total=${total} updated=${updated} failed=${failed} pruned=${pruned}"
    success=true
    break

  elif [[ "$http_code" == "401" ]]; then
    err "Unauthorized (401). Check MPLUS_API_TOKEN in ${CONF_FILE}."
    exit 1  # No point retrying auth failures

  else
    warn "Update failed (HTTP ${http_code}). Body: ${body}"
    if [[ $attempt -lt $MAX_RETRIES ]]; then
      warn "Retrying in ${RETRY_DELAY}s…"
      sleep "$RETRY_DELAY"
    fi
  fi
done

if [[ "$success" != "true" ]]; then
  err "All ${MAX_RETRIES} attempts failed. Check tracker logs."
  exit 1
fi
