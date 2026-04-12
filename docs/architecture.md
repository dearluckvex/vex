# XTune 技术架构设计

## 技术选型

| 模块 | 技术方案 | 版本 | 说明 |
|------|---------|------|------|
| GUI 框架 | gpui + gpui-component | 0.2.2 / 0.5.1 | Zed 编辑器的 GPU 加速 UI 框架，60+ 组件，跨平台 |
| 异步运行时 | tokio | 1.x | 异步网络 IO |
| Shadowsocks | shadowsocks crate | 1.24 | 成熟的 SS 协议库 |
| VMess/VLESS | 自实现 | - | 基于 tokio 实现 AEAD 加密，兼容 shoes |
| TUIC v5 | quinn (QUIC) + 自实现 | - | QUIC 传输 + TUIC v5 协议 |
| TLS | rustls / tokio-rustls | 0.23 / 0.26 | 纯 Rust TLS，无 OpenSSL 依赖 |
| 配置解析 | serde + serde_yaml + serde_json | - | 支持 Clash/V2Ray/Karing 格式 |
| HTTP 客户端 | reqwest | 0.12 | 订阅下载 |
| 系统代理 | sysproxy-rs | - | 无需管理员权限设置系统代理 |

## 项目结构

```
xtune/
├── Cargo.toml                 # workspace root
├── crates/
│   ├── xtune-core/            # 核心库: 协议、配置、路由
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── config/        # 配置解析 (Clash/V2Ray/Karing)
│   │   │   │   ├── mod.rs
│   │   │   │   ├── model.rs   # 统一数据模型
│   │   │   │   ├── clash.rs   # Clash YAML 解析器
│   │   │   │   ├── v2ray.rs   # V2Ray JSON/URI 解析器
│   │   │   │   └── subscription.rs  # 订阅获取与解码
│   │   │   ├── proxy/         # 代理协议实现
│   │   │   │   ├── mod.rs
│   │   │   │   ├── socks5.rs  # 本地 SOCKS5 服务
│   │   │   │   ├── http.rs    # 本地 HTTP 代理服务
│   │   │   │   ├── connector.rs # 协议分发器
│   │   │   │   ├── ss.rs      # Shadowsocks 客户端
│   │   │   │   ├── vmess.rs   # VMess 客户端
│   │   │   │   ├── vless.rs   # VLESS 客户端
│   │   │   │   └── tuic.rs    # TUIC v5 客户端
│   │   │   ├── router/        # 规则路由引擎
│   │   │   └── dns/           # DNS 解析
│   │   └── Cargo.toml
│   ├── xtune-gui/             # GUI 应用 (GPUI)
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── app.rs         # 应用主框架
│   │   │   ├── views/         # UI 视图
│   │   │   │   ├── home.rs    # 主页 / 连接状态
│   │   │   │   ├── nodes.rs   # 节点列表
│   │   │   │   ├── config.rs  # 配置管理
│   │   │   │   └── settings.rs# 设置页面
│   │   │   └── components/    # 自定义组件
│   │   └── Cargo.toml
│   └── xtune-cli/             # CLI 工具 (Linux 路由器)
│       ├── src/main.rs
│       └── Cargo.toml
└── docs/                      # 文档
```

## 整体架构

```
┌─────────────────────────────────────────────────────┐
│              用户界面层 (UI Layer)                    │
│  ┌───────────────────┐  ┌────────────────────────┐  │
│  │  xtune-gui (GPUI) │  │  xtune-cli (Terminal)  │  │
│  │  桌面 GUI 客户端   │  │  路由器/服务器 CLI     │  │
│  └────────┬──────────┘  └──────────┬─────────────┘  │
└───────────┼────────────────────────┼────────────────┘
            │                        │
            ▼                        ▼
┌─────────────────────────────────────────────────────┐
│              核心库 (xtune-core)                     │
│                                                      │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────┐  │
│  │ 配置系统      │  │ 本地代理服务  │  │ 路由引擎  │  │
│  │ Config Layer │  │ Local Proxy  │  │ Router    │  │
│  │              │  │              │  │           │  │
│  │ • Clash YAML │  │ • SOCKS5     │  │ • 域名匹配│  │
│  │ • V2Ray JSON │  │ • HTTP       │  │ • IP/CIDR │  │
│  │ • 订阅URL    │  │ • 系统代理   │  │ • GeoIP   │  │
│  └──────────────┘  └──────┬───────┘  └─────┬─────┘  │
│                           │                │         │
│                           ▼                ▼         │
│  ┌─────────────────────────────────────────────────┐ │
│  │           协议连接器 (Protocol Connector)        │ │
│  │                                                  │ │
│  │  ┌────────┐ ┌───────┐ ┌───────┐ ┌──────────┐   │ │
│  │  │   SS   │ │ VMess │ │ VLESS │ │ TUIC v5  │   │ │
│  │  │        │ │ AEAD  │ │       │ │ (QUIC)   │   │ │
│  │  └────────┘ └───────┘ └───────┘ └──────────┘   │ │
│  │  ┌────────┐ ┌───────────┐                       │ │
│  │  │ Trojan │ │ Hysteria2 │                       │ │
│  │  └────────┘ └───────────┘                       │ │
│  └─────────────────────────────────────────────────┘ │
│                           │                          │
│  ┌─────────────────────────────────────────────────┐ │
│  │           传输层 (Transport Layer)               │ │
│  │  TCP │ TLS (rustls) │ WebSocket │ QUIC (quinn)  │ │
│  │  Reality │ ShadowTLS                             │ │
│  └─────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────┘
            │
            ▼
     ┌──────────────┐
     │ shoes 服务端  │
     │ 远程代理节点  │
     └──────────────┘
```

## 关键设计决策

### 1. 无管理员权限运行
- **默认模式**: SOCKS5/HTTP 本地代理 → 设置系统代理（无需 root/admin）
- **可选 TUN 模式**: 需要管理员权限，仅在用户主动启用时使用
- 使用 `sysproxy-rs` 在各平台设置系统代理

### 2. shoes 服务端适配
shoes 支持的协议（XTune 需要兼容的）：
- VMess AEAD (`aes-128-gcm`, `chacha20-poly1305`, `none`)
- VLESS（含 XTLS Vision）
- Shadowsocks（含 2022 AEAD）
- TUIC v5
- Trojan
- Hysteria2

shoes 的传输层：
- TCP / TLS / WebSocket / QUIC
- XTLS Reality / XTLS Vision
- ShadowTLS v3

### 3. 配置导入兼容性
| 格式 | 来源 | 解析方式 |
|------|------|---------|
| Clash YAML | Clash/mihomo 订阅 | serde_yaml 直接解析 proxies 列表 |
| V2Ray JSON | V2Ray/Xray 客户端 | serde_json 解析 outbounds |
| Base64 订阅 | 通用订阅链接 | Base64 解码后逐行解析 URI |
| Karing 订阅 | Karing 客户端 | 等同 Base64 订阅或 Clash 格式 |
| URI 链接 | 单节点分享 | 解析 `ss://` `vmess://` `vless://` `tuic://` |

### 4. 跨平台策略
- GPUI 原生支持 Windows (Direct3D) / macOS (Metal) / Linux (X11/Wayland)
- CLI 模式支持无 GUI 环境（路由器、服务器）
- 使用 rustls 替代 OpenSSL，减少系统依赖
