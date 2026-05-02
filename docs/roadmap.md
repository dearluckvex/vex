# 项目路线图

## 已完成功能

### ✅ Phase 1：项目基础架构
- Cargo workspace 与各 crate 骨架
- GPUI 窗口框架集成
- 核心数据结构定义（Node, Config, ProxyProtocol 等）

### ✅ Phase 2：配置系统
- 统一内部配置模型（YAML）
- Clash YAML 配置解析
- V2Ray JSON 配置解析
- 订阅 URL 获取与解码（Base64 / JSON）
- SingBox 格式支持
- URI 格式解析（ss/vmess/vless/tuic/trojan/hysteria2）

### ✅ Phase 3：本地代理服务
- SOCKS5 本地代理服务器
- HTTP CONNECT 代理服务器
- 代理服务生命周期管理
- 系统代理自动配置（Linux / macOS / Windows）

### ✅ Phase 4：远程协议实现
- TLS 传输层（rustls + craftls 指纹伪装）
- Shadowsocks 客户端（AEAD + AEAD-2022）
- VMess AEAD 客户端
- VLESS 客户端（含 XTLS Vision）
- TUIC v5 客户端（QUIC）
- Trojan 客户端
- Hysteria2 客户端（QUIC）
- 连接池（ConnPool）

### ✅ Phase 5：GUI 功能完善
- 侧边栏导航（Home / Nodes / Config / Settings）
- 节点列表（协议标签、延迟显示、节点选择）
- 订阅导入界面
- 连接状态面板（开始/停止、流量统计）
- GPUI + tokio 异步桥接

### ✅ Phase 6：高级功能
- 规则路由引擎（域名 / IP-CIDR / GeoIP）
- DNS 解析器（分组策略、中国分流）
- TUN 透明代理模式（Linux / macOS / Windows）
- 速度测试（TCP 延迟 / HTTP 延迟 / 吞吐量）
- RetryOutbound 自动重试

## 待规划

### 🔲 稳定性改进
- 代理健康检查与自动重启
- UDP ASSOCIATE 错误码规范化
- 订阅获取超时保护

### 🔲 性能优化
- ConnPool 存活探测（降低 max_age）
- HTTP 延迟测试复用 Keep-Alive 连接
- Router 缓存策略改进（LRU 替换全量清除）

### 🔲 功能扩展
- AnyTLS 协议支持
- 节点分组与策略组
- GUI 暗色主题
- 配置文件热重载
