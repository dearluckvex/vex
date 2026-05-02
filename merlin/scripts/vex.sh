#!/bin/sh
# Vex proxy service control script for AsusWRT-Merlin
#
# Usage: vex.sh {start|stop|restart|status|reload}

VEX_DIR="/jffs/addons/vex"
VEX_BIN="$VEX_DIR/vex-cli"
VEX_CONF="$VEX_DIR/config.yaml"
VEX_PID="$VEX_DIR/vex.pid"
VEX_LOG="$VEX_DIR/vex.log"

CYAN='\033[0;36m'; GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'
info()  { printf "${CYAN}[Vex] %s${NC}\n" "$*"; }
ok()    { printf "${GREEN}[Vex] ✓ %s${NC}\n" "$*"; }
warn()  { printf "${YELLOW}[Vex] ! %s${NC}\n" "$*"; }
err()   { printf "${RED}[Vex] ✗ %s${NC}\n" "$*"; }

is_running() {
    [ -f "$VEX_PID" ] && kill -0 "$(cat "$VEX_PID")" 2>/dev/null
}

do_start() {
    if is_running; then
        warn "Vex is already running (PID $(cat "$VEX_PID"))"
        return 0
    fi

    [ -x "$VEX_BIN" ] || { err "Binary not found: $VEX_BIN"; return 1; }
    [ -f "$VEX_CONF" ] || { err "Config not found: $VEX_CONF"; return 1; }

    info "Starting Vex..."
    "$VEX_BIN" "$VEX_CONF" >> "$VEX_LOG" 2>&1 &
    echo $! > "$VEX_PID"
    sleep 1

    if is_running; then
        ok "Vex started (PID $(cat "$VEX_PID"))"
        # Apply iptables rules for transparent proxy
        [ -x "$VEX_DIR/scripts/iptables.sh" ] && "$VEX_DIR/scripts/iptables.sh" start
        # Apply dnsmasq rules
        update_dnsmasq start
    else
        err "Vex failed to start. Check log: $VEX_LOG"
        return 1
    fi
}

do_stop() {
    if ! is_running; then
        warn "Vex is not running"
        return 0
    fi

    info "Stopping Vex..."
    local pid
    pid=$(cat "$VEX_PID")
    kill "$pid" 2>/dev/null
    sleep 2
    kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null || true
    rm -f "$VEX_PID"

    # Remove iptables rules
    [ -x "$VEX_DIR/scripts/iptables.sh" ] && "$VEX_DIR/scripts/iptables.sh" stop
    # Remove dnsmasq rules
    update_dnsmasq stop

    ok "Vex stopped"
}

do_status() {
    if is_running; then
        ok "Vex is running (PID $(cat "$VEX_PID"))"
        # Show config summary
        if [ -f "$VEX_CONF" ]; then
            local socks_port http_port
            socks_port=$(grep "socks_port" "$VEX_CONF" | awk '{print $2}' | head -1)
            http_port=$(grep "http_port" "$VEX_CONF" | awk '{print $2}' | head -1)
            echo "  SOCKS5 : 127.0.0.1:${socks_port:-1080}"
            echo "  HTTP   : 127.0.0.1:${http_port:-1087}"
        fi
    else
        info "Vex is stopped"
    fi
}

update_dnsmasq() {
    local action="$1"
    local DNSMASQ_CONF_DIR="/jffs/configs/dnsmasq.d"
    local VEX_DNSMASQ="$DNSMASQ_CONF_DIR/vex.conf"
    local TEMPLATE="$VEX_DIR/scripts/dnsmasq.conf.template"

    mkdir -p "$DNSMASQ_CONF_DIR"

    if [ "$action" = "start" ] && [ -f "$TEMPLATE" ]; then
        # Read DNS port from vex config (default 5300)
        local dns_port=5300
        cp "$TEMPLATE" "$VEX_DNSMASQ"
        sed -i "s/__DNS_PORT__/$dns_port/g" "$VEX_DNSMASQ"
        # Reload dnsmasq
        service restart_dnsmasq 2>/dev/null || true
        ok "dnsmasq rules applied"
    elif [ "$action" = "stop" ]; then
        rm -f "$VEX_DNSMASQ"
        service restart_dnsmasq 2>/dev/null || true
        ok "dnsmasq rules removed"
    fi
}

case "$1" in
    start)   do_start ;;
    stop)    do_stop ;;
    restart) do_stop; sleep 1; do_start ;;
    reload)
        if is_running; then
            info "Reloading Vex config..."
            do_stop; sleep 1; do_start
        else
            do_start
        fi
        ;;
    status)  do_status ;;
    log)     tail -50 "$VEX_LOG" 2>/dev/null || info "No log file found" ;;
    *)
        echo "Usage: $0 {start|stop|restart|reload|status|log}"
        exit 1
        ;;
esac
