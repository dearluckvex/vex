1. ✅ 马来西亚节点无法连接 — 已修复
   - 根因：TCP连接无超时，TcpStream::connect() 可无限挂起
   - 修复：所有TCP协议(Trojan/VLESS/VMess/Direct)添加10s连接超时
   - 增加：TCP nodelay优化、anyhow Context错误链、GUI错误详情展示

2. ✅ TUN模式跨平台完善 — 已修复
   - 设备名检测：改用 tun_name() 替代脆弱的接口扫描
   - 权限检测：启动前检查 root/Administrator 权限，给出明确提示
   - 路由守卫：TunRouteGuard 幂等恢复，避免重复清理
   - DNS拦截：TUN内截获UDP:53，使用内置DnsResolver本地解析
     (中国域名走223.5.5.5/119.29.29.29，国际域名走8.8.8.8/1.1.1.1)
   - 跨平台：Linux/macOS/Windows 各有适配的路由和权限逻辑

3. ✅ GUI现代化 — 已完成
   - 使用 GPUI + gpui-component 构建现代界面
   - 6个标签页：Home / Nodes / Config / Rules / Settings / Logs
   - Home：代理状态卡片、流量统计、TUN模式卡片
   - Nodes：节点列表、延迟测试、订阅导入、节点详情面板
   - Config：配置文件编辑器
   - Rules：规则管理（添加/删除）、规则模式选择(Direct/Proxy/Rule)
   - Settings：服务器/端口配置、验证、自动重启
   - Logs：实时日志查看器（tracing捕获、级别着色、刷新/清除）
   - 状态反馈：颜色编码的验证信息和连接状态

## issue.md 追加修复
1. ✅ Windows管理员权限检测 — 改用Win32 API IsUserAnAdmin()
   - 原方案(net session/System32写入/PowerShell)不可靠
   - 新方案直接调用shell32.dll的IsUserAnAdmin()，最可靠

2. ✅ 延迟测试优化 — 改用HTTP延迟+预热
   - 原方案(raw TCP 1-2ms)太低，不反映实际代理性能
   - 新方案：创建outbound → 预热连接(建立QUIC) → 测量第二次连接
   - 结果：TUIC约100-500ms，TCP协议约50-200ms

3. ✅ 连通性验证消息优化
   - 204 No Content 不再显示给用户（用户以为是错误）
   - 改为 "✓ Connected — internet access verified"
   - 系统代理错误消息按实际错误内容提示

## 待优化（可选）
- ✅ 手动添加节点 — 支持粘贴 vless/vmess/ss/trojan/tuic/hy2 分享链接
- ✅ 节点搜索/过滤 — 按名称、服务器地址、协议类型筛选
- ✅ 删除所有节点 — 一键清空节点列表
- ✅ 规则编辑功能 — 点击编辑按钮即可修改现有规则

## 性能优化 & 退出清理
9. ✅ 高性能数据中继 — 64KB缓冲区替代tokio默认8KB
   - 新增 `relay.rs` 模块，自定义双向中继，缓冲区从8KB提升到64KB（8倍）
   - SOCKS5/HTTP/TUN 三种代理模式全部使用新中继
   - 所有accept的TCP连接设置 `TCP_NODELAY`（之前仅出站连接设置）
   - DNS查询超时从5s缩短到3s，加速故障转移
   - DNS-over-HTTPS 复用 reqwest::Client（原来每次查询创建新Client）

10. ✅ 应用退出自动清理网络 — 防止关闭后网络不可用
    - `main.rs` 退出后自动调用 `clear_system_proxy()`
    - 注册 Ctrl+C 信号处理器，中断时也清理系统代理
    - 注册 panic hook，崩溃时也清理系统代理
    - TUN 路由守卫 (TunRouteGuard) 的 Drop 已有恢复逻辑，无需额外处理

## WinTUN驱动
- ✅ 编译时嵌入 wintun.dll（amd64/arm64/x86），运行时自动释放到exe目录
- 无需用户手动下载或配置

## 其他修复
7. ✅ Windows系统代理bypass格式 — 使用分号分隔+<local>关键字
   - Linux/macOS 使用逗号分隔
   - Windows 使用分号分隔并添加 <local> 确保浏览器正确绕过本地地址

## 订阅导入兼容性修复
4. ✅ VLESS Reality 无 short-id 支持
   - 真实订阅源 reality-opts 只有 public-key，无 short-id
   - 原代码要求两者同时存在才启用 Reality 传输
   - 修复：short-id 缺省时默认为空字符串

5. ✅ Hysteria2 sni 字段支持
   - 订阅源使用 `sni` 字段指定 SNI，原代码只读 `servername`
   - 修复：ClashProxy 增加 `sni` 字段，TLS/Reality 均支持 sni 回退

6. ✅ SS + shadow-tls 插件处理
   - 订阅源 SS 节点使用 shadow-tls 插件包装（不支持）
   - 修复：解析 plugin 字段，跳过使用不支持插件的节点并记录日志

## TLS连接失败 & 节点速度太慢修复
8. ✅ TLS连接兼容性改进 — 多项修复
   - **默认ALPN**：TLS ClientHello 始终发送 `["h2", "http/1.1"]` ALPN
     浏览器每次TLS握手都带ALPN，缺少ALPN使连接极易被DPI/防火墙识别拦截
   - **超时增加**：TCP+TLS连接超时从10s增至30s，QUIC(TUIC/Hy2)从15s增至30s
     延迟测试超时同步增至30s，QUIC空闲超时从30s增至60s
   - **Reality节点修复**：VLESS/VMess Reality transport 现在正确使用
     Reality配置中的SNI + skip_cert_verify，而非留空TLS配置导致握手失败
   - **crypto provider初始化**：transport.rs中确保rustls加密提供者已安装

   注意：完整的TLS指纹伪装(uTLS)需要较大工程量，目前rustls生态暂无等价方案。
   如果添加ALPN后仍有TLS失败，建议优先使用QUIC协议节点(TUIC/Hysteria2)，
   因为QUIC不受TLS指纹检测影响。