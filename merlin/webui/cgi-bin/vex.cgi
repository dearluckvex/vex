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

# JSON-safe string: replace backslash, double-quote, newline, tab
json_str() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g; s/	/\\t/g' | tr -d '\n\r'; }

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
        MODE=$(get_config_value mode)
        VER=$(json_str "$("$VEX_DIR/vex-cli" --version 2>/dev/null | head -1 | tr -d '"' || echo "")")
        # Count nodes: lines with "- name:" or "- server:" under nodes/subscriptions sections
        NODE_COUNT=$(awk '/^nodes:/{in_n=1;next} in_n&&/^[a-zA-Z_]/{in_n=0} in_n&&/- (name|server):/{c++} END{print c+0}' "$VEX_CONF" 2>/dev/null || echo 0)
        # Count subscriptions: entries under subscriptions: block
        SUB_COUNT=$(awk '/^subscriptions:/{in_s=1;next} in_s&&/^[a-zA-Z_]/{in_s=0} in_s&&/- name:/{c++} END{print c+0}' "$VEX_CONF" 2>/dev/null || echo 0)
        printf '{"running":%s,"pid":"%s","socks_port":"%s","http_port":"%s","active_node":"%s","node_count":%s,"sub_count":%s,"mode":"%s","version":"%s"}' \
            "$RUNNING" "$PID" "${SOCKS_PORT:-1080}" "${HTTP_PORT:-1087}" \
            "${ACTIVE_NODE:-0}" "${NODE_COUNT:-0}" "${SUB_COUNT:-0}" "${MODE:-tun}" "$VER"
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
        # Return node list from config (nodes: section only)
        if [ ! -f "$VEX_CONF" ]; then
            echo '{"nodes":[]}'
        else
            NODES=$(awk '
                /^nodes:/ { in_n=1; next }
                in_n && /^[a-zA-Z_]/ { in_n=0 }
                in_n && /- name:/ {
                    sub(/.*name:[[:space:]]*/, ""); gsub(/"/, ""); name=$0
                    printf "%s\"%s\"", (n++ > 0 ? "," : ""), name
                }
            ' "$VEX_CONF")
            ACTIVE=$(get_config_value active_node)
            printf '{"nodes":[%s],"active":%s}' "$NODES" "${ACTIVE:-0}"
        fi
        ;;

    set_node)
        # Set active node index
        INDEX=$(echo "$POST_DATA" | grep -o '"index":[0-9]*' | grep -o '[0-9]*')
        if [ -z "$INDEX" ] || [ ! -f "$VEX_CONF" ]; then
            echo '{"ok":false,"msg":"Invalid index or config not found"}'
        else
            # Validate index is within range
            NODE_COUNT=$(awk '/^nodes:/{in_n=1;next} in_n&&/^[a-zA-Z_]/{in_n=0} in_n&&/- (name|server):/{c++} END{print c+0}' "$VEX_CONF" 2>/dev/null || echo 0)
            if [ "$NODE_COUNT" -gt 0 ] && [ "$INDEX" -ge "$NODE_COUNT" ]; then
                printf '{"ok":false,"msg":"Index %s out of range (0-%s)"}' "$INDEX" "$((NODE_COUNT-1))"
            elif grep -q '^active_node:' "$VEX_CONF"; then
                sed -i "s/^active_node:.*/active_node: $INDEX/" "$VEX_CONF"
                echo '{"ok":true,"msg":"Active node updated"}'
                is_running && "$VEX_SH" restart >> "$VEX_LOG" 2>&1 &
            else
                echo "active_node: $INDEX" >> "$VEX_CONF"
                echo '{"ok":true,"msg":"Active node updated"}'
                is_running && "$VEX_SH" restart >> "$VEX_LOG" 2>&1 &
            fi
        fi
        ;;

    log)
        LINES=$(tail -150 "$VEX_LOG" 2>/dev/null | sed 's/"/'"'"'/g' | tr '\n' '|')
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
                if [ $? -eq 0 ] && [ -s "$VEX_CONF.tmp" ]; then
                    # Backup previous config
                    cp "$VEX_CONF" "$VEX_CONF.bak" 2>/dev/null || true
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
        printf '{"version":"%s"}' "$(json_str "$VER")"
        ;;

    subscriptions)
        # List subscriptions from config YAML
        if [ ! -f "$VEX_CONF" ]; then
            echo '{"subs":[]}'
        else
            SUBS=$(awk '
                BEGIN { in_s=0; name=""; url=""; fmt=""; n=0 }
                /^subscriptions:/ { in_s=1; next }
                in_s && /^[a-zA-Z_]/ { in_s=0 }
                in_s && /- name:/ {
                    if (name != "") {
                        printf "%s{\"index\":%d,\"name\":\"%s\",\"url\":\"%s\",\"format\":\"%s\"}",
                               (n>0?",":""), n, name, url, fmt
                        n++
                    }
                    sub(/.*name:[[:space:]]*/, ""); gsub(/"/, ""); name=$0
                    url=""; fmt="auto"
                }
                in_s && /url:/ && !/name:/ { sub(/.*url:[[:space:]]*/, ""); gsub(/"/, ""); url=$0 }
                in_s && /format:/           { sub(/.*format:[[:space:]]*/, ""); gsub(/"/, ""); fmt=$0 }
                END {
                    if (name != "") {
                        printf "%s{\"index\":%d,\"name\":\"%s\",\"url\":\"%s\",\"format\":\"%s\"}",
                               (n>0?",":""), n, name, url, fmt
                    }
                }
            ' "$VEX_CONF")
            printf '{"subs":[%s]}' "$SUBS"
        fi
        ;;

    add_sub)
        # Add a subscription entry to config
        NAME=$(echo "$POST_DATA" | sed 's/.*"name":"\([^"]*\)".*/\1/')
        URL=$(echo "$POST_DATA"  | sed 's/.*"url":"\([^"]*\)".*/\1/')
        FORMAT=$(echo "$POST_DATA" | sed 's/.*"format":"\([^"]*\)".*/\1/')
        [ -z "$FORMAT" ] && FORMAT="auto"
        if [ -z "$NAME" ] || [ -z "$URL" ]; then
            echo '{"ok":false,"msg":"名称和 URL 不能为空"}'
        elif [ ! -f "$VEX_CONF" ]; then
            echo '{"ok":false,"msg":"配置文件不存在"}'
        else
            tmp="$VEX_CONF.tmp.$$"
            awk -v n="$NAME" -v u="$URL" -v f="$FORMAT" '
                BEGIN { found=0; in_s=0 }
                /^subscriptions:[[:space:]]*\[\]/ {
                    print "subscriptions:"
                    printf "  - name: \"%s\"\n    url: \"%s\"\n    format: \"%s\"\n", n, u, f
                    found=1; next
                }
                /^subscriptions:/ { in_s=1; print; next }
                in_s && /^[a-zA-Z_]/ {
                    if (!found) {
                        printf "  - name: \"%s\"\n    url: \"%s\"\n    format: \"%s\"\n", n, u, f
                        found=1
                    }
                    in_s=0
                }
                { print }
                END {
                    if (!found) {
                        print "\nsubscriptions:"
                        printf "  - name: \"%s\"\n    url: \"%s\"\n    format: \"%s\"\n", n, u, f
                    }
                }
            ' "$VEX_CONF" > "$tmp" && mv "$tmp" "$VEX_CONF"
            echo '{"ok":true,"msg":"订阅已添加"}'
        fi
        ;;

    del_sub)
        # Delete subscription by index
        IDX=$(echo "$POST_DATA" | grep -o '"index":[0-9]*' | grep -o '[0-9]*')
        if [ -z "$IDX" ] || [ ! -f "$VEX_CONF" ]; then
            echo '{"ok":false,"msg":"参数错误"}'
        else
            tmp="$VEX_CONF.tmp.$$"
            awk -v target="$IDX" '
                BEGIN { in_s=0; entry=-1; skip=0 }
                /^subscriptions:/ { in_s=1; print; next }
                in_s && /^[a-zA-Z_]/ { in_s=0; skip=0 }
                in_s && /- name:/ { entry++; skip=(entry == target+0) }
                skip { next }
                { print }
            ' "$VEX_CONF" > "$tmp" && mv "$tmp" "$VEX_CONF"
            echo '{"ok":true,"msg":"订阅已删除"}'
        fi
        ;;

    update_subs)
        # Trigger subscription update (restart so vex-cli fetches new subs)
        "$VEX_SH" update-subs >> "$VEX_LOG" 2>&1 &
        echo '{"ok":true,"msg":"订阅更新已启动，完成后节点列表自动刷新"}'
        ;;

    speedtest)
        # Test latency through current active node via SOCKS5 proxy
        if ! is_running; then
            echo '{"ok":false,"latency":-1,"msg":"Vex 未运行，请先启动服务"}'
        else
            SOCKS_PORT=$(get_config_value socks_port)
            RESULT=$(curl -s -o /dev/null -w '%{time_connect}|%{http_code}' \
                --connect-timeout 10 --max-time 15 \
                --socks5 "127.0.0.1:${SOCKS_PORT:-1080}" \
                'http://www.gstatic.com/generate_204' 2>/dev/null)
            CONN=$(echo "$RESULT" | cut -d'|' -f1)
            CODE=$(echo "$RESULT" | cut -d'|' -f2)
            if [ "$CODE" = "204" ] || [ "$CODE" = "200" ]; then
                MS=$(echo "$CONN" | awk '{printf "%d", $1 * 1000}')
                printf '{"ok":true,"latency":%s}' "$MS"
            else
                echo '{"ok":false,"latency":-1,"msg":"连接失败或超时"}'
            fi
        fi
        ;;

    stats)
        # Return uptime, DNS active state, firewall active state
        UPTIME_STR="stopped"
        PID_VAL=0
        if is_running; then
            PID_VAL=$(cat "$VEX_PID")
            if [ -f "/proc/$PID_VAL/stat" ]; then
                STARTTIME=$(awk '{print $22}' "/proc/$PID_VAL/stat" 2>/dev/null || echo 0)
                BTIME=$(grep '^btime' /proc/stat 2>/dev/null | awk '{print $2}' || echo 0)
                NOW=$(date +%s 2>/dev/null || echo 0)
                if [ "$BTIME" -gt 0 ] && [ "$NOW" -gt 0 ] && [ "$STARTTIME" -gt 0 ]; then
                    START_SEC=$(( BTIME + STARTTIME / 100 ))
                    ELAPSED=$(( NOW - START_SEC ))
                    H=$(( ELAPSED / 3600 ))
                    M=$(( (ELAPSED % 3600) / 60 ))
                    S=$(( ELAPSED % 60 ))
                    UPTIME_STR="${H}h ${M}m ${S}s"
                else
                    UPTIME_STR="running"
                fi
            fi
        fi
        DNS_ACTIVE=false
        [ -f "/jffs/configs/dnsmasq.d/vex.conf" ] && DNS_ACTIVE=true
        FW_ACTIVE=false
        iptables -t nat -L VEX_PREROUTING -n 2>/dev/null | grep -q 'REDIRECT' && FW_ACTIVE=true
        printf '{"ok":true,"uptime":"%s","pid":%s,"dns_active":%s,"fw_active":%s}' \
            "$UPTIME_STR" "$PID_VAL" "$DNS_ACTIVE" "$FW_ACTIVE"
        ;;

    set_mode)
        # Set proxy mode: tun | socks | system
        MODE=$(echo "$POST_DATA" | sed 's/.*"mode":"\([^"]*\)".*/\1/')
        case "$MODE" in
            tun|socks|system)
                if [ ! -f "$VEX_CONF" ]; then
                    echo '{"ok":false,"msg":"配置文件不存在"}'
                elif grep -q '^mode:' "$VEX_CONF"; then
                    sed -i "s/^mode:.*/mode: \"$MODE\"/" "$VEX_CONF"
                    echo '{"ok":true,"msg":"模式已设置，重启生效"}'
                    is_running && "$VEX_SH" restart >> "$VEX_LOG" 2>&1 &
                else
                    echo "mode: \"$MODE\"" >> "$VEX_CONF"
                    echo '{"ok":true,"msg":"模式已设置，重启生效"}'
                    is_running && "$VEX_SH" restart >> "$VEX_LOG" 2>&1 &
                fi
                ;;
            *)
                echo '{"ok":false,"msg":"无效的代理模式"}'
                ;;
        esac
        ;;

    logs_clear)
        > "$VEX_LOG" 2>/dev/null
        echo '{"ok":true,"msg":"日志已清空"}'
        ;;

    ip_check)
        # Check external IP through the active SOCKS proxy
        if ! is_running; then
            echo '{"ok":false,"ip":"","country":"","msg":"Vex 未运行，请先启动服务"}'
        else
            SOCKS_PORT=$(get_config_value socks_port)
            RESP=$(curl -s --connect-timeout 8 --max-time 12 \
                --socks5 "127.0.0.1:${SOCKS_PORT:-1080}" \
                'http://ip-api.com/json?fields=query,country' 2>/dev/null || echo '{}')
            IP=$(printf '%s' "$RESP" | grep -o '"query":"[^"]*"' | cut -d'"' -f4)
            COUNTRY=$(printf '%s' "$RESP" | grep -o '"country":"[^"]*"' | cut -d'"' -f4)
            if [ -n "$IP" ]; then
                printf '{"ok":true,"ip":"%s","country":"%s"}' \
                    "$(json_str "$IP")" "$(json_str "$COUNTRY")"
            else
                echo '{"ok":false,"ip":"","country":"","msg":"IP 检测失败，请确认代理正常运行"}'
            fi
        fi
        ;;

    speedtest_node)
        # Per-node TCP latency test: direct connect to server:port (no proxy)
        INDEX=$(printf '%s' "$POST_DATA" | grep -o '"index":[0-9]*' | grep -o '[0-9]*')
        if [ -z "$INDEX" ] || [ ! -f "$VEX_CONF" ]; then
            echo '{"ok":false,"latency":-1,"msg":"参数错误"}'
        else
            SERVER=$(awk -v t="$INDEX" '
                /^nodes:/ { in_n=1; entry=-1; next }
                in_n && /^[a-zA-Z_]/ { in_n=0 }
                in_n && /- name:/ { entry++ }
                in_n && entry==t+0 && /server:/ { sub(/.*server:[[:space:]]*/, ""); gsub(/"/, ""); print; exit }
            ' "$VEX_CONF")
            PORT=$(awk -v t="$INDEX" '
                /^nodes:/ { in_n=1; entry=-1; next }
                in_n && /^[a-zA-Z_]/ { in_n=0 }
                in_n && /- name:/ { entry++ }
                in_n && entry==t+0 && /^[[:space:]]*port:/ { sub(/.*port:[[:space:]]*/, ""); gsub(/"/, ""); print; exit }
            ' "$VEX_CONF")
            if [ -z "$SERVER" ] || [ -z "$PORT" ]; then
                printf '{"ok":false,"latency":-1,"msg":"未找到节点 %s"}' "$INDEX"
            else
                TIME=$(curl -s -o /dev/null -w '%{time_connect}' \
                    --connect-timeout 8 --max-time 8 \
                    "http://$(json_str "$SERVER"):$PORT/" 2>/dev/null || echo "")
                if printf '%s' "$TIME" | grep -qE '^[0-9]+\.[0-9]+$' && [ "$TIME" != "0.000000" ]; then
                    MS=$(printf '%s' "$TIME" | awk '{printf "%d", $1 * 1000}')
                    [ "$MS" -eq 0 ] && MS=1
                    printf '{"ok":true,"latency":%s,"server":"%s","port":%s}' \
                        "$MS" "$(json_str "$SERVER")" "$PORT"
                else
                    printf '{"ok":false,"latency":-1,"msg":"连接 %s:%s 超时"}' \
                        "$(json_str "$SERVER")" "$PORT"
                fi
            fi
        fi
        ;;

    node_details)
        # Return full node info: [{index, name, server, port}, ...]
        if [ ! -f "$VEX_CONF" ]; then
            echo '{"nodes":[],"active":0}'
        else
            DETAILS=$(awk '
                BEGIN { in_n=0; entry=-1; name=""; server=""; port="" }
                /^nodes:/ { in_n=1; next }
                in_n && /^[a-zA-Z_]/ { in_n=0 }
                in_n && /- name:/ {
                    if (entry >= 0) {
                        printf "%s{\"index\":%d,\"name\":\"%s\",\"server\":\"%s\",\"port\":%s}",
                            (entry>0?",":""), entry, name, server, (port?port:0)
                    }
                    entry++
                    sub(/.*name:[[:space:]]*/, ""); gsub(/"/, ""); name=$0
                    server=""; port=""
                }
                in_n && entry>=0 && /[[:space:]]server:/ {
                    sub(/.*server:[[:space:]]*/, ""); gsub(/"/, ""); server=$0
                }
                in_n && entry>=0 && /^[[:space:]]*port:/ {
                    sub(/.*port:[[:space:]]*/, ""); gsub(/"/, ""); port=$0
                }
                END {
                    if (entry >= 0) {
                        printf "%s{\"index\":%d,\"name\":\"%s\",\"server\":\"%s\",\"port\":%s}",
                            (entry>0?",":""), entry, name, server, (port?port:0)
                    }
                }
            ' "$VEX_CONF")
            ACTIVE=$(get_config_value active_node)
            printf '{"nodes":[%s],"active":%s}' "$DETAILS" "${ACTIVE:-0}"
        fi
        ;;

    *)
        echo '{"error":"Unknown action"}'
        ;;
esac
