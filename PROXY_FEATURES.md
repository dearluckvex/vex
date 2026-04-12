# xTune 代理功能实现指南

## 功能概述

xTune 现已实现了基础的 TUN 网络代理框架，包括数据包解析、转发和统计追踪。

## 核心模块

### 1. **packet** 模块 (`src/packet/mod.rs`)

负责网络数据包的解析和表示。

#### 主要功能：
- **IPv4 数据包解析**：从原始字节数据中提取 IPv4 头信息
- **协议识别**：自动识别 TCP、UDP 或其他协议
- **端口提取**：从 TCP/UDP 头中提取源端口和目标端口
- **连接追踪**：生成连接的唯一标识符（connection key）

#### 数据结构：

```rust
pub struct IpPacket {
    pub src_ip: IpAddr,           // 源 IP 地址
    pub dst_ip: IpAddr,           // 目标 IP 地址
    pub protocol: Protocol,       // 协议类型 (TCP/UDP/Other)
    pub src_port: Option<u16>,    // 源端口
    pub dst_port: Option<u16>,    // 目标端口
    pub payload: Vec<u8>,         // 载荷数据
    pub raw: Vec<u8>,             // 原始数据包
}
```

#### 使用示例：

```rust
let packet_data = [/* raw bytes */];
if let Some(packet) = IpPacket::parse(&packet_data) {
    println!("TCP 连接: {}:{} -> {}:{}", 
        packet.src_ip, packet.src_port.unwrap(),
        packet.dst_ip, packet.dst_port.unwrap()
    );
}
```

### 2. **proxy** 模块 (`src/proxy/`)

处理网络代理逻辑，包括数据转发和连接管理。

#### 子模块：

##### 2.1 TCP 转发 (`src/proxy/tcp.rs`)

**TcpForwarder** 结构体处理 TCP 连接的双向转发。

特点：
- 异步双向数据转发（使用 tokio）
- 自动处理连接关闭
- 统计往返方向的字节数
- 详细的连接日志记录

使用流程：
```rust
let forwarder = TcpForwarder::new("192.168.1.100:8080".parse()?);
let client = TcpStream::accept().await?;
let (sent, received) = forwarder.forward(client, src_addr).await?;
```

##### 2.2 UDP 转发 (`src/proxy/udp.rs`)

**UdpForwarder** 结构体处理 UDP 数据包的转发。

特点：
- 异步 UDP 数据转发
- 响应超时处理（5 秒）
- 字节计数统计
- 错误恢复机制

##### 2.3 主代理 (`src/proxy/mod.rs`)

**PacketProxy** 结构体是核心的数据包处理器。

核心函数：
```rust
pub async fn process_packet(&self, packet: IpPacket)
```

主要任务：
- 统计接收和转发的数据包数量
- 根据协议类型分发处理（TCP/UDP/Other）
- 追踪活跃连接
- 收集流量统计信息

#### 统计数据结构：

```rust
pub struct ProxyStats {
    pub packets_received: u64,      // 接收的数据包总数
    pub packets_forwarded: u64,     // 转发的数据包总数
    pub bytes_received: u64,        // 接收的总字节数
    pub bytes_forwarded: u64,       // 转发的总字节数
    pub active_connections: usize,  // 活跃连接数
}
```

## 平台实现

### Linux (`#[cfg(target_os = "linux")]`)

当前实现流程：
1. 创建 TUN 设备 `xtun0` (需要 root 权限)
2. 启动 Tokio 异步运行时
3. 从 TUN 设备读取 IP 数据包
4. 解析数据包并交由 PacketProxy 处理
5. 每接收 100 个数据包输出统计信息
6. 若无法创建设备，进入演示模式

如果无法创建设备，程序会自动进入演示模式，定期输出状态信息而不处理真实数据。

### Windows 和 macOS

- **Windows**：使用 WinTun 库创建虚拟网络适配器
- **macOS**：使用系统 TUN 接口
- 两者都支持演示模式作为 fallback

## 使用场景

### 1. 数据包拦截和日志
```
TUN 设备 → 数据包解析 → 日志记录 → 转发到目标
```

### 2. 连接追踪
```
实时追踪所有活跃 TCP/UDP 连接
显示源地址、目标地址、端口信息
统计每个连接的字节传输量
```

### 3. 流量分析
```
定期输出统计信息：
- 处理的数据包总数
- 转发的总字节数
- 活跃连接数
```

## 性能特性

- **异步处理**：使用 Tokio 实现高性能并发处理
- **零拷贝**：尽可能减少数据复制
- **流式转发**：TCP 连接使用 8KB 缓冲区
- **超时保护**：UDP 转发有 5 秒超时机制

## 测试

项目包含单元测试验证核心功能：

```bash
cargo test --bin xtune
```

测试覆盖：
- ✅ IPv4 数据包解析
- ✅ TCP 端口提取
- ✅ 连接 key 生成

## 下一步开发建议

### 短期（推荐）
1. 实现 DNS 拦截和重定向
2. 添加配置文件支持（YAML/TOML）
3. 实现规则引擎（按域名/IP 匹配）

### 中期
4. 添加 WebUI 实时监控
5. 实现数据包过滤规则
6. 添加性能优化（使用 BPF）

### 长期
7. 支持 HTTP 代理
8. 支持 SOCKS5 代理
9. 容器化部署

## 调试建议

### 启用详细日志
```bash
RUST_LOG=debug cargo run
```

### 查看日志文件
```bash
tail -f xtune.log
```

### 检查网络连接
```bash
# Linux
netstat -an | grep ESTABLISHED
ss -tunap | grep xtun

# Windows
netstat -ano

# macOS
netstat -an | grep ESTABLISHED
```

## 限制和已知问题

1. **权限要求**
   - Linux：需要 root 权限创建 TUN 设备
   - Windows：需要管理员权限和 WinTun 驱动
   - macOS：需要 root 权限

2. **当前局限**
   - TCP/UDP 转发器已实现但未在主流程中集成
   - 还未实现 DNS 拦截
   - 暂无配置文件支持
   - 监控界面还在规划中

3. **性能考量**
   - 在高流量场景下可能需要优化缓冲区大小
   - 连接追踪使用 HashMap，可能在海量连接下内存占用较大

## 相关命令

```bash
# 编译发布版本
cargo build --release

# 运行调试版本
cargo run

# 运行测试
cargo test

# 查看代码覆盖率
cargo tarpaulin --out Html

# 检查代码质量
cargo clippy

# 格式化代码
cargo fmt
```
