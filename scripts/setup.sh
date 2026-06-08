#!/usr/bin/env bash
# =============================================================================
# setup.sh — install and configure the mplus-tracker update timer
#
# Run as root on the host machine that runs the mplus-tracker Docker container.
# Idempotent — safe to re-run to update config or script changes.
#
# Usage:
#   sudo bash scripts/setup.sh
# =============================================================================

set -euo pipefail

# ── Colours ───────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
CYAN='\033[0;36m'; BOLD='\033[1m'; RESET='\033[0m'

info()    { echo -e "${CYAN}[INFO]${RESET}  $*"; }
success() { echo -e "${GREEN}[OK]${RESET}    $*"; }
warn()    { echo -e "${YELLOW}[WARN]${RESET}  $*"; }
die()     { echo -e "${RED}[ERROR]${RESET} $*" >&2; exit 1; }
header()  { echo -e "\n${BOLD}${CYAN}── $* ──${RESET}"; }

# ── Must run as root ──────────────────────────────────────────────────────────
[[ $EUID -eq 0 ]] || die "This script must be run as root. Use: sudo bash scripts/setup.sh"

# ── Locate project root (script is in scripts/) ───────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# ── Paths ─────────────────────────────────────────────────────────────────────
INSTALL_BIN="/usr/local/bin/mplus-update.sh"
SYSTEMD_DIR="/etc/systemd/system"
CONF_DIR="/etc/mplus-tracker"
CONF_FILE="$CONF_DIR/update.conf"
SERVICE_USER="mplus-tracker"

# ── Dependency check ──────────────────────────────────────────────────────────
header "Checking dependencies"

for cmd in curl systemctl; do
  if command -v "$cmd" &>/dev/null; then
    success "$cmd found"
  else
    die "$cmd is required but not installed."
  fi
done

# ── Create system user ────────────────────────────────────────────────────────
header "System user"

if id "$SERVICE_USER" &>/dev/null; then
  success "User '$SERVICE_USER' already exists"
else
  useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
  success "Created system user '$SERVICE_USER'"
fi

# ── Install the update script ─────────────────────────────────────────────────
header "Installing update script"

cp "$SCRIPT_DIR/mplus-update.sh" "$INSTALL_BIN"
chmod 755 "$INSTALL_BIN"
success "Installed $INSTALL_BIN"

# ── Create config directory and file ─────────────────────────────────────────
header "Configuration"

mkdir -p "$CONF_DIR"
chmod 750 "$CONF_DIR"

if [[ -f "$CONF_FILE" ]]; then
  warn "Config already exists at $CONF_FILE — not overwriting."
  warn "Edit it manually if you need to update the token or URL."
else
  cp "$SCRIPT_DIR/update.conf.template" "$CONF_FILE"
  chmod 640 "$CONF_FILE"
  chown root:"$SERVICE_USER" "$CONF_FILE"
  success "Created config at $CONF_FILE"
  echo ""
  echo -e "  ${YELLOW}ACTION REQUIRED:${RESET} Edit the config file before starting the timer:"
  echo -e "  ${BOLD}sudo nano $CONF_FILE${RESET}"
  echo ""
fi

# ── Install systemd units ─────────────────────────────────────────────────────
header "Installing systemd units"

cp "$SCRIPT_DIR/mplus-update.service" "$SYSTEMD_DIR/mplus-update.service"
cp "$SCRIPT_DIR/mplus-update.timer"   "$SYSTEMD_DIR/mplus-update.timer"
success "Copied mplus-update.service and mplus-update.timer to $SYSTEMD_DIR"

systemctl daemon-reload
success "systemd daemon reloaded"

# ── Enable and start timer ────────────────────────────────────────────────────
header "Enabling timer"

systemctl enable mplus-update.timer
systemctl start  mplus-update.timer
success "mplus-update.timer enabled and started"

# ── Summary ───────────────────────────────────────────────────────────────────
header "Setup complete"

echo ""
echo -e "  ${BOLD}Timer status:${RESET}"
systemctl status mplus-update.timer --no-pager -l | sed 's/^/    /'
echo ""
echo -e "  ${BOLD}Next steps:${RESET}"
echo ""

# Check if config still has the placeholder token
if grep -q "REPLACE_WITH_YOUR_API_TOKEN" "$CONF_FILE" 2>/dev/null; then
  echo -e "  ${RED}1. Edit the config file (token not set yet):${RESET}"
  echo -e "     ${BOLD}sudo nano $CONF_FILE${RESET}"
  echo ""
  echo -e "  ${YELLOW}2. Run a manual test after editing:${RESET}"
else
  echo -e "  ${GREEN}1. Config looks set. Run a manual test:${RESET}"
fi

echo -e "     ${BOLD}sudo systemctl start mplus-update.service${RESET}"
echo -e "     ${BOLD}sudo journalctl -u mplus-update -f${RESET}"
echo ""
echo -e "  ${CYAN}Useful commands:${RESET}"
echo -e "     View timer schedule:   ${BOLD}systemctl list-timers mplus-update.timer${RESET}"
echo -e "     View recent logs:      ${BOLD}journalctl -u mplus-update --since '24h ago'${RESET}"
echo -e "     Disable timer:         ${BOLD}sudo systemctl disable --now mplus-update.timer${RESET}"
echo -e "     Update script only:    ${BOLD}sudo bash scripts/setup.sh${RESET}  (re-run is safe)"
echo ""
