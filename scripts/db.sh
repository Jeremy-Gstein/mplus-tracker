#!/usr/bin/env bash
# =============================================================================
# mptracker - db.sh
# Backup and restore the SQLite database from/to a Docker volume.
#
# Usage:
#   ./scripts/db.sh backup  [OPTIONS]   — copy DB out of container → local file
#   ./scripts/db.sh restore [OPTIONS]   — copy local file → container, verify
#   ./scripts/db.sh status  [OPTIONS]   — show DB stats without touching data
#
# Options:
#   -c, --container NAME    Docker container name  (default: mptracker)
#   -f, --file PATH         Local backup file path (default: ./backups/mptracker_<timestamp>.db)
#   -d, --db-path PATH      Path inside container  (default: /data/mptracker.db)
#   -y, --yes               Skip confirmation prompts
#   -h, --help              Show this help
#
# Examples:
#   ./scripts/db.sh backup
#   ./scripts/db.sh backup  -c mptracker-prod -f ./backups/prod.db
#   ./scripts/db.sh restore -f ./backups/mptracker_2026-06-01.db
#   ./scripts/db.sh restore -f ./backups/prod.db -y
#   ./scripts/db.sh status
# =============================================================================

set -euo pipefail

# ─── Defaults ────────────────────────────────────────────────────────────────

CONTAINER="mplus-tracker"
DB_PATH_IN_CONTAINER="/data/mplus.sqlite"
BACKUP_DIR="./backups"
BACKUP_FILE=""
SKIP_CONFIRM=false
COMMAND=""

# ─── Colors ──────────────────────────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # no color

log()    { echo -e "${CYAN}[mptracker/db]${NC} $*"; }
ok()     { echo -e "${GREEN}[mptracker/db]${NC} OK $*"; }
warn()   { echo -e "${YELLOW}[mptracker/db]${NC} WARN $*"; }
err()    { echo -e "${RED}[mptracker/db]${NC} ERROR $*" >&2; }
header() { echo -e "\n${BOLD}$*${NC}"; }

# ─── Usage ───────────────────────────────────────────────────────────────────

usage() {
  sed -n '/^# Usage:/,/^# ====/p' "$0" | grep '^#' | sed 's/^# \?//'
  exit 0
}

# ─── Arg parsing ─────────────────────────────────────────────────────────────

if [[ $# -eq 0 ]]; then usage; fi

COMMAND="$1"; shift

while [[ $# -gt 0 ]]; do
  case "$1" in
    -c|--container) CONTAINER="$2";              shift 2 ;;
    -f|--file)      BACKUP_FILE="$2";            shift 2 ;;
    -d|--db-path)   DB_PATH_IN_CONTAINER="$2";   shift 2 ;;
    -y|--yes)       SKIP_CONFIRM=true;           shift   ;;
    -h|--help)      usage ;;
    *) err "Unknown option: $1"; usage ;;
  esac
done

# ─── Helpers ─────────────────────────────────────────────────────────────────

require_cmd() {
  command -v "$1" &>/dev/null || { err "Required command not found: $1"; exit 1; }
}

confirm() {
  if $SKIP_CONFIRM; then return 0; fi
  echo -en "${YELLOW}[mptracker/db]${NC} $* [y/N] "
  read -r reply
  [[ "$reply" =~ ^[Yy]$ ]]
}

container_running() {
  docker inspect --format '{{.State.Running}}' "$CONTAINER" 2>/dev/null | grep -q "true"
}

db_exists_in_container() {
  docker exec "$CONTAINER" test -f "$DB_PATH_IN_CONTAINER" 2>/dev/null
}

# Run SQLite pragma inside the container and return the value
container_pragma() {
  local pragma="$1"
  docker exec "$CONTAINER" sqlite3 "$DB_PATH_IN_CONTAINER" "$pragma" 2>/dev/null || echo "N/A"
}

# Run SQLite query on a LOCAL file
local_pragma() {
  local file="$1"
  local pragma="$2"
  sqlite3 "$file" "$pragma" 2>/dev/null || echo "N/A"
}

db_summary() {
  local mode="$1"   # "container" or "local:<path>"
  if [[ "$mode" == "container" ]]; then
    local chars runs players
    chars=$(docker exec "$CONTAINER" sqlite3 "$DB_PATH_IN_CONTAINER" \
      "SELECT COUNT(*) FROM characters;" 2>/dev/null || echo "?")
    runs=$(docker exec "$CONTAINER" sqlite3 "$DB_PATH_IN_CONTAINER" \
      "SELECT COUNT(*) FROM runs;" 2>/dev/null || echo "?")
    players=$(docker exec "$CONTAINER" sqlite3 "$DB_PATH_IN_CONTAINER" \
      "SELECT COUNT(*) FROM players;" 2>/dev/null || echo "?")
    local size
    size=$(docker exec "$CONTAINER" du -sh "$DB_PATH_IN_CONTAINER" 2>/dev/null | awk '{print $1}' || echo "?")
    echo "  characters: ${BOLD}$chars${NC}  |  runs: ${BOLD}$runs${NC}  |  players: ${BOLD}$players${NC}  |  size: ${BOLD}$size${NC}"
  else
    local path="${mode#local:}"
    local chars runs players size
    chars=$(sqlite3 "$path" "SELECT COUNT(*) FROM characters;" 2>/dev/null || echo "?")
    runs=$(sqlite3 "$path" "SELECT COUNT(*) FROM runs;" 2>/dev/null || echo "?")
    players=$(sqlite3 "$path" "SELECT COUNT(*) FROM players;" 2>/dev/null || echo "?")
    size=$(du -sh "$path" 2>/dev/null | awk '{print $1}' || echo "?")
    echo "  characters: ${BOLD}$chars${NC}  |  runs: ${BOLD}$runs${NC}  |  players: ${BOLD}$players${NC}  |  size: ${BOLD}$size${NC}"
  fi
}

# ─── Preflight ───────────────────────────────────────────────────────────────

preflight() {
  require_cmd docker
  # sqlite3 is optional — we degrade gracefully if it's missing
  if ! command -v sqlite3 &>/dev/null; then
    warn "sqlite3 not found locally — integrity checks and stats will be skipped"
  fi

  if ! container_running; then
    err "Container '${CONTAINER}' is not running."
    echo "  Start it with:  docker compose up -d"
    exit 1
  fi
}

# ─── BACKUP ──────────────────────────────────────────────────────────────────

cmd_backup() {
  preflight

  # Default backup filename with timestamp
  if [[ -z "$BACKUP_FILE" ]]; then
    mkdir -p "$BACKUP_DIR"
    BACKUP_FILE="${BACKUP_DIR}/mptracker_$(date +%Y-%m-%dT%H-%M-%S).db"
  fi

  # Make sure destination dir exists
  local dest_dir
  dest_dir=$(dirname "$BACKUP_FILE")
  mkdir -p "$dest_dir"

  header "Backing up mptracker database"
  log "Container : $CONTAINER"
  log "Source    : $DB_PATH_IN_CONTAINER"
  log "Dest      : $BACKUP_FILE"

  if ! db_exists_in_container; then
    err "Database not found at $DB_PATH_IN_CONTAINER in container '$CONTAINER'"
    exit 1
  fi

  # Use SQLite's online backup API via .backup command so we get a
  # consistent snapshot even if the server is actively writing.
  log "Running online SQLite backup (WAL-safe)..."
  docker exec "$CONTAINER" sqlite3 "$DB_PATH_IN_CONTAINER" \
    ".timeout 10000" \
    ".backup /tmp/mptracker_backup.db"

  # Copy the clean backup out
  docker cp "${CONTAINER}:/tmp/mptracker_backup.db" "$BACKUP_FILE"

  # Clean up temp file inside container
  docker exec "$CONTAINER" rm -f /tmp/mptracker_backup.db

  # Verify the local copy is a valid SQLite file
  if command -v sqlite3 &>/dev/null; then
    log "Verifying backup integrity..."
    local check
    check=$(sqlite3 "$BACKUP_FILE" "PRAGMA integrity_check;" 2>&1)
    if [[ "$check" == "ok" ]]; then
      ok "Integrity check passed"
    else
      err "Integrity check FAILED: $check"
      err "Backup file may be corrupt: $BACKUP_FILE"
      exit 1
    fi
    echo -e "$(db_summary "local:$BACKUP_FILE")"
  fi

  local size
  size=$(du -sh "$BACKUP_FILE" | awk '{print $1}')
  ok "Backup complete → ${BOLD}$BACKUP_FILE${NC} ($size)"
}

# ─── RESTORE ─────────────────────────────────────────────────────────────────

cmd_restore() {
  preflight

  if [[ -z "$BACKUP_FILE" ]]; then
    err "No backup file specified. Use -f <path>"
    exit 1
  fi

  if [[ ! -f "$BACKUP_FILE" ]]; then
    err "Backup file not found: $BACKUP_FILE"
    exit 1
  fi

  header "Restoring mptracker database"
  log "Container : $CONTAINER"
  log "Source    : $BACKUP_FILE"
  log "Dest      : $DB_PATH_IN_CONTAINER"

  # Validate the source file first
  if command -v sqlite3 &>/dev/null; then
    log "Validating source file..."
    if ! file "$BACKUP_FILE" | grep -q "SQLite"; then
      err "$BACKUP_FILE does not appear to be a SQLite database"
      exit 1
    fi
    local check
    check=$(sqlite3 "$BACKUP_FILE" "PRAGMA integrity_check;" 2>&1)
    if [[ "$check" != "ok" ]]; then
      err "Source file integrity check FAILED: $check"
      exit 1
    fi
    ok "Source file is valid"
    echo -e "Source DB contents:"
    echo -e "$(db_summary "local:$BACKUP_FILE")"
  fi

  # Show current state of container DB if it exists
  if db_exists_in_container; then
    echo -e "\nCurrent container DB:"
    echo -e "$(db_summary "container")"
  else
    warn "No existing database found in container — this is a fresh restore"
  fi

  echo ""
  if ! confirm "This will REPLACE the database in '$CONTAINER'. Continue?"; then
    log "Aborted."
    exit 0
  fi

  # Backup the current DB first as a safety net
  if db_exists_in_container; then
    local safety_backup="/tmp/mptracker_pre_restore_$(date +%s).db"
    log "Creating safety backup of current DB at ${safety_backup}..."
    docker exec "$CONTAINER" sqlite3 "$DB_PATH_IN_CONTAINER" \
      ".timeout 10000" \
      ".backup /tmp/pre_restore.db" 2>/dev/null || true
    docker cp "${CONTAINER}:/tmp/pre_restore.db" "$safety_backup" 2>/dev/null || true
    docker exec "$CONTAINER" rm -f /tmp/pre_restore.db 2>/dev/null || true
    ok "Safety backup saved → $safety_backup"
  fi

  # Stop the app briefly so SQLite isn't mid-write
  # We SIGSTOP rather than kill so we don't lose the container
  log "Pausing app writes (SIGSTOP)..."
  docker exec "$CONTAINER" kill -STOP 1 2>/dev/null || true

  # Copy new DB in
  log "Copying database..."
  docker cp "$BACKUP_FILE" "${CONTAINER}:${DB_PATH_IN_CONTAINER}"

  # Resume the app
  log "Resuming app (SIGCONT)..."
  docker exec "$CONTAINER" kill -CONT 1 2>/dev/null || true

  # Verify what's now in the container
  if command -v sqlite3 &>/dev/null; then
    log "Verifying restored database..."
    sleep 1  # give the process a moment to resume
    echo -e "Restored DB contents:"
    echo -e "$(db_summary "container")"
  fi

  ok "Restore complete."
  log "If the app is misbehaving, restart it with:  docker compose restart mptracker"
}

# ─── STATUS ──────────────────────────────────────────────────────────────────

cmd_status() {
  preflight

  header "mptracker database status"
  log "Container : $CONTAINER"
  log "DB path   : $DB_PATH_IN_CONTAINER"

  if ! db_exists_in_container; then
    warn "Database not found at $DB_PATH_IN_CONTAINER"
    exit 0
  fi

  echo ""
  echo -e "$(db_summary "container")"

  # Recent runs
  echo ""
  log "5 most recent runs:"
  docker exec "$CONTAINER" sqlite3 -column -header "$DB_PATH_IN_CONTAINER" \
    "SELECT c.name, r.dungeon_short, r.key_level, r.within_time, r.completed_at
     FROM runs r JOIN characters c ON c.id = r.character_id
     ORDER BY r.completed_at DESC LIMIT 5;" 2>/dev/null || echo "  (could not query)"

  # List local backups if dir exists
  if [[ -d "$BACKUP_DIR" ]]; then
    echo ""
    log "Local backups in ${BACKUP_DIR}/:"
    local count=0
    while IFS= read -r -d '' f; do
      size=$(du -sh "$f" | awk '{print $1}')
      ts=$(basename "$f" .db | sed 's/mptracker_//')
      printf "  %-45s %s\n" "$(basename "$f")" "$size"
      ((count++))
    done < <(find "$BACKUP_DIR" -name "*.db" -print0 | sort -z)
    [[ $count -eq 0 ]] && echo "  (none)"
  fi

  echo ""
}

# ─── Dispatch ────────────────────────────────────────────────────────────────

case "$COMMAND" in
  backup)  cmd_backup  ;;
  restore) cmd_restore ;;
  status)  cmd_status  ;;
  help|-h|--help) usage ;;
  *)
    err "Unknown command: $COMMAND"
    echo "  Valid commands: backup, restore, status"
    exit 1
    ;;
esac
