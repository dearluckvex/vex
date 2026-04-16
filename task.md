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
- 手动添加/编辑节点表单（当前仅支持订阅导入）
- 规则编辑功能（当前需删除后重新添加）
- 节点和规则列表的搜索/过滤

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