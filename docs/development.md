# 开发指南

## 环境要求

| 依赖 | 最低版本 | 说明 |
|------|---------|------|
| Rust 工具链 | 1.85+ (edition 2024) | `rustup default stable && rustup update` |
| 系统包 (Linux/WSL) | — | 见下方"系统依赖"小节 |
| Git | 2.x | 代码版本管理 |

### 安装 Rust 工具链

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
rustup default stable
```

### 系统依赖 (Ubuntu / WSL2)

GPUI 框架（GUI 部分）依赖部分系统库：

```bash
sudo apt update
sudo apt install -y \
    build-essential cmake pkg-config \
    libfontconfig1-dev libfreetype-dev \
    libxcb-shape0-dev libxcb-xfixes0-dev \
    libxkbcommon-dev libwayland-dev \
    libvulkan-dev libssl-dev
```

> **Windows 原生构建**：在 Windows 上无需额外安装，但建议使用 Visual Studio Build Tools 中的 MSVC 工具链。

---

## 项目结构

```
vex/
├── Cargo.toml              # workspace 根配置
├── crates/
│   ├── vex-core/         # 核心库（协议、配置解析、代理服务）
│   ├── vex-gui/          # GUI 客户端 (gpui)
│   └── vex-cli/          # CLI 客户端
├── docs/                   # 文档
└── target/                 # 编译输出（自动生成）
```

---

## 编译

### 编译全部

```bash
cargo build
```

### 编译 Release 版本

```bash
cargo build --release
```

Release 二进制产物位于 `target/release/` 目录下：
- `vex` — GUI 客户端
- `vex-cli` — CLI 客户端

### 仅编译某个 crate

```bash
# 仅核心库
cargo build -p vex-core

# 仅 CLI
cargo build -p vex-cli

# 仅 GUI
cargo build -p vex-gui
```

### 快速检查编译（不生成二进制）

```bash
cargo check
```

---

## 运行

### 运行 GUI 客户端

```bash
cargo run -p vex-gui
# 或直接运行编译好的二进制
./target/debug/vex
```

GUI 启动后：
1. 在 **Config** 页面输入订阅链接，点击 **Import** 导入节点
2. 在 **Nodes** 页面选择一个节点
3. 在 **Home** 页面点击 **Connect** 启动代理
4. 代理端口默认为 SOCKS5=1080, HTTP=1087

### 运行 CLI 客户端

CLI 需要传入一个 YAML 配置文件：

```bash
cargo run -p vex-cli -- config.yaml
```

配置文件示例 (`config.yaml`)：

```yaml
listen_addr: "127.0.0.1"
socks_port: 1080
http_port: 1087
active_node: 0
nodes:
  - name: "My SS Node"
    server: "your-server.com"
    port: 8388
    protocol:
      type: shadowsocks
      cipher: "aes-256-gcm"
      password: "your-password"
subscriptions:
  - name: "My Sub"
    url: "https://example.com/subscribe?token=xxx"
    format: "auto"
rules: []
```

运行后 CLI 会监听配置的端口，按 `Ctrl+C` 停止。

### 配置代理后验证

启动代理后可用以下命令验证连通性：

```bash
# 通过 SOCKS5 代理测试
curl -x socks5://127.0.0.1:1080 https://www.google.com -I

# 通过 HTTP 代理测试
curl -x http://127.0.0.1:1087 https://www.google.com -I

# 测试马来西亚节点访问 Google（选中马来西亚节点后）
curl -x http://127.0.0.1:1087 https://www.google.com.my -I
```

---

## 测试

### 运行全部测试

```bash
cargo test
```

### 运行特定 crate 的测试

```bash
# 核心库测试（含协议解析、代理服务、路由等）
cargo test -p vex-core

# GUI 测试
cargo test -p vex-gui

# CLI 测试
cargo test -p vex-cli
```

### 运行匹配名称的测试

```bash
# 运行所有名称含 "clash" 的测试
cargo test clash

# 运行所有名称含 "vmess" 的测试
cargo test vmess

# 运行所有名称含 "speed" 的测试
cargo test speed
```

### 显示测试输出

```bash
cargo test -- --nocapture
```

### 当前测试覆盖

| 模块 | 测试内容 |
|------|---------|
| `config::clash` | Clash YAML 解析（SS、VMess+WS+TLS、VLESS+Reality、TUIC、混合代理） |
| `config::v2ray` | V2Ray JSON/URI 解析（VMess、VLESS、SS、Trojan、TUIC、Hysteria2 分享链接） |
| `config::subscription` | 订阅格式自动检测、Base64 解码、Plain Lines 解析 |
| `config::model` | 节点名称 URL 解码（单重/双重编码、+号空格、混合编码） |
| `proxy::connector` | Direct 出站连接 |
| `proxy::factory` | 各协议出站创建（VLESS、Trojan、SS） |
| `proxy::socks5` | SOCKS5 代理服务（IPv4、域名连接） |
| `proxy::http` | HTTP CONNECT 代理隧道 |
| `proxy::service` | 代理服务生命周期、完整流量转发 |
| `proxy::vmess` | VMess AEAD 加密（KDF、分块加密、Header 构建） |
| `proxy::trojan` | Trojan 密码哈希、请求构建 |
| `proxy::speedtest` | 真实延迟测速（通过出站连接） |
| `proxy::routing` | 路由规则匹配、直连/代理分流 |
| `router` | 路由引擎（CIDR 解析、GeoIP、规则配置） |

---

## 常见问题

### Q: 编译 GPUI 失败
确保已安装系统依赖（见上方"系统依赖"小节），特别是 `libvulkan-dev` 和字体相关库。

### Q: `cargo check` 很慢
首次编译需要下载和编译所有依赖（~300 个 crate），后续编译利用增量编译会快很多。可使用 `cargo check` 代替 `cargo build` 来加速检查。

### Q: GUI 无法启动（WSL 下）
WSL2 下运行 GUI 需要配置 X11/Wayland 转发，或使用 WSLg（Windows 11 自带）。确保 `DISPLAY` 环境变量已设置。

### Q: 节点名称显示为乱码或编码字符
导入节点后名称会自动进行 URL 解码。如果仍有问题，可尝试重新导入订阅，或检查订阅源是否使用了非标准编码。

### Q: 重启后节点列表丢失
GUI 会自动将节点保存到 `~/.config/vex/gui-state.yaml`（Linux/macOS）或 `%APPDATA%\vex\gui-state.yaml`（Windows）。如果路径不存在程序会自动创建。

---

## 贡献

1. Fork 仓库
2. 创建功能分支：`git checkout -b feat/my-feature`
3. 提交前确保以下命令通过：
   ```bash
   cargo fmt --all
   cargo clippy --workspace -- -D warnings
   cargo test --workspace
   ```
4. 提交 Pull Request
