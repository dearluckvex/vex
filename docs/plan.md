# XTune - Rust 跨平台代理客户端

## 项目概述

XTune 是一款基于 Rust 的跨平台代理/VPN 客户端，支持 TUIC v5、Shadowsocks、VMess、VLESS 等常见协议，GUI 使用 GPUI + gpui-component，适配 [shoes](https://github.com/cfal/shoes) 服务端。

## 核心目标

1. **多协议支持** - TUIC v5、Shadowsocks、VMess AEAD、VLESS、Trojan、Hysteria2
2. **跨平台** - Windows / macOS / Linux 桌面端
3. **云端配置导入** - 支持 Karing、V2Ray、Clash 订阅和配置格式
4. **低权限运行** - 默认使用 SOCKS5/HTTP 代理模式，无需管理员权限
5. **shoes 服务端适配** - 协议实现与 shoes 兼容
6. **路由器部署** - 核心代码支持 CLI 模式部署到 Linux 路由器

## 分阶段实现计划

### Phase 1: 项目脚手架 + 基础架构 ✅ 已完成
- 建立 Cargo workspace 和各 crate 骨架
- 配置 gpui + gpui-component 依赖
- 实现最基本的 GPUI 窗口
- 定义核心数据结构 (Node, Config, ProxyProtocol 等)

### Phase 2: 配置系统 ✅ 已完成
- 定义统一的内部配置模型
- 实现 Clash YAML 配置解析
- 实现 V2Ray JSON 配置解析
- 实现订阅 URL 获取与解码 (Base64/JSON)
- 支持 Karing 订阅格式导入
- 17 个单元测试全部通过

### Phase 3: 本地代理服务 ⬅️ 当前阶段
- 实现 SOCKS5 本地代理服务器
- 实现 HTTP 本地代理服务器
- 连接管理与生命周期
- 系统代理设置（无需管理员权限）

### Phase 4: 远程协议实现 ✅ 已完成
- TLS 传输层（rustls + insecure verifier）
- Shadowsocks 客户端（shadowsocks crate, AEAD + AEAD-2022）
- VLESS 客户端
- Trojan 客户端
- Node→Outbound 工厂函数
- VMess/TUIC/Hysteria2：桩实现（fallback DirectOutbound）

### Phase 5: GUI 功能完善 ✅ 已完成
- 侧边栏导航布局（Home/Nodes/Config/Settings）
- 节点列表视图（协议标签、延迟测试、选择指示器）
- 配置导入界面（订阅URL输入、自动格式检测）
- 连接状态面板（开始/停止、状态指示、端点展示）
- 流量统计（活跃连接、总连接数）
- 设置面板（监听地址、SOCKS5/HTTP端口）
- Tokio运行时桥接（GUI异步控制代理服务）

### Phase 6: 高级功能
- 规则路由引擎（域名 / IP / GeoIP）
- DNS 解析策略
- Linux 路由器部署模式 (CLI)
- 可选 TUN 模式
