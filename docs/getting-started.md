# 快速开始

## 安装

### 下载预编译二进制

从 [GitHub Releases](https://github.com/YOUR_USERNAME/vex/releases) 下载对应平台的二进制文件：

| 平台 | CLI | GUI |
|------|-----|-----|
| Linux x86_64 | `vex-cli-linux-x86_64` | `vex-linux-x86_64` |
| Linux aarch64 | `vex-cli-linux-aarch64` | `vex-linux-aarch64` |
| macOS x86_64 | `vex-cli-macos-x86_64` | `vex-macos-x86_64` |
| macOS arm64 | `vex-cli-macos-aarch64` | `vex-macos-aarch64` |
| Windows x86_64 | `vex-cli-windows-x86_64.exe` | `vex-windows-x86_64.exe` |

### 从源码编译

**前置依赖**

- Rust 工具链（1.85+）：`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- Linux 额外系统依赖：

```bash
sudo apt-get install -y \
    libxcb-shape0-dev libxcb-xfixes0-dev \
    libxkbcommon-dev libxkbcommon-x11-dev \
    libvulkan-dev libgl1-mesa-dev \
    libasound2-dev libfontconfig1-dev \
    libfreetype6-dev pkg-config clang
```

**编译**

```bash
git clone https://github.com/YOUR_USERNAME/vex.git
cd vex

cargo build --release --package vex-cli   # CLI
cargo build --release --package vex-gui   # GUI
```

二进制产物位于 `target/release/` 目录。

---

## 配置

### 生成默认配置

```bash
vex-cli --init config.yaml
```

### 最小配置示例

```yaml
listen_addr: "127.0.0.1"
socks_port: 1080
http_port: 1087

# 订阅链接（启动时自动拉取节点）
subscriptions:
  - name: "我的订阅"
    url: "https://example.com/subscribe?token=xxx"
    format: "auto"

# 或手动填写节点
nodes:
  - name: "My-SS"
    server: "your-server.com"
    port: 8388
    protocol:
      type: shadowsocks
      cipher: aes-256-gcm
      password: "your-password"

active_node: 0   # 使用第一个节点
```

---

## 运行

### CLI

```bash
vex-cli config.yaml
```

启动后监听：
- **SOCKS5** → `127.0.0.1:1080`
- **HTTP 代理** → `127.0.0.1:1087`

按 `Ctrl+C` 停止。

### GUI

```bash
vex
```

1. 在 **Config** 页面输入订阅链接，点击 **Import**
2. 在 **Nodes** 页面选择节点
3. 在 **Home** 页面点击 **Connect**

---

## 验证连通性

```bash
# SOCKS5
curl -x socks5://127.0.0.1:1080 https://www.google.com -I

# HTTP 代理
curl -x http://127.0.0.1:1087 https://www.google.com -I
```

---

## 下一步

- 📖 [配置参考](./configuration.md) — 完整配置选项
- 🔌 [协议支持](./protocols.md) — 各协议详细说明
- 🛠 [开发指南](./development.md) — 参与贡献
