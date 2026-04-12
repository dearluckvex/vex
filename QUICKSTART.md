# xTune 快速开始指南

## 5 分钟上手

### 1. 编译项目

```bash
cd xtune
cargo build --release
```

### 2. 运行程序

#### Linux
```bash
sudo ./target/release/xtune
```

#### Windows
```bash
.\target\release\xtune.exe
```

#### macOS
```bash
sudo ./target/release/xtune
```

### 3. 查看日志

```bash
tail -f xtune.log
```

## 功能验证

### 检查点 1：程序启动
```
✓ 看到启动横幅和平台信息
✓ 看到 "TUN 设备已创建" 或 "演示模式"
```

### 检查点 2：数据包处理
```
✓ 看到 "已处理 X 个数据包" 消息
✓ xtune.log 文件被创建并记录日志
```

### 检查点 3：连接追踪
```
✓ 程序记录 TCP/UDP 连接信息
✓ 显示源地址、目标地址、端口
```

## 运行模式

### 完整模式（需要权限）
```
TUN 设备 ← 真实 IP 包 ← 网络接口
        ↓ 数据包解析
        ↓ 转发处理
    统计和日志
```

### 演示模式（无需权限）
```
模拟 TUN 设备 → 定期输出状态 → 演示代理框架
```

## 常见问题

### Q1：Linux 上无法创建 TUN 设备
**A：** 需要 root 权限。使用 `sudo` 运行程序。

### Q2：Windows 上加载 DLL 失败
**A：** 
1. 下载 WinTun DLL：https://www.wintun.net/
2. 放到项目目录或 `C:\Windows\System32\`
3. 以管理员身份运行

### Q3：没有看到任何输出
**A：** 检查 `xtune.log` 文件是否有内容，可能在演示模式。

### Q4：如何启用详细日志
**A：** 
```bash
RUST_LOG=debug cargo run
```

## 项目结构说明

```
src/packet/         - 数据包解析
src/proxy/          - 代理转发
  ├── mod.rs        - 核心逻辑
  ├── tcp.rs        - TCP 转发
  └── udp.rs        - UDP 转发
```

## 下一步

1. 阅读 [PROXY_FEATURES.md](PROXY_FEATURES.md) 了解架构
2. 查看源代码理解实现细节
3. 运行单元测试：`cargo test`
4. 尝试修改配置参数

## 命令速查

```bash
# 开发模式编译
cargo build

# 发布模式编译（优化）
cargo build --release

# 运行程序
cargo run

# 运行测试
cargo test

# 检查代码
cargo clippy

# 格式化代码
cargo fmt

# 查看文档
cargo doc --open
```

## 输出示例

```
╔════════════════════════════════════════════════════════════════╗
║                   🚀 xTune TUN 网络适配器                     ║
╚════════════════════════════════════════════════════════════════╝

📍 平台: Linux
✓ TUN 设备已创建: xtun0
✓ 会话已启动，监听中...

📊 已处理 100 个数据包 (45230 字节)
📊 已处理 200 个数据包 (89456 字节)
```

## 支持的协议

- ✅ IPv4
- ✅ TCP (端口识别、双向转发)
- ✅ UDP (端口识别、转发)
- ✅ DNS (识别、日志记录)
- 🔄 其他协议 (日志记录)

## 性能参数

| 参数 | 值 |
|------|-----|
| TCP 缓冲区 | 8KB |
| UDP 缓冲区 | 4KB |
| UDP 超时 | 5秒 |
| 统计更新频率 | 每 100 包 |
| 最大连接数 | 受内存限制 |

## 获取帮助

- 查看代码注释
- 阅读 README.md
- 查看 PROXY_FEATURES.md
- 运行 `cargo doc --open`

---

**需要帮助？** 检查 xtune.log 或运行 `RUST_LOG=debug cargo run`
