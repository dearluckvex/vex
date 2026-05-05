#!/bin/sh
# Vex Merlin Router Plugin Installer
# Supports AsusWRT-Merlin firmware (armv7 / aarch64)
#
# Usage: sh install.sh [arch]           — offline install from bundled package
#        sh install.sh --online [arch]  — download latest binary from GitHub
#        sh install.sh --update [arch]  — update binary only (preserve config)
#   arch: armv7 | aarch64 (auto-detected if not specified)

set -e

VEX_DIR="/jffs/addons/vex"
VEX_BIN="$VEX_DIR/vex-cli"
VEX_CONF="$VEX_DIR/config.yaml"
VEX_SCRIPTS="$VEX_DIR/scripts"
WWW_VEX="/www/ext/vex"
CGI_BIN="/www/cgi-bin"

# GitHub release config — set VEX_GITHUB_REPO env var or edit below
GITHUB_REPO="${VEX_GITHUB_REPO:-YOUR_USERNAME/vex}"

CYAN='\033[0;36m'; GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'
info()  { printf "${CYAN}[Vex] %s${NC}\n" "$*"; }
ok()    { printf "${GREEN}[Vex] ✓ %s${NC}\n" "$*"; }
warn()  { printf "${YELLOW}[Vex] ! %s${NC}\n" "$*"; }
err()   { printf "${RED}[Vex] ✗ %s${NC}\n" "$*" >&2; exit 1; }

# ── Parse arguments ────────────────────────────────────────────────────────────
ONLINE=false
UPDATE_ONLY=false
ARCH_ARG=""
for arg in "$@"; do
    case "$arg" in
        --online) ONLINE=true ;;
        --update) UPDATE_ONLY=true ;;
        armv7|aarch64) ARCH_ARG="$arg" ;;
    esac
done

# ── Architecture detection ─────────────────────────────────────────────────────
detect_arch() {
    local machine
    machine=$(uname -m)
    case "$machine" in
        armv7*|armhf)  echo "armv7" ;;
        aarch64|arm64) echo "aarch64" ;;
        *) err "Unsupported architecture: $machine. Use armv7 or aarch64." ;;
    esac
}

ARCH="${ARCH_ARG:-$(detect_arch)}"
info "Target architecture: $ARCH"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ── Dependency checks ──────────────────────────────────────────────────────────
check_deps() {
    local missing=""
    for cmd in sed grep base64 iptables ip service nvram; do
        command -v "$cmd" >/dev/null 2>&1 || missing="$missing $cmd"
    done
    [ -n "$missing" ] && warn "Missing tools:$missing — some features may not work"
}

# ── Online download ────────────────────────────────────────────────────────────
download_binary() {
    local arch="$1" dst="$2"
    local base_url="https://github.com/$GITHUB_REPO/releases/latest/download"
    local filename="vex-cli-linux-$arch"
    info "Downloading $filename from GitHub Releases..."
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 --connect-timeout 30 -o "$dst" "$base_url/$filename"
    elif command -v wget >/dev/null 2>&1; then
        wget -q --tries=3 --timeout=30 -O "$dst" "$base_url/$filename"
    else
        err "Neither curl nor wget found. Cannot download binary."
    fi
}

# ── Resolve binary source ──────────────────────────────────────────────────────
TMP_BIN=""
if [ "$ONLINE" = "true" ]; then
    TMP_BIN="/tmp/vex-cli-$ARCH.$$"
    download_binary "$ARCH" "$TMP_BIN"
    [ -s "$TMP_BIN" ] || err "Download failed or empty. Check network / GITHUB_REPO setting."
    BINARY="$TMP_BIN"
elif [ -f "$SCRIPT_DIR/bin/vex-cli-$ARCH" ]; then
    BINARY="$SCRIPT_DIR/bin/vex-cli-$ARCH"
    info "Using bundled binary: $BINARY"
else
    info "Local binary not found — trying online download..."
    TMP_BIN="/tmp/vex-cli-$ARCH.$$"
    download_binary "$ARCH" "$TMP_BIN" || true
    if [ -s "$TMP_BIN" ]; then
        BINARY="$TMP_BIN"
    else
        rm -f "$TMP_BIN"; TMP_BIN=""
        err "Binary not found at $SCRIPT_DIR/bin/vex-cli-$ARCH and download failed.\nSet VEX_GITHUB_REPO=owner/repo or download the binary manually."
    fi
fi

# ── Check Merlin environment ───────────────────────────────────────────────────
[ -d /jffs ] || err "JFFS not mounted. Enable JFFS custom scripts in router settings."
[ -f /usr/sbin/nvram ] || err "NVRAM tool not found. Is this an AsusWRT-Merlin router?"
check_deps

# ── Update-only mode ───────────────────────────────────────────────────────────
if [ "$UPDATE_ONLY" = "true" ]; then
    info "Update mode: replacing binary only (config preserved)..."
    [ -d "$VEX_DIR" ] || err "Vex is not installed. Run without --update first."
    WAS_RUNNING=false
    [ -f "$VEX_DIR/vex.pid" ] && kill -0 "$(cat "$VEX_DIR/vex.pid")" 2>/dev/null && WAS_RUNNING=true
    [ "$WAS_RUNNING" = "true" ] && "$VEX_SCRIPTS/vex.sh" stop 2>/dev/null || true
    cp "$BINARY" "$VEX_BIN" && chmod +x "$VEX_BIN"
    ok "Binary updated: $("$VEX_BIN" --version 2>/dev/null || echo 'version unknown')"
    [ "$WAS_RUNNING" = "true" ] && "$VEX_SCRIPTS/vex.sh" start
    [ -n "$TMP_BIN" ] && rm -f "$TMP_BIN"
    exit 0
fi

# ── Create directories ─────────────────────────────────────────────────────────
info "Creating directories..."
mkdir -p "$VEX_DIR" "$VEX_SCRIPTS" "$WWW_VEX" /jffs/scripts /jffs/configs/dnsmasq.d

# ── Backup existing config ─────────────────────────────────────────────────────
if [ -f "$VEX_CONF" ]; then
    cp "$VEX_CONF" "$VEX_CONF.bak"
    ok "Existing config backed up to $VEX_CONF.bak"
fi

# ── Install binary ─────────────────────────────────────────────────────────────
info "Installing vex-cli binary..."
cp "$BINARY" "$VEX_BIN" && chmod +x "$VEX_BIN"
ok "Binary installed: $("$VEX_BIN" --version 2>/dev/null || echo 'version unknown')"

# ── Install scripts ────────────────────────────────────────────────────────────
info "Installing control scripts..."
cp "$SCRIPT_DIR/scripts/vex.sh"       "$VEX_SCRIPTS/vex.sh"
cp "$SCRIPT_DIR/scripts/iptables.sh"  "$VEX_SCRIPTS/iptables.sh"
cp "$SCRIPT_DIR/scripts/dnsmasq.conf" "$VEX_SCRIPTS/dnsmasq.conf.template"
chmod +x "$VEX_SCRIPTS/vex.sh" "$VEX_SCRIPTS/iptables.sh"
ok "Control scripts installed"

# ── Install Web UI ─────────────────────────────────────────────────────────────
info "Installing Web UI..."
cp "$SCRIPT_DIR/webui/pages/vex.asp"   "$WWW_VEX/vex.asp"
cp "$SCRIPT_DIR/webui/cgi-bin/vex.cgi" "$CGI_BIN/vex.cgi"
chmod +x "$CGI_BIN/vex.cgi"
ok "Web UI installed"

# ── Generate default config ────────────────────────────────────────────────────
if [ ! -f "$VEX_CONF" ]; then
    info "Generating default config..."
    cp "$SCRIPT_DIR/config.yaml.template" "$VEX_CONF"
    ok "Default config at $VEX_CONF"
else
    ok "Existing config preserved"
fi

# ── Hook into Merlin startup scripts ──────────────────────────────────────────
hook_script() {
    local file="$1" marker="$2" content="$3"
    [ -f "$file" ] || { printf '#!/bin/sh\n' > "$file"; chmod +x "$file"; }
    if ! grep -q "$marker" "$file" 2>/dev/null; then
        printf '\n%s' "$content" >> "$file"
        chmod +x "$file"
        ok "Hooked into $(basename "$file")"
    else
        ok "Already hooked into $(basename "$file")"
    fi
}

info "Hooking into Merlin startup scripts..."

hook_script "/jffs/scripts/firewall-start" "# Vex firewall" \
"# Vex firewall
[ -x $VEX_DIR/scripts/iptables.sh ] && $VEX_DIR/scripts/iptables.sh start
"

hook_script "/jffs/scripts/services-start" "# Vex services" \
"# Vex services
[ -x $VEX_DIR/scripts/vex.sh ] && $VEX_DIR/scripts/vex.sh start
"

SERVICE_EVENT="/jffs/scripts/service-event"
[ -f "$SERVICE_EVENT" ] || { printf '#!/bin/sh\n' > "$SERVICE_EVENT"; chmod +x "$SERVICE_EVENT"; }
if ! grep -q "# Vex service-event" "$SERVICE_EVENT" 2>/dev/null; then
    cat >> "$SERVICE_EVENT" << 'EOF'

# Vex service-event
if [ "$1" = "restart" ] && [ "$2" = "vex" ]; then
    /jffs/addons/vex/scripts/vex.sh restart
fi
EOF
    chmod +x "$SERVICE_EVENT"
    ok "Hooked into service-event"
fi

# ── Cleanup temp binary ────────────────────────────────────────────────────────
[ -n "$TMP_BIN" ] && rm -f "$TMP_BIN"

# ── Post-install validation ────────────────────────────────────────────────────
info "Validating installation..."
errors=0
[ -x "$VEX_BIN" ]                 || { warn "Binary not executable";       errors=$((errors+1)); }
[ -f "$VEX_CONF" ]                || { warn "Config file missing";         errors=$((errors+1)); }
[ -x "$VEX_SCRIPTS/vex.sh" ]     || { warn "vex.sh not executable";       errors=$((errors+1)); }
[ -x "$VEX_SCRIPTS/iptables.sh" ] || { warn "iptables.sh not executable"; errors=$((errors+1)); }
[ -f "$WWW_VEX/vex.asp" ]        || { warn "Web UI page missing";         errors=$((errors+1)); }
[ -x "$CGI_BIN/vex.cgi" ]        || { warn "CGI script not executable";   errors=$((errors+1)); }
[ "$errors" -eq 0 ] && ok "All files installed correctly" \
                     || warn "$errors validation warning(s) — check output above"

# ── Done ───────────────────────────────────────────────────────────────────────
echo ""
ok "Vex installed successfully!"
echo ""
echo "  Config file : $VEX_CONF"
echo "  Web UI      : http://router.asus.com/ext/vex/vex.asp"
echo ""
echo "  Start  : $VEX_SCRIPTS/vex.sh start"
echo "  Stop   : $VEX_SCRIPTS/vex.sh stop"
echo "  Status : $VEX_SCRIPTS/vex.sh status"
echo "  Update : sh $0 --update"
echo ""
echo "Edit $VEX_CONF to add your subscription or nodes, then start."
