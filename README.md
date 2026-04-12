# xTune - 跨平台 TUN 网络适配器

一个使用 Rust 编写的跨平台 TUN（不成对的 TUN）网络适配器和代理框架，支持 Linux、Windows 和 macOS。

## 功能特性

✅ **跨平台支持**
- Linux: 使用 `tun2` 库
- Windows: 使用 `wintun` 库
- macOS: 使用 `tun` 库

✅ **条件编译**
- 每个平台使用最合适的网络库
- 自动编译对应平台的代码

✅ **代理功能**（新增）
- IP 数据包解析和识别
- TCP/UDP 协议检测
- 连接追踪和统计
- 异步数据转发框架
- 流量统计和监控
- 详细的连接日志

## 编译要求

### Linux
```bash
cargo build
```

### Windows
需要安装 WinTun 驱动程序：

1. 下载 WinTun: https://www.wintun.net/
2. 解压并将 `wintun.dll` 放到：
   - `C:\Windows\System32\` (系统目录)
   - 或项目目录下

```bash
cargo build
```

### macOS
```bash
cargo build
```

## 使用方法

### Linux
```bash
sudo ./target/debug/xtune
```

## Windows 使用指南

### 方式 A：自动打包（推荐）✨

这是最简单的方式，DLL 会自动打包到可执行文件所在目录。

1. **下载 WinTun**
   - 访问: https://www.wintun.net/
   - 下载最新版本

2. **提取 wintun.dll**
   - 解压下载的文件
   - 找到 `wintun.dll`
   - 复制到项目根目录（与 `Cargo.toml` 同级）

3. **编译**
   ```bash
   cargo build --release
   ```

4. **运行**
   ```bash
   # 需要管理员权限
   .\target\release\xtune.exe
   ```

✓ DLL 会自动复制到 `target/release/` 目录
✓ 可以将整个 `target/release/` 目录打包发布

### 方式 B：系统全局安装

如果想在系统级别安装 WinTun：

1. **下载并提取 wintun.dll**
2. **复制到系统目录**
   ```
   C:\Windows\System32\wintun.dll
   ```
3. **编译并运行**
   ```bash
   cargo build --release
   .\target\release\xtune.exe
   ```

### 发布说明

使用方式 A 打包后，可以这样发布：

```
xtune-release/
├── xtune.exe       # 主程序
├── wintun.dll      # 自动复制的 DLL
└── README.md
```

用户只需下载这个文件夹并以管理员身份运行 `xtune.exe`。

### macOS
```bash
sudo ./target/debug/xtune
```

## 配置

默认 TUN 网卡配置：
- **IP 地址**: 10.0.0.1
- **子网掩码**: 255.255.255.0
- **MTU**: 1500 字节

可在 `src/main.rs` 中修改 `config` 部分以自定义设置。

## 依赖

### Linux
- `tun2` v4.0.0 - TUN 设备创建和管理

### Windows
- `wintun` v0.5 - Windows TUN 驱动适配器
- WinTun 驱动程序（运行时必需）

### macOS
- `tun` v0.6 - macOS TUN 设备支持

所有平台
- `tokio` v1.44.0 - 异步运行时（可选）

## 平台特定说明

### Linux
- 需要 root 权限才能创建 TUN 设备
- 创建网卡名称: `xtun0`
- 使用标准同步 IO 模型

### Windows
- 需要安装 WinTun 驱动程序
- 需要管理员权限运行
- 创建网卡: `xtun`
- 必须在 `unsafe` 块中加载驱动库

### macOS
- 需要 root 权限
- 自动创建 `utun` 接口
- 需要特殊权限配置

## 错误诊断

### Linux
```
permission denied
```
**解决**: 使用 `sudo` 运行

### Windows
```
加载 WinTun 失败: LoadLibraryExW failed
```
**解决**:
1. 确保 WinTun 驱动程序已安装
2. 将 `wintun.dll` 放在 `System32` 目录或项目根目录
3. 以管理员身份运行

### macOS
```
permission denied
```
**解决**: 使用 `sudo` 运行

## 项目结构

```
xtune/
├── Cargo.toml          # 项目配置（条件依赖）
├── Cargo.lock          # 依赖锁定
├── src/
│   └── main.rs         # 主程序（条件编译）
└── README.md           # 本文件
```

## 开发指南

### 添加新平台支持

1. 在 `Cargo.toml` 中添加平台特定依赖：
```toml
[target.'cfg(target_os = "xxx")'.dependencies]
lib = "version"
```

2. 在 `src/main.rs` 中实现平台特定函数：
```rust
#[cfg(target_os = "xxx")]
fn create_tun_device() -> Result<(), Box<dyn std::error::Error>> {
    // 实现
}
```

### 条件编译标记

- `target_os = "linux"` - Linux 平台
- `target_os = "windows"` - Windows 平台
- `target_os = "macos"` - macOS 平台

## 许可证

MIT License

## 项目架构

### 模块设计

```
src/
├── main.rs               # 应用入口（平台特定逻辑）
├── packet/
│   ├── mod.rs            # IP/TCP/UDP 数据包解析
│   └── tests.rs          # 数据包解析单元测试
└── proxy/
    ├── mod.rs            # 核心代理逻辑和统计
    ├── tcp.rs            # TCP 双向转发器
    └── udp.rs            # UDP 转发器
```

### 数据流

```
TUN 设备
   ↓
读取原始数据包 (IP layer)
   ↓
packet::IpPacket::parse()  [解析 IP/TCP/UDP]
   ↓
proxy::PacketProxy::process_packet()
   ├─→ TCP 流 → proxy::tcp::TcpForwarder
   ├─→ UDP 流 → proxy::udp::UdpForwarder
   └─→ 其他   → 日志记录
   ↓
统计和监控
```

## 功能模块说明

更详细的功能说明请参考 [PROXY_FEATURES.md](./PROXY_FEATURES.md)

## 贡献

欢迎提交 Issues 和 Pull Requests！

## 相关资源

- [Rust 条件编译文档](https://doc.rust-lang.org/reference/conditional-compilation.html)
- [WinTun 项目](https://www.wintun.net/)
- [TUN 设备解释](https://en.wikipedia.org/wiki/TUN/TAP)
