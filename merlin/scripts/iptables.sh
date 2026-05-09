#!/bin/sh
# Vex iptables transparent proxy rules for AsusWRT-Merlin
#
# TCP mode : REDIRECT → HTTP proxy port
# UDP mode : ip route → TUN device (when TUN device exists)
# DNS mode : redirect LAN port 53 → Vex DNS port
#
# Usage: iptables.sh {start|stop|restart|status}

VEX_DIR="/jffs/addons/vex"
VEX_CONF="$VEX_DIR/config.yaml"

# Read ports from config
get_conf() { grep "^${1}:" "$VEX_CONF" 2>/dev/null | awk '{print $2}' | head -1 | tr -d '"'; }
SOCKS_PORT=$(get_conf socks_port); SOCKS_PORT=${SOCKS_PORT:-1080}
HTTP_PORT=$(get_conf http_port);   HTTP_PORT=${HTTP_PORT:-1087}
DNS_PORT=$(get_conf dns_port);     DNS_PORT=${DNS_PORT:-5300}

REDIRECT_PORT=$HTTP_PORT
CHAIN="VEX_PREROUTING"
DNS_CHAIN="VEX_DNS"
MANGLE_CHAIN="VEX_MANGLE"
TUN_MARK=0x4765   # 'Ve' in hex

CYAN='\033[0;36m'; GREEN='\033[0;32m'; NC='\033[0m'
info() { printf "${CYAN}[Vex-iptables] %s${NC}\n" "$*"; }
ok()   { printf "${GREEN}[Vex-iptables] ✓ %s${NC}\n" "$*"; }

# LAN subnet from nvram
ROUTER_IP=$(nvram get lan_ipaddr 2>/dev/null || echo "192.168.1.1")
LAN_NETMASK=$(nvram get lan_netmask 2>/dev/null || echo "255.255.255.0")

_ip2int() { echo "$1" | awk -F. '{print ($1*16777216)+($2*65536)+($3*256)+$4}'; }
_int2cidr() {
    local mask_int cidr=0 bit=2147483648
    mask_int=$(_ip2int "$1")
    while [ $((mask_int & bit)) -ne 0 ]; do cidr=$((cidr+1)); bit=$((bit>>1)); done
    echo "$cidr"
}
_network() {
    local ip_int mask_int net a b c d
    ip_int=$(_ip2int "$1"); mask_int=$(_ip2int "$2")
    net=$((ip_int & mask_int))
    a=$(( (net>>24)&255 )); b=$(( (net>>16)&255 ))
    c=$(( (net>>8)&255  )); d=$(( net&255 ))
    echo "$a.$b.$c.$d"
}
LAN_SUBNET="$(_network "$ROUTER_IP" "$LAN_NETMASK")/$(_int2cidr "$LAN_NETMASK")"

detect_tun_dev() {
    ip link show vex-tun 2>/dev/null | grep -q "vex-tun" && echo "vex-tun" && return
    ip link show 2>/dev/null | awk -F: '/tun[0-9]/{print $2}' | tr -d ' ' | head -1
}

do_start() {
    info "Setting up transparent proxy rules..."
    info "LAN: $LAN_SUBNET  TCP→:$REDIRECT_PORT  DNS→:$DNS_PORT"

    # ── Private address exclusion list ─────────────────────────────────────────
    PRIVATE="-d 10.0.0.0/8 -j RETURN \
             -A VEX_TMP -d 172.16.0.0/12 -j RETURN \
             -A VEX_TMP -d 192.168.0.0/16 -j RETURN \
             -A VEX_TMP -d 100.64.0.0/10 -j RETURN \
             -A VEX_TMP -d 127.0.0.0/8 -j RETURN \
             -A VEX_TMP -d 169.254.0.0/16 -j RETURN \
             -A VEX_TMP -d 224.0.0.0/4 -j RETURN \
             -A VEX_TMP -d 240.0.0.0/4 -j RETURN"

    # ── TCP transparent proxy (REDIRECT) ──────────────────────────────────────
    iptables -t nat -N "$CHAIN" 2>/dev/null || iptables -t nat -F "$CHAIN"
    iptables -t nat -A "$CHAIN" -o lo -j RETURN
    iptables -t nat -A "$CHAIN" -d "$ROUTER_IP" -j RETURN
    iptables -t nat -A "$CHAIN" -d 10.0.0.0/8    -j RETURN
    iptables -t nat -A "$CHAIN" -d 172.16.0.0/12 -j RETURN
    iptables -t nat -A "$CHAIN" -d 192.168.0.0/16 -j RETURN
    iptables -t nat -A "$CHAIN" -d 100.64.0.0/10  -j RETURN
    iptables -t nat -A "$CHAIN" -d 127.0.0.0/8    -j RETURN
    iptables -t nat -A "$CHAIN" -d 169.254.0.0/16 -j RETURN
    iptables -t nat -A "$CHAIN" -d 224.0.0.0/4    -j RETURN
    iptables -t nat -A "$CHAIN" -d 240.0.0.0/4    -j RETURN
    iptables -t nat -A "$CHAIN" -s "$LAN_SUBNET" -p tcp \
        -j REDIRECT --to-ports "$REDIRECT_PORT"
    # Insert jump only once
    iptables -t nat -C PREROUTING -j "$CHAIN" 2>/dev/null \
        || iptables -t nat -A PREROUTING -j "$CHAIN"
    ok "TCP rules applied (LAN TCP → :$REDIRECT_PORT)"

    # ── DNS redirect (port 53 → Vex DNS port) ─────────────────────────────────
    iptables -t nat -N "$DNS_CHAIN" 2>/dev/null || iptables -t nat -F "$DNS_CHAIN"
    iptables -t nat -A "$DNS_CHAIN" -d "$ROUTER_IP" -j RETURN
    iptables -t nat -A "$DNS_CHAIN" -s "$LAN_SUBNET" -p udp --dport 53 \
        -j REDIRECT --to-ports "$DNS_PORT"
    iptables -t nat -A "$DNS_CHAIN" -s "$LAN_SUBNET" -p tcp --dport 53 \
        -j REDIRECT --to-ports "$DNS_PORT"
    iptables -t nat -C PREROUTING -j "$DNS_CHAIN" 2>/dev/null \
        || iptables -t nat -A PREROUTING -j "$DNS_CHAIN"
    ok "DNS rules applied (LAN port 53 → :$DNS_PORT)"

    # ── UDP: route to TUN device (if available) ───────────────────────────────
    TUN_DEV=$(detect_tun_dev)
    if [ -n "$TUN_DEV" ]; then
        info "TUN device found: $TUN_DEV — enabling UDP transparent proxy"
        iptables -t mangle -N "$MANGLE_CHAIN" 2>/dev/null \
            || iptables -t mangle -F "$MANGLE_CHAIN"
        iptables -t mangle -A "$MANGLE_CHAIN" -d 10.0.0.0/8    -j RETURN
        iptables -t mangle -A "$MANGLE_CHAIN" -d 172.16.0.0/12 -j RETURN
        iptables -t mangle -A "$MANGLE_CHAIN" -d 192.168.0.0/16 -j RETURN
        iptables -t mangle -A "$MANGLE_CHAIN" -d 100.64.0.0/10  -j RETURN
        iptables -t mangle -A "$MANGLE_CHAIN" -d 127.0.0.0/8    -j RETURN
        iptables -t mangle -A "$MANGLE_CHAIN" -d 224.0.0.0/4    -j RETURN
        iptables -t mangle -A "$MANGLE_CHAIN" -d 240.0.0.0/4    -j RETURN
        iptables -t mangle -A "$MANGLE_CHAIN" -s "$LAN_SUBNET" -p udp \
            -j MARK --set-mark "$TUN_MARK"
        iptables -t mangle -C PREROUTING -j "$MANGLE_CHAIN" 2>/dev/null \
            || iptables -t mangle -A PREROUTING -j "$MANGLE_CHAIN"
        ip rule  add fwmark "$TUN_MARK" table 100 2>/dev/null || true
        ip route add default dev "$TUN_DEV" table 100 2>/dev/null || true
        ok "UDP rules applied (LAN UDP → $TUN_DEV)"
    else
        info "TUN device not found — UDP transparent proxy disabled"
    fi
}

do_stop() {
    info "Removing transparent proxy rules..."

    # TCP rules
    iptables -t nat -D PREROUTING -j "$CHAIN"     2>/dev/null || true
    iptables -t nat -F "$CHAIN" 2>/dev/null || true
    iptables -t nat -X "$CHAIN" 2>/dev/null || true

    # DNS rules
    iptables -t nat -D PREROUTING -j "$DNS_CHAIN" 2>/dev/null || true
    iptables -t nat -F "$DNS_CHAIN" 2>/dev/null || true
    iptables -t nat -X "$DNS_CHAIN" 2>/dev/null || true

    # UDP/mangle rules
    iptables -t mangle -D PREROUTING -j "$MANGLE_CHAIN" 2>/dev/null || true
    iptables -t mangle -F "$MANGLE_CHAIN" 2>/dev/null || true
    iptables -t mangle -X "$MANGLE_CHAIN" 2>/dev/null || true

    # ip rule / route
    ip rule  del fwmark "$TUN_MARK" table 100 2>/dev/null || true
    ip route flush table 100 2>/dev/null || true

    ok "iptables rules removed"
}

do_status() {
    echo "=== $CHAIN (nat) ==="
    iptables -t nat    -L "$CHAIN"     -n --line-numbers 2>/dev/null || echo "(not active)"
    echo ""
    echo "=== $DNS_CHAIN (nat) ==="
    iptables -t nat    -L "$DNS_CHAIN" -n --line-numbers 2>/dev/null || echo "(not active)"
    echo ""
    echo "=== $MANGLE_CHAIN (mangle) ==="
    iptables -t mangle -L "$MANGLE_CHAIN" -n --line-numbers 2>/dev/null || echo "(not active)"
}

case "$1" in
    start)   do_start ;;
    stop)    do_stop ;;
    restart) do_stop; do_start ;;
    status)  do_status ;;
    *)
        echo "Usage: $0 {start|stop|restart|status}"
        exit 1
        ;;
esac
