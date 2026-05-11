#!/bin/sh
# Vex Plugin Installer
# Supports:
#   1. koolcenter / koolshare software center (auto-detected)
#   2. Standard AsusWRT-Merlin firmware (armv7 / aarch64)
#
# Usage: sh install.sh [arch]           — offline install from bundled package
#        sh install.sh --online [arch]  — download latest binary from GitHub
#        sh install.sh --update [arch]  — update binary only (preserve config)
#   arch: armv7 | aarch64 (auto-detected if not specified)

# Do NOT use set -e — koolshare's base.sh changes error handling

# ── Common paths ──────────────────────────────────────────────────────────────
# Plugin data always lives under /jffs/addons/vex/ (both modes)
VEX_DIR="/jffs/addons/vex"
VEX_BIN="$VEX_DIR/vex-cli"
VEX_CONF="$VEX_DIR/config.yaml"
VEX_SCRIPTS="$VEX_DIR/scripts"
CGI_BIN="/www/cgi-bin"

# koolshare paths
KS_DIR="/koolshare"
KS_WEBS="$KS_DIR/webs"
KS_SCRIPTS="$KS_DIR/scripts"
KS_INIT="$KS_DIR/init.d"
MODULE="vex"   # must match the package directory name

# Standard Merlin path (used when NOT in koolshare mode)
WWW_VEX="/www/ext/vex"

# GitHub release config — set VEX_GITHUB_REPO env var or edit below
GITHUB_REPO="${VEX_GITHUB_REPO:-dearluckvex/vex}"

CYAN='\033[0;36m'; GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'
info()  { printf "${CYAN}[Vex] %s${NC}\n" "$*"; }
ok()    { printf "${GREEN}[Vex] ✓ %s${NC}\n" "$*"; }
warn()  { printf "${YELLOW}[Vex] ! %s${NC}\n" "$*"; }
err()   { printf "${RED}[Vex] ✗ %s${NC}\n" "$*" >&2; exit 1; }

# ── Detect koolshare environment ───────────────────────────────────────────────
KOOLSHARE=0
if [ -d "$KS_DIR" ] && [ -f "$KS_DIR/scripts/base.sh" ]; then
    # shellcheck source=/dev/null
    . "$KS_DIR/scripts/base.sh" 2>/dev/null || true
    KOOLSHARE=1
fi

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

# ── Environment check ─────────────────────────────────────────────────────────
[ -d /jffs ] || err "JFFS not mounted. Enable JFFS custom scripts in router settings."
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

# ── Shared helpers ─────────────────────────────────────────────────────────────
install_common_files() {
    # Binary, scripts, config — same in both modes
    info "Creating data directories..."
    mkdir -p "$VEX_DIR" "$VEX_SCRIPTS" "$CGI_BIN" /jffs/scripts /jffs/configs/dnsmasq.d

    if [ -f "$VEX_CONF" ]; then
        cp "$VEX_CONF" "$VEX_CONF.bak"
        ok "Existing config backed up to $VEX_CONF.bak"
    fi

    info "Installing vex-cli binary..."
    cp "$BINARY" "$VEX_BIN" && chmod +x "$VEX_BIN"
    ok "Binary installed: $("$VEX_BIN" --version 2>/dev/null || echo 'version unknown')"

    info "Installing control scripts..."
    cp "$SCRIPT_DIR/scripts/vex.sh"       "$VEX_SCRIPTS/vex.sh"
    cp "$SCRIPT_DIR/scripts/iptables.sh"  "$VEX_SCRIPTS/iptables.sh"
    cp "$SCRIPT_DIR/scripts/dnsmasq.conf" "$VEX_SCRIPTS/dnsmasq.conf.template"
    chmod +x "$VEX_SCRIPTS/vex.sh" "$VEX_SCRIPTS/iptables.sh"
    ok "Control scripts installed"

    info "Installing CGI backend..."
    # Store CGI in JFFS (persistent), symlink into /www/cgi-bin/ (tmpfs on most routers)
    cp "$SCRIPT_DIR/cgi-bin/vex.cgi" "$VEX_DIR/vex.cgi"
    chmod +x "$VEX_DIR/vex.cgi"
    mkdir -p "$CGI_BIN"
    ln -sf "$VEX_DIR/vex.cgi" "$CGI_BIN/vex.cgi" 2>/dev/null || \
        cp "$VEX_DIR/vex.cgi" "$CGI_BIN/vex.cgi" 2>/dev/null || \
        warn "Could not install CGI to $CGI_BIN — will retry on next start"
    ok "CGI backend installed"

    if [ ! -f "$VEX_CONF" ]; then
        info "Generating default config..."
        cp "$SCRIPT_DIR/config.yaml.template" "$VEX_CONF"
        ok "Default config at $VEX_CONF"
    else
        ok "Existing config preserved"
    fi
}

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

# ─────────────────────────────────────────────────────────────────────────────
# koolshare / koolcenter install path
# ─────────────────────────────────────────────────────────────────────────────
ks_install() {
    local VER
    VER=$(cat "${SCRIPT_DIR}/version" 2>/dev/null || echo "0.1.0")
    info "koolcenter 模式安装 Vex ${VER}..."

    install_common_files

    # koolshare Web UI: Module_vex.asp → /koolshare/webs/
    info "Installing koolshare Web UI..."
    mkdir -p "$KS_WEBS" "$KS_SCRIPTS" "$KS_INIT"
    cp "$SCRIPT_DIR/webs/Module_vex.asp" "$KS_WEBS/Module_vex.asp"
    ok "Web UI installed to $KS_WEBS/Module_vex.asp"

    # koolshare startup wrapper: delegates to our vex.sh
    cat > "$KS_SCRIPTS/${MODULE}_config.sh" << 'WRAPPER'
#!/bin/sh
case "$1" in
    start|"") /jffs/addons/vex/scripts/vex.sh start ;;
    stop)     /jffs/addons/vex/scripts/vex.sh stop  ;;
    restart)  /jffs/addons/vex/scripts/vex.sh restart ;;
    *)        /jffs/addons/vex/scripts/vex.sh "$@" ;;
esac
WRAPPER
    chmod +x "$KS_SCRIPTS/${MODULE}_config.sh"

    # Register with koolshare init.d for auto-start
    ln -sf "$KS_SCRIPTS/${MODULE}_config.sh" "$KS_INIT/S98${MODULE}.sh"
    ok "Registered koolshare startup hook"

    # Register plugin metadata in dbus (software center status)
    dbus set "${MODULE}_version"="${VER}"                        2>/dev/null || true
    dbus set "softcenter_module_${MODULE}_version"="${VER}"      2>/dev/null || true
    dbus set "softcenter_module_${MODULE}_install"="1"          2>/dev/null || true
    dbus set "softcenter_module_${MODULE}_name"="${MODULE}"      2>/dev/null || true
    dbus set "softcenter_module_${MODULE}_title"="Vex"          2>/dev/null || true
    dbus set "softcenter_module_${MODULE}_description"="透明代理（TUN/SOCKS5/HTTP）" 2>/dev/null || true
    ok "Plugin registered in dbus"

    [ -n "$TMP_BIN" ] && rm -f "$TMP_BIN"
    # koolshare installer expects temp dir cleanup
    rm -rf "/tmp/${MODULE}" "/tmp/${MODULE}.tar.gz" 2>/dev/null || true

    echo ""
    ok "Vex ${VER} 安装成功！"
    echo ""
    echo "  配置文件 : $VEX_CONF"
    echo "  Web UI   : 软件中心 → Vex"
    echo ""
    echo "  启动: $VEX_SCRIPTS/vex.sh start"
    echo "  停止: $VEX_SCRIPTS/vex.sh stop"
    echo ""
}

# ─────────────────────────────────────────────────────────────────────────────
# Standard AsusWRT-Merlin install path
# ─────────────────────────────────────────────────────────────────────────────
merlin_install() {
    info "Standard Merlin 模式安装..."
    mkdir -p "$WWW_VEX"
    install_common_files

    info "Installing Web UI page..."
    cp "$SCRIPT_DIR/webs/Module_vex.asp" "$WWW_VEX/vex.asp"
    ok "Web UI installed to $WWW_VEX/vex.asp"

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

    [ -n "$TMP_BIN" ] && rm -f "$TMP_BIN"

    # Post-install validation
    info "Validating installation..."
    errors=0
    [ -x "$VEX_BIN" ]                  || { warn "Binary not executable";       errors=$((errors+1)); }
    [ -f "$VEX_CONF" ]                 || { warn "Config file missing";         errors=$((errors+1)); }
    [ -x "$VEX_SCRIPTS/vex.sh" ]      || { warn "vex.sh not executable";       errors=$((errors+1)); }
    [ -x "$VEX_SCRIPTS/iptables.sh" ] || { warn "iptables.sh not executable";  errors=$((errors+1)); }
    [ -f "$WWW_VEX/vex.asp" ]         || { warn "Web UI page missing";         errors=$((errors+1)); }
    [ -x "$VEX_DIR/vex.cgi" ]         || { warn "CGI script not executable";   errors=$((errors+1)); }
    [ "$errors" -eq 0 ] && ok "All files installed correctly" \
                         || warn "$errors validation warning(s) — check output above"

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
}

# ── Dispatch ──────────────────────────────────────────────────────────────────
if [ "$KOOLSHARE" = "1" ]; then
    ks_install
else
    merlin_install
fi
