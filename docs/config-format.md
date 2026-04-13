# XTune 配置格式说明

## 内部配置格式

XTune 使用统一的内部 YAML 配置格式，所有导入的配置最终都会转换为此格式。

```yaml
# XTune 配置文件
listen_addr: "127.0.0.1"
socks_port: 1080
http_port: 1087
active_node: 0  # CLI 模式建议显式指定当前节点

# 订阅源
subscriptions:
  - name: "My Subscription"
    url: "https://example.com/subscribe"
    format: auto  # auto/clash/v2ray/karing

# 代理节点
nodes:
  - name: "HK-SS"
    server: "1.2.3.4"
    port: 8388
    protocol:
      type: shadowsocks
      cipher: aes-256-gcm
      password: "your-password"
      udp: true

  - name: "JP-VMess"
    server: "5.6.7.8"
    port: 443
    protocol:
      type: vmess
      uuid: "b0e80a62-8a51-47f0-91f1-f0f7faf8d9d4"
      alter_id: 0
      cipher: auto
      udp: true
    transport:
      type: tls
      tls:
        sni: "example.com"
        alpn: ["h2", "http/1.1"]

  - name: "US-VLESS"
    server: "9.10.11.12"
    port: 443
    protocol:
      type: vless
      uuid: "b85798ef-e9dc-46a4-9a87-8da4499d36d0"
      flow: "xtls-rprx-vision"
      udp: true
    transport:
      type: reality
      reality:
        public_key: "YOUR_PUBLIC_KEY"
        short_id: "0123456789abcdef"
        sni: "www.example.com"

  - name: "SG-TUIC"
    server: "13.14.15.16"
    port: 443
    protocol:
      type: tuic
      uuid: "d685aef3-b3c4-4932-9a9d-d0c2f6727dfa"
      password: "supersecret"
      congestion_control: bbr
      udp: true

# 路由规则
rules:
  - rule_type: domain-suffix
    pattern: "google.com"
    target: proxy
  - rule_type: ip-cidr
    pattern: "192.168.0.0/16"
    target: direct
  - rule_type: geoip
    pattern: CN
    target: direct
```

---

## 导入格式支持

### 1. Clash YAML 格式

Clash 是最广泛使用的代理配置格式。XTune 解析其 `proxies` 字段。

**示例**:
```yaml
proxies:
  - name: "SS-Server"
    type: ss
    server: 1.2.3.4
    port: 8388
    cipher: aes-256-gcm
    password: "password"
    udp: true

  - name: "VMess-Server"
    type: vmess
    server: 5.6.7.8
    port: 443
    uuid: "uuid-here"
    alterId: 0
    cipher: auto
    tls: true
    servername: "example.com"
    network: ws
    ws-opts:
      path: /path
      headers:
        Host: example.com

  - name: "VLESS-Server"
    type: vless
    server: 9.10.11.12
    port: 443
    uuid: "uuid-here"
    tls: true
    flow: xtls-rprx-vision
    client-fingerprint: chrome

  - name: "TUIC-Server"
    type: tuic
    server: 13.14.15.16
    port: 443
    uuid: "uuid-here"
    password: "password"
    congestion-controller: bbr
    alpn: [h3]
```

**字段映射**: Clash `type` → XTune `protocol.type`
| Clash type | XTune protocol |
|-----------|---------------|
| `ss` | `shadowsocks` |
| `vmess` | `vmess` |
| `vless` | `vless` |
| `tuic` | `tuic` |
| `trojan` | `trojan` |
| `hysteria2` | `hysteria2` |

---

### 2. V2Ray JSON 格式

V2Ray 的标准 JSON 配置格式。

**示例**:
```json
{
  "outbounds": [
    {
      "protocol": "vmess",
      "settings": {
        "vnext": [{
          "address": "5.6.7.8",
          "port": 443,
          "users": [{
            "id": "uuid-here",
            "alterId": 0,
            "security": "auto"
          }]
        }]
      },
      "streamSettings": {
        "network": "ws",
        "security": "tls",
        "wsSettings": {
          "path": "/path"
        },
        "tlsSettings": {
          "serverName": "example.com"
        }
      }
    }
  ]
}
```

---

### 3. Base64 订阅格式 (V2Ray/Karing 通用)

订阅 URL 返回 Base64 编码的文本，每行一个 URI。

**获取流程**:
1. HTTP GET 请求订阅 URL
2. Base64 解码响应体
3. 按行分割，每行是一个代理 URI
4. 解析各协议 URI

**支持的 URI 前缀**:
- `ss://` → Shadowsocks
- `vmess://` → VMess (Base64 JSON)
- `vless://` → VLESS
- `tuic://` → TUIC
- `trojan://` → Trojan
- `hysteria2://` 或 `hy2://` → Hysteria2

**示例** (解码后):
```
ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@1.2.3.4:8388#SS-Server
vmess://eyJ2IjoiMiIsInBzIjoiVk1lc3MiLC4uLn0=
vless://uuid@9.10.11.12:443?encryption=none&security=tls&sni=example.com#VLESS
tuic://uuid:password@13.14.15.16:443?congestion_control=bbr#TUIC
```

---

### 4. Karing 订阅格式

Karing 客户端使用的订阅本质上与上述 Base64 订阅或 Clash YAML 格式相同。
XTune 通过 `format: auto` 自动检测：
1. 尝试 YAML 解析 → 如果成功且包含 `proxies` 字段 → Clash 格式
2. 尝试 JSON 解析 → 如果成功且包含 `outbounds` 字段 → V2Ray 格式
3. 尝试 Base64 解码 → 如果解码后包含 `://` → Base64 订阅格式
4. 按行尝试 URI 解析 → 单节点 URI 列表

---

## 配置文件位置

| 平台 | 路径 |
|------|------|
| Windows | `%APPDATA%\xtune\config.yaml` |
| macOS | `~/Library/Application Support/xtune/config.yaml` |
| Linux | `~/.config/xtune/config.yaml` |
