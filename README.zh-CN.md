# Vex

<p align="center">
  <img src="crates/vex-gui/assets/logo-banner.svg" alt="Vex Logo" width="360"/>
</p>

<p align="center">
  <a href="https://github.com/dearluckvex/vex/actions/workflows/ci.yml"><img src="https://github.com/dearluckvex/vex/actions/workflows/ci.yml/badge.svg" alt="CI"/></a>
  <a href="https://github.com/dearluckvex/vex/releases"><img src="https://github.com/dearluckvex/vex/actions/workflows/release.yml/badge.svg" alt="Release"/></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue.svg" alt="License"/></a>
</p>

<p align="right">中文 | <a href="README.md">English</a></p>

一款用 Rust 编写的跨平台代理客户端，支持多种协议、订阅管理、TUN 模式和灵活的路由规则。

---

## 功能特性

- **多协议支持**：Shadowsocks、VMess、VLess、TUIC、Trojan、Hysteria2
- **双模界面**：GUI（基于 [GPUI](https://github.com/zed-industries/zed)）和 CLI
- **代理模式**：SOCKS5、HTTP 代理、TUN（系统级透明代理）
- **订阅管理**：从 Clash、V2Ray / VMess 和 SingBox 订阅链接导入节点
- **路由规则**：域名后缀/关键词匹配、基于 GeoIP 的路由、每条规则可设置直连/代理/拒绝
- **分流 DNS**：针对国内优化的 DNS，支持上游分组路由
- **速度测试**：对每个节点进行 TCP 延迟、HTTP 延迟和吞吐量测试
- **系统代理**：自动配置操作系统级 HTTP/HTTPS 代理（Linux、macOS、Windows）
- **跨平台**：Linux（x86_64、aarch64）、macOS（x86_64、Apple Silicon）、Windows（x86_64）

---

## 安装

### 预构建二进制文件

从 [GitHub Releases](https://github.com/dearluckvex/vex/releases) 下载最新版本：

| 平台             | CLI 二进制文件                   | GUI 二进制文件               |
|------------------|----------------------------------|-----------------------------|
| Linux x86_64     | `vex-cli-linux-x86_64`         | `vex-linux-x86_64`         |
| Linux aarch64    | `vex-cli-linux-aarch64`        | `vex-linux-aarch64`        |
| macOS x86_64     | `vex-cli-macos-x86_64`         | `vex-macos-x86_64`         |
| macOS arm64      | `vex-cli-macos-aarch64`        | `vex-macos-aarch64`        |
| Windows x86_64   | `vex-cli-windows-x86_64.exe`   | `vex-windows-x86_64.exe`   |

### 从源码构建

**前置依赖**

- Rust stable 工具链（`rustup install stable`）
- Linux 额外依赖包：
  ```bash
  sudo apt-get install -y \
    libxcb-shape0-dev libxcb-xfixes0-dev \
    libxkbcommon-dev libxkbcommon-x11-dev \
    libvulkan-dev libgl1-mesa-dev \
    libasound2-dev libfontconfig1-dev \
    libfreetype6-dev pkg-config clang
  ```

**构建**

```bash
# 克隆仓库
git clone https://github.com/dearluckvex/vex.git
cd vex

# 构建 CLI
cargo build --release --package vex-cli

# 构建 GUI
cargo build --release --package vex-gui

# 二进制文件位于 target/release/
```

---

## 快速开始

### CLI

```bash
# 生成默认配置
vex-cli --init config.yaml

# 编辑 config.yaml 添加订阅或节点，然后启动
vex-cli config.yaml
```

CLI 监听地址：
- **SOCKS5** → `127.0.0.1:1080`
- **HTTP 代理** → `127.0.0.1:1087`

### GUI

```bash
vex
```

启动 GUI 后，通过界面添加订阅或节点。

---

## 配置

将 `config.yaml.example` 复制为 `config.yaml` 并编辑：

```yaml
listen_addr: "127.0.0.1"
socks_port: 1080
http_port: 1087

# 当前活跃节点的索引（从 0 开始），null 表示直连模式
active_node: 0

# 订阅源（启动时自动拉取）
subscriptions:
  - name: "我的订阅"
    url: "https://example.com/subscribe?token=xxx"
    format: "auto"   # auto | clash | v2ray | singbox

# 手动节点
nodes: []

# 路由规则
rules:
  - rule_type: "domain-suffix"
    pattern: "google.com"
    target: "proxy"
  - rule_type: "geoip"
    pattern: "CN"
    target: "direct"
```

### 支持的订阅格式

| 格式      | 说明                              |
|-----------|-----------------------------------|
| `auto`    | 自动检测（默认）                  |
| `clash`   | Clash YAML 配置                   |
| `v2ray`   | Base64 编码的 V2Ray / VMess URI   |
| `singbox` | SingBox JSON 配置                 |

### 路由规则类型

| `rule_type`        | 匹配对象              | `pattern` 示例      |
|--------------------|-----------------------|---------------------|
| `domain`           | 精确域名              | `example.com`       |
| `domain-suffix`    | 域名后缀              | `google.com`        |
| `domain-keyword`   | 域名中的关键词        | `youtube`           |
| `ip-cidr`          | IP 地址范围           | `192.168.0.0/16`    |
| `geoip`            | GeoIP 国家代码        | `CN`                |

`target` 可选值：`proxy` | `direct` | `reject`

---

## 工作区结构

```
vex/
├── crates/
│   ├── vex-core/    # 核心库：协议、代理、DNS、路由
│   ├── vex-cli/     # 命令行界面
│   ├── vex-gui/     # GUI 应用（GPUI）
│   │   └── assets/
│   │       ├── logo.svg          # 应用图标（256×256，深色背景）
│   │       ├── logo-icon.svg     # 应用内狐狸图标（透明背景）
│   │       ├── logo-banner.svg   # 横幅图（480×128）
│   │       ├── icon.ico          # Windows 可执行文件图标
│   │       ├── icon.icns         # macOS 应用包图标
│   │       └── icon-{16..512}.png  # Linux / 跨平台 PNG 图标
│   └── craftls/       # 带自定义 TLS 指纹的 rustls 补丁版本
├── config.yaml.example
└── .github/workflows/
    ├── ci.yml         # 每次推送/PR 时执行构建和测试
    └── release.yml    # 版本标签时发布二进制文件
```

---

## 支持的协议

| 协议           | 传输方式                        |
|----------------|---------------------------------|
| Shadowsocks    | TCP / UDP                       |
| VMess          | TCP、WebSocket、TLS             |
| VLess          | TCP、WebSocket、TLS、REALITY    |
| Trojan         | TLS                             |
| TUIC v5        | QUIC                            |
| Hysteria2      | QUIC                            |

---

## CI / CD

| 触发条件           | 工作流             | 说明                                          |
|--------------------|--------------------|-----------------------------------------------|
| push / PR          | `ci.yml`           | 在所有平台上执行构建、测试、Clippy、Rustfmt   |
| `git tag v*.*.*`   | `release.yml`      | 构建发布二进制文件，签名后上传至 GitHub Releases |

### 创建发布版本

```bash
git tag v0.1.0
git push origin v0.1.0
```

GitHub Actions 将自动为所有支持的平台构建并发布发布资产。

---

## 贡献

1. Fork 本仓库
2. 创建功能分支：`git checkout -b feat/my-feature`
3. 提交更改并推送
4. 发起 Pull Request

提交前请确保 `cargo fmt --all` 和 `cargo clippy --workspace` 通过检查。

---

## 许可证

[Apache License 2.0](LICENSE)
