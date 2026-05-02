#!/bin/sh
# Vex iptables transparent proxy rules for AsusWRT-Merlin
#
# Mode: REDIRECT (redirects TCP traffic to local HTTP/SOCKS5 proxy)
# The router acts as a transparent proxy for all LAN clients.
#
# Usage: iptables.sh {start|stop}

VEX_DIR="/jffs/addons/vex"
VEX_CONF="$VEX_DIR/config.yaml"

# Read ports from config (defaults)
SOCKS_PORT=$(grep "socks_port" "$VEX_CONF" 2>/dev/null | awk '{print $2}' | head -1)
HTTP_PORT=$(grep "http_port" "$VEX_CONF" 2>/dev/null | awk '{print $2}' | head -1)
SOCKS_PORT=${SOCKS_PORT:-1080}
HTTP_PORT=${HTTP_PORT:-1087}

# Redirect port: we create a REDIRECT rule to send all TCP to HTTP proxy
REDIRECT_PORT=$HTTP_PORT

CHAIN="VEX_PREROUTING"

CYAN='\033[0;36m'; GREEN='\033[0;32m'; NC='\033[0m'
info() { printf "${CYAN}[Vex-iptables] %s${NC}\n" "$*"; }
ok()   { printf "${GREEN}[Vex-iptables] ✓ %s${NC}\n" "$*"; }

# LAN subnet (auto-detect from nvram, fallback to 192.168.1.0/24)
LAN_SUBNET=$(nvram get lan_ipaddr 2>/dev/null | sed 's/\.[0-9]*$/\.0\/24/' || echo "192.168.1.0/24")
ROUTER_IP=$(nvram get lan_ipaddr 2>/dev/null || echo "192.168.1.1")

do_start() {
    info "Setting up transparent proxy rules..."
    info "LAN: $LAN_SUBNET, Redirect port: $REDIRECT_PORT"

    # Create custom chain
    iptables -t nat -N "$CHAIN" 2>/dev/null || iptables -t nat -F "$CHAIN"

    # Don't redirect traffic from the router itself (loopback)
    iptables -t nat -A "$CHAIN" -o lo -j RETURN

    # Don't redirect traffic destined for the router
    iptables -t nat -A "$CHAIN" -d "$ROUTER_IP" -j RETURN

    # Don't redirect LAN-to-LAN traffic (private IP ranges)
    iptables -t nat -A "$CHAIN" -d 10.0.0.0/8 -j RETURN
    iptables -t nat -A "$CHAIN" -d 172.16.0.0/12 -j RETURN
    iptables -t nat -A "$CHAIN" -d 192.168.0.0/16 -j RETURN
    iptables -t nat -A "$CHAIN" -d 127.0.0.0/8 -j RETURN
    iptables -t nat -A "$CHAIN" -d 169.254.0.0/16 -j RETURN
    iptables -t nat -A "$CHAIN" -d 224.0.0.0/4 -j RETURN
    iptables -t nat -A "$CHAIN" -d 240.0.0.0/4 -j RETURN

    # Redirect remaining TCP traffic from LAN to Vex HTTP proxy
    iptables -t nat -A "$CHAIN" -s "$LAN_SUBNET" -p tcp \
        -j REDIRECT --to-ports "$REDIRECT_PORT"

    # Apply chain to PREROUTING
    iptables -t nat -A PREROUTING -j "$CHAIN"

    ok "iptables rules applied (TCP → 127.0.0.1:$REDIRECT_PORT)"
}

do_stop() {
    info "Removing transparent proxy rules..."

    # Remove chain from PREROUTING
    iptables -t nat -D PREROUTING -j "$CHAIN" 2>/dev/null || true

    # Flush and delete custom chain
    iptables -t nat -F "$CHAIN" 2>/dev/null || true
    iptables -t nat -X "$CHAIN" 2>/dev/null || true

    ok "iptables rules removed"
}

case "$1" in
    start)  do_start ;;
    stop)   do_stop ;;
    restart) do_stop; do_start ;;
    *)
        echo "Usage: $0 {start|stop|restart}"
        exit 1
        ;;
esac
