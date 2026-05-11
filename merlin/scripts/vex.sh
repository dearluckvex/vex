#!/bin/sh
# Vex proxy service control script for AsusWRT-Merlin
#
# Usage: vex.sh {start|stop|restart|reload|status|log|update-subs|watchdog}

VEX_DIR="/jffs/addons/vex"
VEX_BIN="$VEX_DIR/vex-cli"
VEX_CONF="$VEX_DIR/config.yaml"
VEX_PID="$VEX_DIR/vex.pid"
VEX_LOG="$VEX_DIR/vex.log"
VEX_LOCK="$VEX_DIR/vex.lock"
LOG_MAX_BYTES=524288   # 512 KB

CYAN='\033[0;36m'; GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'
info()  { printf "${CYAN}[Vex] %s${NC}\n" "$*"; }
ok()    { printf "${GREEN}[Vex] ✓ %s${NC}\n" "$*"; }
warn()  { printf "${YELLOW}[Vex] ! %s${NC}\n" "$*"; }
err()   { printf "${RED}[Vex] ✗ %s${NC}\n" "$*"; }

is_running() {
    [ -f "$VEX_PID" ] && kill -0 "$(cat "$VEX_PID")" 2>/dev/null
}

# ── Lock ─────────────────────────────────────────────────────────────────────
acquire_lock() {
    local wait=0
    while [ -f "$VEX_LOCK" ] && [ "$wait" -lt 15 ]; do
        sleep 1; wait=$((wait + 1))
    done
    if [ -f "$VEX_LOCK" ]; then
        err "Another vex operation is in progress (lock: $VEX_LOCK)"
        return 1
    fi
    echo $$ > "$VEX_LOCK"
}

release_lock() { rm -f "$VEX_LOCK"; }

# ── Log rotation ──────────────────────────────────────────────────────────────
rotate_log() {
    [ -f "$VEX_LOG" ] || return
    local size
    size=$(wc -c < "$VEX_LOG" 2>/dev/null || echo 0)
    if [ "$size" -gt "$LOG_MAX_BYTES" ]; then
        tail -c "$((LOG_MAX_BYTES / 2))" "$VEX_LOG" > "$VEX_LOG.tmp" \
            && mv "$VEX_LOG.tmp" "$VEX_LOG"
        info "Log rotated (was ${size} bytes)"
    fi
}

# ── Cron integration ──────────────────────────────────────────────────────────
setup_cron() {
    # Subscription auto-update daily at 04:00
    cru a VexSubUpdate "0 4 * * * $VEX_DIR/scripts/vex.sh update-subs" 2>/dev/null || true
    # Watchdog every 5 minutes
    cru a VexWatchdog "*/5 * * * * $VEX_DIR/scripts/vex.sh watchdog" 2>/dev/null || true
}

remove_cron() {
    cru d VexSubUpdate 2>/dev/null || true
    cru d VexWatchdog  2>/dev/null || true
}

# ── DNS config ────────────────────────────────────────────────────────────────
get_config_value() {
    grep "^${1}:" "$VEX_CONF" 2>/dev/null | awk '{print $2}' | head -1 | tr -d '"'
}

update_dnsmasq() {
    local action="$1"
    local DNSMASQ_CONF_DIR="/jffs/configs/dnsmasq.d"
    local VEX_DNSMASQ="$DNSMASQ_CONF_DIR/vex.conf"
    local TEMPLATE="$VEX_DIR/scripts/dnsmasq.conf.template"

    mkdir -p "$DNSMASQ_CONF_DIR"

    if [ "$action" = "start" ] && [ -f "$TEMPLATE" ]; then
        local dns_port router_ip dns_server
        dns_port=$(get_config_value dns_port)
        dns_port=${dns_port:-5300}
        router_ip=$(nvram get lan_ipaddr 2>/dev/null || echo "192.168.1.1")
        dns_server=$(get_config_value dns_direct_server)
        dns_server=${dns_server:-114.114.114.114}
        sed -e "s/__DNS_PORT__/$dns_port/g" \
            -e "s/__ROUTER_IP__/$router_ip/g" \
            "$TEMPLATE" > "$VEX_DNSMASQ"
        # Append dns_direct_domains from config.yaml as dnsmasq server entries
        local domain_count=0
        awk -v dns="$dns_server" '
            /^dns_direct_domains:/ { in_d=1; next }
            in_d && /^[a-zA-Z_]/ { in_d=0 }
            in_d && /^[[:space:]]*- / {
                sub(/^[[:space:]]*- /, ""); gsub(/["'"'"'[:space:]]/, "")
                if (length > 0) { printf "server=/%s/%s\n", $0, dns; count++ }
            }
            END { exit (count > 0 ? 0 : 1) }
        ' "$VEX_CONF" >> "$VEX_DNSMASQ" && \
            domain_count=$(awk '/^dns_direct_domains:/{in_d=1;next} in_d&&/^[a-zA-Z_]/{in_d=0} in_d&&/^[[:space:]]*- /{c++} END{print c+0}' "$VEX_CONF")
        service restart_dnsmasq 2>/dev/null || true
        ok "dnsmasq rules applied (DNS port: $dns_port, ${domain_count:-0} direct domains → $dns_server)"
    elif [ "$action" = "stop" ]; then
        rm -f "$VEX_DNSMASQ"
        service restart_dnsmasq 2>/dev/null || true
        ok "dnsmasq rules removed"
    fi
}

# ── Start ─────────────────────────────────────────────────────────────────────
do_start() {
    if is_running; then
        warn "Vex is already running (PID $(cat "$VEX_PID"))"
        return 0
    fi

    [ -x "$VEX_BIN" ]  || { err "Binary not found: $VEX_BIN"; return 1; }
    [ -f "$VEX_CONF" ] || { err "Config not found: $VEX_CONF"; return 1; }

    # Restore CGI symlink (tmpfs /www/cgi-bin loses files on reboot)
    local cgi_src="$VEX_DIR/vex.cgi"
    local cgi_dst="/www/cgi-bin/vex.cgi"
    if [ -f "$cgi_src" ] && [ ! -e "$cgi_dst" ]; then
        mkdir -p "/www/cgi-bin"
        ln -sf "$cgi_src" "$cgi_dst" 2>/dev/null || cp "$cgi_src" "$cgi_dst" 2>/dev/null || true
    fi

    rotate_log
    info "Starting Vex..."

    # Respect the mode setting: only pass --tun when mode is tun (default)
    local mode
    mode=$(get_config_value mode)
    case "${mode:-tun}" in
        tun)    "$VEX_BIN" "$VEX_CONF" --tun >> "$VEX_LOG" 2>&1 & ;;
        *)      "$VEX_BIN" "$VEX_CONF"       >> "$VEX_LOG" 2>&1 & ;;
    esac
    echo $! > "$VEX_PID"
    sleep 1

    if is_running; then
        ok "Vex started (PID $(cat "$VEX_PID"))"
        [ -x "$VEX_DIR/scripts/iptables.sh" ] && "$VEX_DIR/scripts/iptables.sh" start >> "$VEX_LOG" 2>&1
        update_dnsmasq start
        setup_cron
    else
        err "Vex failed to start. Check log: $VEX_LOG"
        rm -f "$VEX_PID"
        return 1
    fi
}

# ── Stop ──────────────────────────────────────────────────────────────────────
do_stop() {
    if ! is_running; then
        warn "Vex is not running"
        return 0
    fi

    info "Stopping Vex..."
    local pid
    pid=$(cat "$VEX_PID")
    kill "$pid" 2>/dev/null
    local wait=0
    while kill -0 "$pid" 2>/dev/null && [ "$wait" -lt 8 ]; do
        sleep 1; wait=$((wait + 1))
    done
    kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null || true
    rm -f "$VEX_PID"

    [ -x "$VEX_DIR/scripts/iptables.sh" ] && "$VEX_DIR/scripts/iptables.sh" stop >> "$VEX_LOG" 2>&1
    update_dnsmasq stop
    remove_cron

    ok "Vex stopped"
}

# ── Status ────────────────────────────────────────────────────────────────────
do_status() {
    if is_running; then
        local pid socks_port http_port active_node mode
        pid=$(cat "$VEX_PID")
        socks_port=$(get_config_value socks_port)
        http_port=$(get_config_value http_port)
        active_node=$(get_config_value active_node)
        mode=$(get_config_value mode)
        ok "Vex is running (PID $pid)"
        echo "  Mode   : ${mode:-tun}"
        echo "  SOCKS5 : 0.0.0.0:${socks_port:-1080}"
        echo "  HTTP   : 0.0.0.0:${http_port:-1087}"
        echo "  Node   : ${active_node:-0}"
        # DNS status
        [ -f "/jffs/configs/dnsmasq.d/vex.conf" ] \
            && echo "  DNS    : active" || echo "  DNS    : inactive"
        # Firewall status
        iptables -t nat -L VEX_PREROUTING -n 2>/dev/null | grep -q REDIRECT \
            && echo "  FW     : active" || echo "  FW     : inactive"
    else
        info "Vex is stopped"
    fi
}

# ── Update subscriptions ──────────────────────────────────────────────────────
do_update_subs() {
    info "Updating subscriptions (restart to re-fetch)..."
    if is_running; then
        do_stop
        sleep 1
        do_start
    else
        do_start
    fi
    ok "Subscription update triggered"
}

# ── Watchdog ──────────────────────────────────────────────────────────────────
do_watchdog() {
    if ! is_running; then
        warn "Watchdog: Vex not running — restarting..."
        do_start >> "$VEX_LOG" 2>&1
    fi
}

# ── Dispatch ──────────────────────────────────────────────────────────────────
case "$1" in
    start)
        acquire_lock && { do_start; release_lock; } || true
        ;;
    stop)
        acquire_lock && { do_stop; release_lock; } || true
        ;;
    restart)
        acquire_lock && { do_stop; sleep 1; do_start; release_lock; } || true
        ;;
    reload)
        acquire_lock || exit 1
        if is_running; then
            info "Reloading Vex config..."
            do_stop; sleep 1; do_start
        else
            do_start
        fi
        release_lock
        ;;
    status)  do_status ;;
    log)     tail -100 "$VEX_LOG" 2>/dev/null || info "No log file found" ;;
    update-subs)
        acquire_lock && { do_update_subs; release_lock; } || true
        ;;
    watchdog) do_watchdog ;;
    *)
        echo "Usage: $0 {start|stop|restart|reload|status|log|update-subs|watchdog}"
        exit 1
        ;;
esac
