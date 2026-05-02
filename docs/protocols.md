# Vex 协议支持文档

## 支持的代理协议

### 1. Shadowsocks (SS)

**概述**: 轻量级加密代理协议，广泛使用，生态成熟。

**shoes 服务端配置示例**:
```yaml
- address: 0.0.0.0:8388
  protocol:
    type: shadowsocks
    cipher: aes-256-gcm
    password: "your-password"
    udp_enabled: true
```

**支持的加密方式**:
- `aes-128-gcm`, `aes-256-gcm` (推荐)
- `chacha20-ietf-poly1305` (推荐)
- `2022-blake3-aes-128-gcm`, `2022-blake3-aes-256-gcm` (SS 2022)
- `2022-blake3-chacha20-ietf-poly1305` (SS 2022)

**URI 格式**:
```
ss://base64(method:password)@host:port#name
ss://aes-256-gcm:password@1.2.3.4:8388#MyServer
```

**实现方案**: 使用 `shadowsocks` crate (v1.24)，成熟稳定。

---

### 2. VMess (V2Ray)

**概述**: V2Ray 核心协议，支持 AEAD 加密，有内置加密不依赖 TLS。

**shoes 服务端配置示例**:
```yaml
- address: 0.0.0.0:16823
  protocol:
    type: vmess
    cipher: chacha20-poly1305
    user_id: b0e80a62-8a51-47f0-91f1-f0f7faf8d9d4
    udp_enabled: true
```

**支持的加密方式**:
- `aes-128-gcm`
- `chacha20-poly1305`
- `none` (无加密，需配合 TLS)

**URI 格式** (Base64 编码 JSON):
```
vmess://base64({
  "v": "2",
  "ps": "name",
  "add": "host",
  "port": "443",
  "id": "uuid",
  "aid": "0",
  "net": "ws",
  "type": "none",
  "host": "example.com",
  "path": "/path",
  "tls": "tls"
})
```

**实现方案**: 自实现 VMess AEAD 协议，参考 shoes 和 v2ray-core 规范。

---

### 3. VLESS

**概述**: VMess 的轻量替代，无内置加密（依赖 TLS/Reality），性能更好。

**shoes 服务端配置示例**:
```yaml
- address: 0.0.0.0:443
  protocol:
    type: tls
    tls_targets:
      "vless.example.com":
        cert: cert.pem
        key: key.pem
        vision: true
        protocol:
          type: vless
          user_id: b85798ef-e9dc-46a4-9a87-8da4499d36d0
          udp_enabled: true
```

**URI 格式**:
```
vless://uuid@host:port?encryption=none&security=tls&sni=example.com&type=ws&path=/path#name
```

**参数说明**:
- `encryption`: 加密方式 (通常为 `none`)
- `security`: `tls`, `reality`, `none`
- `type`: 传输类型 `tcp`, `ws`, `grpc`
- `flow`: XTLS 流控 `xtls-rprx-vision`

**实现方案**: 自实现，协议较简单（UUID 认证 + 请求头）。

---

### 4. TUIC v5

**概述**: 基于 QUIC 的代理协议，支持 0-RTT、UDP 转发、连接迁移。

**shoes 服务端配置示例**:
```yaml
- address: 0.0.0.0:443
  transport: quic
  quic_settings:
    cert: cert.pem
    key: key.pem
  protocol:
    type: tuic
    uuid: d685aef3-b3c4-4932-9a9d-d0c2f6727dfa
    password: supersecret
```

**URI 格式**:
```
tuic://uuid:password@host:port?congestion_control=bbr&alpn=h3#name
```

**特性**:
- 0-RTT 快速连接
- 双 UDP 模式 (有损/无损)
- 流多路复用
- 网络切换时连接迁移

**实现方案**: 基于 `quinn` QUIC 库 + 自实现 TUIC v5 协议层。

---

### 5. Trojan

**概述**: 模拟 HTTPS 流量的代理协议，使用密码认证。

**URI 格式**:
```
trojan://password@host:port?sni=example.com&type=tcp#name
```

**实现方案**: 自实现，协议简单（密码 SHA256 + SOCKS5 请求）。

---

### 6. Hysteria2

**概述**: 基于 QUIC 修改版的高速代理协议，针对高丢包网络优化。

**URI 格式**:
```
hysteria2://password@host:port?sni=example.com#name
```

**实现方案**: 基于 `quinn` + Hysteria2 协议规范。

---

## 传输层支持

| 传输类型 | 说明 | 适用协议 |
|---------|------|---------|
| TCP | 直接 TCP 连接 | 所有 |
| TLS | TLS 加密传输 | VMess, VLESS, Trojan, SS |
| WebSocket | WebSocket 传输 (SIP003) | VMess, VLESS, SS |
| QUIC | QUIC 传输 | TUIC, Hysteria2 |
| Reality | XTLS Reality 伪装 | VLESS |
| Vision | XTLS Vision 流控 | VLESS |

## shoes 服务端兼容性矩阵

| 协议 | shoes 支持 | Vex 实现优先级 |
|------|-----------|----------------|
| Shadowsocks | ✅ | P0 - 首批实现 |
| VMess AEAD | ✅ | P0 - 首批实现 |
| VLESS | ✅ (+Vision) | P0 - 首批实现 |
| TUIC v5 | ✅ | P1 - 第二批 |
| Trojan | ✅ | P1 - 第二批 |
| Hysteria2 | ✅ | P2 - 第三批 |
| AnyTLS | ✅ | P2 - 第三批 |
| NaiveProxy | ✅ | P3 - 按需 |
