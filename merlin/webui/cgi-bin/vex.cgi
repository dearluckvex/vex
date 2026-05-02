#!/bin/sh
# Vex CGI backend for AsusWRT-Merlin Web UI
# Placed at: /www/cgi-bin/vex.cgi
#
# Handles JSON API calls from the web UI page.

VEX_DIR="/jffs/addons/vex"
VEX_CONF="$VEX_DIR/config.yaml"
VEX_PID="$VEX_DIR/vex.pid"
VEX_LOG="$VEX_DIR/vex.log"
VEX_SH="$VEX_DIR/scripts/vex.sh"

# Output JSON content type
echo "Content-Type: application/json"
echo "Cache-Control: no-cache"
echo ""

# Parse query string
ACTION=$(echo "$QUERY_STRING" | grep -o 'action=[^&]*' | cut -d= -f2)
[ -z "$ACTION" ] && ACTION=$(echo "$REQUEST_URI" | grep -o 'action=[^&]*' | cut -d= -f2)

# Read POST body if needed
if [ "$REQUEST_METHOD" = "POST" ]; then
    read -r POST_DATA
fi

is_running() {
    [ -f "$VEX_PID" ] && kill -0 "$(cat "$VEX_PID")" 2>/dev/null
}

get_config_value() {
    local key="$1"
    grep "^${key}:" "$VEX_CONF" 2>/dev/null | awk '{print $2}' | head -1 | tr -d '"'
}

case "$ACTION" in

    status)
        RUNNING=false
        PID=""
        if is_running; then
            RUNNING=true
            PID=$(cat "$VEX_PID")
        fi
        SOCKS_PORT=$(get_config_value socks_port)
        HTTP_PORT=$(get_config_value http_port)
        ACTIVE_NODE=$(get_config_value active_node)
        # Count nodes
        NODE_COUNT=$(grep -c "^  - name:" "$VEX_CONF" 2>/dev/null || echo 0)
        printf '{"running":%s,"pid":"%s","socks_port":"%s","http_port":"%s","active_node":"%s","node_count":%s}' \
            "$RUNNING" "$PID" "${SOCKS_PORT:-1080}" "${HTTP_PORT:-1087}" \
            "${ACTIVE_NODE:-0}" "${NODE_COUNT:-0}"
        ;;

    start)
        if is_running; then
            echo '{"ok":false,"msg":"Already running"}'
        else
            "$VEX_SH" start >> "$VEX_LOG" 2>&1
            sleep 1
            if is_running; then
                echo '{"ok":true,"msg":"Vex started"}'
            else
                LAST_LOG=$(tail -5 "$VEX_LOG" 2>/dev/null | tr '\n' ' ')
                printf '{"ok":false,"msg":"Failed to start: %s"}' "$LAST_LOG"
            fi
        fi
        ;;

    stop)
        "$VEX_SH" stop >> "$VEX_LOG" 2>&1
        echo '{"ok":true,"msg":"Vex stopped"}'
        ;;

    restart)
        "$VEX_SH" restart >> "$VEX_LOG" 2>&1
        sleep 1
        if is_running; then
            echo '{"ok":true,"msg":"Vex restarted"}'
        else
            echo '{"ok":false,"msg":"Restart failed, check log"}'
        fi
        ;;

    nodes)
        # Return node list from config
        if [ ! -f "$VEX_CONF" ]; then
            echo '{"nodes":[]}'
        else
            # Parse node names from YAML
            NODES=$(grep "^  - name:" "$VEX_CONF" | sed 's/^  - name: *//' | tr -d '"' | \
                    awk '{printf "%s\"%s\"", (NR>1?",":""), $0}')
            ACTIVE=$(get_config_value active_node)
            printf '{"nodes":[%s],"active":%s}' "$NODES" "${ACTIVE:-0}"
        fi
        ;;

    set_node)
        # Set active node index
        INDEX=$(echo "$POST_DATA" | grep -o '"index":[0-9]*' | grep -o '[0-9]*')
        if [ -n "$INDEX" ] && [ -f "$VEX_CONF" ]; then
            sed -i "s/^active_node:.*/active_node: $INDEX/" "$VEX_CONF"
            echo '{"ok":true,"msg":"Active node updated"}'
            # Restart if running
            is_running && "$VEX_SH" restart >> "$VEX_LOG" 2>&1 &
        else
            echo '{"ok":false,"msg":"Invalid index or config not found"}'
        fi
        ;;

    log)
        LINES=$(tail -100 "$VEX_LOG" 2>/dev/null | tr '"' "'" | tr '\n' '|')
        printf '{"log":"%s"}' "$LINES"
        ;;

    config)
        # Return current config (sanitized — remove password values)
        if [ -f "$VEX_CONF" ]; then
            CONFIG=$(cat "$VEX_CONF" | tr '"' "'" | tr '\n' '|')
            printf '{"config":"%s"}' "$CONFIG"
        else
            echo '{"config":""}'
        fi
        ;;

    save_config)
        # Save config from POST body (base64-encoded for safety)
        if [ -n "$POST_DATA" ]; then
            NEW_CONF=$(echo "$POST_DATA" | grep -o '"config":"[^"]*"' | cut -d'"' -f4)
            if [ -n "$NEW_CONF" ]; then
                echo "$NEW_CONF" | base64 -d > "$VEX_CONF.tmp" 2>/dev/null
                if [ $? -eq 0 ]; then
                    mv "$VEX_CONF.tmp" "$VEX_CONF"
                    echo '{"ok":true,"msg":"Config saved"}'
                    is_running && "$VEX_SH" restart >> "$VEX_LOG" 2>&1 &
                else
                    rm -f "$VEX_CONF.tmp"
                    echo '{"ok":false,"msg":"Invalid config data"}'
                fi
            else
                echo '{"ok":false,"msg":"No config data"}'
            fi
        else
            echo '{"ok":false,"msg":"No POST data"}'
        fi
        ;;

    version)
        VER=$("$VEX_DIR/vex-cli" --version 2>/dev/null || echo "unknown")
        printf '{"version":"%s"}' "$VER"
        ;;

    *)
        echo '{"error":"Unknown action"}'
        ;;
esac
