#!/bin/sh
# Vex Merlin Router Plugin Uninstaller

VEX_DIR="/jffs/addons/vex"
WWW_VEX="/www/ext/vex"
CGI_BIN="/www/cgi-bin"

CYAN='\033[0;36m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
info() { printf "${CYAN}[Vex] %s${NC}\n" "$*"; }
ok()   { printf "${GREEN}[Vex] ✓ %s${NC}\n" "$*"; }
warn() { printf "${YELLOW}[Vex] ! %s${NC}\n" "$*"; }

# ── Confirm ───────────────────────────────────────────────────────────────────
if [ "$1" != "--force" ]; then
    printf "This will completely remove Vex. Continue? [y/N] "
    read -r ans
    case "$ans" in y|Y) ;; *) echo "Aborted."; exit 0 ;; esac
fi

# ── Stop service ──────────────────────────────────────────────────────────────
info "Stopping Vex service..."
[ -x "$VEX_DIR/scripts/vex.sh" ] && "$VEX_DIR/scripts/vex.sh" stop 2>/dev/null || true

# Kill any leftover vex-cli processes
if [ -f "$VEX_DIR/vex.pid" ]; then
    pid=$(cat "$VEX_DIR/vex.pid" 2>/dev/null)
    [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
fi

# ── Remove iptables rules ──────────────────────────────────────────────────────
info "Removing iptables rules..."
[ -x "$VEX_DIR/scripts/iptables.sh" ] && "$VEX_DIR/scripts/iptables.sh" stop 2>/dev/null || true

# ── Remove cron jobs ──────────────────────────────────────────────────────────
info "Removing cron jobs..."
cru d VexSubUpdate 2>/dev/null || true
cru d VexWatchdog  2>/dev/null || true
ok "Cron jobs removed"

# ── Remove Web UI ─────────────────────────────────────────────────────────────
info "Removing Web UI..."
rm -rf "$WWW_VEX"
rm -f  "$CGI_BIN/vex.cgi"
ok "Web UI removed"

# ── Remove startup hooks ───────────────────────────────────────────────────────
info "Removing startup hooks..."
for script in \
    /jffs/scripts/firewall-start \
    /jffs/scripts/services-start \
    /jffs/scripts/service-event; do
    [ -f "$script" ] || continue
    # Remove specific lines: our comment markers and the exact vex command lines
    sed -i '/^# Vex firewall$/d'        "$script" 2>/dev/null || true
    sed -i '/^# Vex services$/d'        "$script" 2>/dev/null || true
    sed -i '/^# Vex service-event$/d'   "$script" 2>/dev/null || true
    # Remove the if/fi block for service-event (3 specific lines)
    sed -i '/^if \[ "\$1" = "restart" \] && \[ "\$2" = "vex" \]; then$/d' "$script" 2>/dev/null || true
    sed -i '/^    \/jffs\/addons\/vex\/scripts\/vex\.sh restart$/d'        "$script" 2>/dev/null || true
    sed -i '/^fi$/d' "$script" 2>/dev/null || true
    # Remove lines referencing the vex addon path
    sed -i '\|/jffs/addons/vex/|d' "$script" 2>/dev/null || true
    ok "Cleaned $script"
done

# ── Remove dnsmasq config ──────────────────────────────────────────────────────
info "Removing dnsmasq config..."
rm -f /jffs/configs/dnsmasq.d/vex.conf
service restart_dnsmasq 2>/dev/null || true
ok "dnsmasq config removed"

# ── Remove Vex files ───────────────────────────────────────────────────────────
info "Removing Vex files..."
rm -rf "$VEX_DIR"
ok "Vex files removed"

echo ""
ok "Vex uninstalled successfully."
