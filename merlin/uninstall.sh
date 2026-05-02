#!/bin/sh
# Vex Merlin Router Plugin Uninstaller

set -e

VEX_DIR="/jffs/addons/vex"
WWW_VEX="/www/ext/vex"
CGI_BIN="/www/cgi-bin"

CYAN='\033[0;36m'; GREEN='\033[0;32m'; NC='\033[0m'
info() { printf "${CYAN}[Vex] %s${NC}\n" "$*"; }
ok()   { printf "${GREEN}[Vex] ✓ %s${NC}\n" "$*"; }

info "Stopping Vex service..."
[ -x "$VEX_DIR/scripts/vex.sh" ] && "$VEX_DIR/scripts/vex.sh" stop 2>/dev/null || true

info "Removing iptables rules..."
[ -x "$VEX_DIR/scripts/iptables.sh" ] && "$VEX_DIR/scripts/iptables.sh" stop 2>/dev/null || true

info "Removing Web UI..."
rm -rf "$WWW_VEX"
rm -f "$CGI_BIN/vex.cgi"

info "Removing startup hooks..."
for script in /jffs/scripts/firewall-start /jffs/scripts/services-start /jffs/scripts/service-event; do
    if [ -f "$script" ]; then
        sed -i '/# Vex/,+1d' "$script" 2>/dev/null || true
        sed -i '/vex/d' "$script" 2>/dev/null || true
    fi
done

info "Removing Vex files..."
rm -rf "$VEX_DIR"

ok "Vex uninstalled successfully."
