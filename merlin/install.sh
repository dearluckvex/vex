#!/bin/sh
# Vex Merlin Router Plugin Installer
# Supports AsusWRT-Merlin firmware (armv7 / aarch64)
#
# Usage: sh install.sh [arch]
#   arch: armv7 | aarch64 (auto-detected if not specified)

set -e

VEX_DIR="/jffs/addons/vex"
VEX_BIN="$VEX_DIR/vex-cli"
VEX_CONF="$VEX_DIR/config.yaml"
VEX_SCRIPTS="$VEX_DIR/scripts"
WWW_VEX="/www/ext/vex"
CGI_BIN="/www/cgi-bin"

CYAN='\033[0;36m'; GREEN='\033[0;32m'; RED='\033[0;31m'; NC='\033[0m'
info()  { printf "${CYAN}[Vex] %s${NC}\n" "$*"; }
ok()    { printf "${GREEN}[Vex] ✓ %s${NC}\n" "$*"; }
err()   { printf "${RED}[Vex] ✗ %s${NC}\n" "$*" >&2; exit 1; }

# ── Architecture detection ─────────────────────────────────────────────────
detect_arch() {
    local machine
    machine=$(uname -m)
    case "$machine" in
        armv7*|armhf)  echo "armv7" ;;
        aarch64|arm64) echo "aarch64" ;;
        *) err "Unsupported architecture: $machine. Use armv7 or aarch64." ;;
    esac
}

ARCH="${1:-$(detect_arch)}"
info "Installing Vex for architecture: $ARCH"

# ── Detect install source ──────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
if [ -f "$SCRIPT_DIR/bin/vex-cli-$ARCH" ]; then
    # Offline install from package
    BINARY="$SCRIPT_DIR/bin/vex-cli-$ARCH"
    info "Using bundled binary: $BINARY"
else
    err "Binary not found at $SCRIPT_DIR/bin/vex-cli-$ARCH\nPlease download the full merlin package from GitHub Releases."
fi

# ── Check Merlin environment ───────────────────────────────────────────────
[ -d /jffs ] || err "JFFS not mounted. Enable JFFS custom scripts in router settings."
[ -f /usr/sbin/nvram ] || err "NVRAM tool not found. Is this an AsusWRT-Merlin router?"

# ── Create directories ─────────────────────────────────────────────────────
info "Creating directories..."
mkdir -p "$VEX_DIR" "$VEX_SCRIPTS" "$WWW_VEX" /jffs/scripts

# ── Install binary ─────────────────────────────────────────────────────────
info "Installing vex-cli binary..."
cp "$BINARY" "$VEX_BIN"
chmod +x "$VEX_BIN"
ok "Binary installed at $VEX_BIN"

# ── Install scripts ────────────────────────────────────────────────────────
info "Installing control scripts..."
cp "$SCRIPT_DIR/scripts/vex.sh" "$VEX_SCRIPTS/vex.sh"
cp "$SCRIPT_DIR/scripts/iptables.sh" "$VEX_SCRIPTS/iptables.sh"
cp "$SCRIPT_DIR/scripts/dnsmasq.conf" "$VEX_SCRIPTS/dnsmasq.conf.template"
chmod +x "$VEX_SCRIPTS/vex.sh" "$VEX_SCRIPTS/iptables.sh"
ok "Control scripts installed"

# ── Install Web UI ─────────────────────────────────────────────────────────
info "Installing Web UI..."
cp "$SCRIPT_DIR/webui/pages/vex.asp" "$WWW_VEX/vex.asp"
cp "$SCRIPT_DIR/webui/cgi-bin/vex.cgi" "$CGI_BIN/vex.cgi"
chmod +x "$CGI_BIN/vex.cgi"
ok "Web UI installed"

# ── Generate default config ────────────────────────────────────────────────
if [ ! -f "$VEX_CONF" ]; then
    info "Generating default config..."
    cp "$SCRIPT_DIR/config.yaml.template" "$VEX_CONF"
    ok "Default config at $VEX_CONF"
fi

# ── Hook into Merlin startup scripts ──────────────────────────────────────
info "Hooking into Merlin startup scripts..."

# firewall-start: restore iptables rules after firewall reset
FIREWALL_SCRIPT="/jffs/scripts/firewall-start"
if ! grep -q "vex" "$FIREWALL_SCRIPT" 2>/dev/null; then
    printf '\n# Vex transparent proxy\n[ -x %s/scripts/iptables.sh ] && %s/scripts/iptables.sh start\n' \
        "$VEX_DIR" "$VEX_DIR" >> "$FIREWALL_SCRIPT"
    chmod +x "$FIREWALL_SCRIPT"
    ok "Hooked into firewall-start"
fi

# services-start: start vex on boot
SERVICES_START="/jffs/scripts/services-start"
if ! grep -q "vex" "$SERVICES_START" 2>/dev/null; then
    printf '\n# Vex proxy service\n[ -x %s/scripts/vex.sh ] && %s/scripts/vex.sh start\n' \
        "$VEX_DIR" "$VEX_DIR" >> "$SERVICES_START"
    chmod +x "$SERVICES_START"
    ok "Hooked into services-start"
fi

# service-event: handle restart events
SERVICE_EVENT="/jffs/scripts/service-event"
if ! grep -q "vex" "$SERVICE_EVENT" 2>/dev/null; then
    cat >> "$SERVICE_EVENT" << 'EOF'

# Vex service event handler
if [ "$1" = "restart" ] && [ "$2" = "vex" ]; then
    /jffs/addons/vex/scripts/vex.sh restart
fi
EOF
    chmod +x "$SERVICE_EVENT"
    ok "Hooked into service-event"
fi

# ── Done ───────────────────────────────────────────────────────────────────
echo ""
ok "Vex installed successfully!"
echo ""
echo "  Config file : $VEX_CONF"
echo "  Web UI      : http://router.asus.com/ext/vex/vex.asp"
echo ""
echo "  Start  : $VEX_SCRIPTS/vex.sh start"
echo "  Stop   : $VEX_SCRIPTS/vex.sh stop"
echo "  Status : $VEX_SCRIPTS/vex.sh status"
echo ""
echo "Edit $VEX_CONF to add your subscription or nodes, then start."
