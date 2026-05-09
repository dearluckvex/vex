# Vex

<p align="center">
  <img src="crates/vex-gui/assets/logo-banner.svg" alt="Vex Logo" width="360"/>
</p>

<p align="center">
  <a href="https://github.com/YOUR_USERNAME/vex/actions/workflows/ci.yml"><img src="https://github.com/YOUR_USERNAME/vex/actions/workflows/ci.yml/badge.svg" alt="CI"/></a>
  <a href="https://github.com/YOUR_USERNAME/vex/releases"><img src="https://github.com/YOUR_USERNAME/vex/actions/workflows/release.yml/badge.svg" alt="Release"/></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue.svg" alt="License"/></a>
</p>

<p align="right"><a href="README.zh-CN.md">中文</a> | English</p>

A cross-platform proxy client written in Rust, supporting multiple protocols, subscription management, TUN mode, and flexible routing rules.

---

## Features

- **Multi-protocol support**: Shadowsocks, VMess, VLess, TUIC, Trojan, Hysteria2
- **Dual interface**: GUI (powered by [GPUI](https://github.com/zed-industries/zed)) and CLI
- **Proxy modes**: SOCKS5, HTTP proxy, TUN (system-level transparent proxy)
- **Subscription management**: Import nodes from Clash, V2Ray / VMess, and SingBox subscription links
- **Routing rules**: Domain suffix/keyword matching, GeoIP-based routing, per-rule direct/proxy/reject actions
- **Split DNS**: China-optimized DNS with upstream group routing
- **Speed testing**: TCP latency, HTTP latency, and throughput tests per node
- **System proxy**: Automatic OS-level HTTP/HTTPS proxy configuration (Linux, macOS, Windows)
- **Cross-platform**: Linux (x86_64, aarch64), macOS (x86_64, Apple Silicon), Windows (x86_64)

---

## Installation

### Pre-built binaries

Download the latest release from [GitHub Releases](https://github.com/YOUR_USERNAME/vex/releases):

| Platform       | CLI binary                      | GUI binary                  |
|----------------|---------------------------------|-----------------------------|
| Linux x86_64   | `vex-cli-linux-x86_64`        | `vex-linux-x86_64`        |
| Linux aarch64  | `vex-cli-linux-aarch64`       | `vex-linux-aarch64`       |
| macOS x86_64   | `vex-cli-macos-x86_64`        | `vex-macos-x86_64`        |
| macOS arm64    | `vex-cli-macos-aarch64`       | `vex-macos-aarch64`       |
| Windows x86_64 | `vex-cli-windows-x86_64.exe`  | `vex-windows-x86_64.exe`  |

### Build from source

**Prerequisites**

- Rust stable toolchain (`rustup install stable`)
- Linux additional packages:
  ```bash
  sudo apt-get install -y \
    libxcb-shape0-dev libxcb-xfixes0-dev \
    libxkbcommon-dev libxkbcommon-x11-dev \
    libvulkan-dev libgl1-mesa-dev \
    libasound2-dev libfontconfig1-dev \
    libfreetype6-dev pkg-config clang
  ```

**Build**

```bash
# Clone the repository
git clone https://github.com/YOUR_USERNAME/vex.git
cd vex

# Build CLI
cargo build --release --package vex-cli

# Build GUI
cargo build --release --package vex-gui

# Binaries are in target/release/
```

---

## Quick Start

### CLI

```bash
# Generate a default config
vex-cli --init config.yaml

# Edit config.yaml to add subscriptions or nodes, then start
vex-cli config.yaml
```

The CLI listens on:
- **SOCKS5** → `127.0.0.1:1080`
- **HTTP proxy** → `127.0.0.1:1087`

### GUI

```bash
vex
```

Launch the GUI, then add subscriptions or nodes via the interface.

---

## Configuration

Copy `config.yaml.example` to `config.yaml` and edit:

```yaml
listen_addr: "127.0.0.1"
socks_port: 1080
http_port: 1087

# Index of the active node (0-based), null = direct mode
active_node: 0

# Subscription sources (auto-fetched on startup)
subscriptions:
  - name: "My Subscription"
    url: "https://example.com/subscribe?token=xxx"
    format: "auto"   # auto | clash | v2ray | singbox

# Manual nodes
nodes: []

# Routing rules
rules:
  - rule_type: "domain-suffix"
    pattern: "google.com"
    target: "proxy"
  - rule_type: "geoip"
    pattern: "CN"
    target: "direct"
```

### Supported subscription formats

| Format   | Description                       |
|----------|-----------------------------------|
| `auto`   | Auto-detect (default)             |
| `clash`  | Clash YAML config                 |
| `v2ray`  | Base64-encoded V2Ray / VMess URIs |
| `singbox` | SingBox JSON config              |

### Routing rule types

| `rule_type`        | Match against            | Example `pattern` |
|--------------------|--------------------------|-------------------|
| `domain`           | Exact domain             | `example.com`     |
| `domain-suffix`    | Domain suffix            | `google.com`      |
| `domain-keyword`   | Keyword in domain        | `youtube`         |
| `ip-cidr`          | IP address range         | `192.168.0.0/16`  |
| `geoip`            | GeoIP country code       | `CN`              |

`target` values: `proxy` | `direct` | `reject`

---

## Workspace Structure

```
vex/
├── crates/
│   ├── vex-core/    # Core library: protocols, proxy, DNS, routing
│   ├── vex-cli/     # Command-line interface
│   ├── vex-gui/     # GUI application (GPUI)
│   │   └── assets/
│   │       ├── logo.svg          # App icon (256×256, dark bg)
│   │       ├── logo-icon.svg     # In-app fox icon (transparent bg)
│   │       ├── logo-banner.svg   # Horizontal banner (480×128)
│   │       ├── icon.ico          # Windows executable icon
│   │       ├── icon.icns         # macOS app bundle icon
│   │       └── icon-{16..512}.png  # Linux / cross-platform PNGs
│   └── craftls/       # Patched rustls with custom TLS fingerprinting
├── config.yaml.example
└── .github/workflows/
    ├── ci.yml         # Build + test on every push / PR
    └── release.yml    # Release binaries on version tags
```

---

## Supported Protocols

| Protocol       | Transport          |
|----------------|--------------------|
| Shadowsocks    | TCP / UDP          |
| VMess          | TCP, WebSocket, TLS |
| VLess          | TCP, WebSocket, TLS, REALITY |
| Trojan         | TLS                |
| TUIC v5        | QUIC               |
| Hysteria2      | QUIC               |

---

## CI / CD

| Trigger        | Workflow          | What it does                              |
|----------------|-------------------|-------------------------------------------|
| push / PR      | `ci.yml`          | Build, test, Clippy, Rustfmt on all platforms |
| `git tag v*.*.*` | `release.yml`   | Build release binaries, sign, upload to GitHub Releases |

### Creating a release

```bash
git tag v0.1.0
git push origin v0.1.0
```

GitHub Actions will automatically build and publish release assets for all supported platforms.

---

## Contributing

1. Fork the repository
2. Create a feature branch: `git checkout -b feat/my-feature`
3. Commit your changes and push
4. Open a Pull Request

Please ensure `cargo fmt --all` and `cargo clippy --workspace` pass before submitting.

---

## License

[Apache License 2.0](LICENSE)
