# Vex — AsusWRT-Merlin Router Plugin

透明代理插件，将 Vex 代理客户端集成到搭载 **AsusWRT-Merlin** 固件的华硕路由器中。

## 功能

- **透明代理**：通过 iptables 将局域网 TCP 流量自动重定向（无需客户端配置）
- **DNS 防泄露**：劫持路由器 DNS 请求至本地 Vex DNS 端口，防止 DNS 泄露
- **订阅管理**：在 Web UI 中添加/删除/更新订阅链接
- **节点切换**：从 Web UI 选择节点，支持一键测速
- **代理模式**：支持 TUN / SOCKS5 / 系统代理三种模式
- **自动更新**：每日自动更新订阅（cron），每 5 分钟守护进程自动重启崩溃的服务
- **Web UI**：集成到 Merlin 路由器管理界面（`/ext/vex/vex.asp`）

## 支持架构

| 架构 | 说明 |
|------|------|
| `armv7` | 大多数旧款华硕路由器（如 RT-AC86U） |
| `aarch64` | 新款 AX 系列路由器（如 RT-AX88U、GT-AX11000） |

## 前置要求

- AsusWRT-Merlin 固件（384.x 或更高）
- 已启用 JFFS 自定义脚本（管理页面 → 系统管理 → 系统设置）
- SSH 访问权限

## 安装

### 方式一：离线安装（推荐）

将插件包上传到路由器后执行：

```sh
# 自动检测架构
sh install.sh

# 或手动指定架构
sh install.sh aarch64
sh install.sh armv7
```

### 方式二：在线安装（从 GitHub Releases 下载二进制）

```sh
# 设置你的 GitHub 仓库（包含 Release 的仓库）
export VEX_GITHUB_REPO="YOUR_USERNAME/vex"

sh install.sh --online
sh install.sh --online aarch64
```

### 方式三：仅更新二进制（保留配置）

```sh
sh install.sh --update
```

## 卸载

```sh
sh uninstall.sh

# 无交互式确认（用于脚本）
sh uninstall.sh --force
```

## 配置

配置文件位于 `/jffs/addons/vex/config.yaml`，安装时会从模板自动生成。

```yaml
# 主要配置项
mode: "tun"          # 代理模式: tun | socks | system
dns_port: 5300       # 本地 DNS 监听端口（不要与 dnsmasq 冲突）

subscriptions:
  - name: "我的机场"
    url: "https://example.com/subscribe?token=xxx"

# 手动添加节点（不使用订阅时）
nodes:
  - name: "my-server"
    server: "1.2.3.4"
    port: 443
    # ... 其他节点参数
```

修改配置后，重新加载服务：

```sh
/jffs/addons/vex/scripts/vex.sh reload
```

## 服务管理

```sh
VEX=/jffs/addons/vex/scripts/vex.sh

$VEX start          # 启动
$VEX stop           # 停止
$VEX restart        # 重启
$VEX reload         # 重新加载配置（不停止 iptables）
$VEX status         # 查看运行状态
$VEX log            # 查看日志（最近 150 行）
$VEX update-subs    # 立即更新所有订阅
$VEX watchdog       # 手动触发守护进程检查
```

## iptables 管理

```sh
IPT=/jffs/addons/vex/scripts/iptables.sh

$IPT start          # 应用所有规则
$IPT stop           # 删除所有规则
$IPT status         # 查看当前规则状态
```

## 目录结构

```
/jffs/addons/vex/
├── vex-cli                    # 代理客户端二进制
├── config.yaml                # 主配置文件
├── config.yaml.bak            # 配置备份（安装时自动创建）
├── vex.pid                    # 运行时 PID 文件
├── vex.log                    # 运行日志（最大 512KB，自动轮转）
├── vex.lock                   # 操作锁文件（防止并发）
└── scripts/
    ├── vex.sh                 # 服务控制脚本
    ├── iptables.sh            # iptables 规则管理
    └── dnsmasq.conf.template  # dnsmasq 配置模板

/jffs/configs/dnsmasq.d/
└── vex.conf                   # 生成的 dnsmasq 配置（DNS 防泄露）

/www/ext/vex/
└── vex.asp                    # Web UI 页面

/www/cgi-bin/
└── vex.cgi                    # Web UI 后端 CGI 接口
```

## Merlin 启动钩子

安装程序会自动向以下脚本追加钩子（幂等，不会重复添加）：

| 脚本 | 触发时机 | 作用 |
|------|----------|------|
| `/jffs/scripts/firewall-start` | 防火墙重启时 | 应用 iptables 规则 |
| `/jffs/scripts/services-start` | 路由器启动时 | 启动 Vex 服务 |
| `/jffs/scripts/service-event` | `service restart vex` | 重启 Vex 服务 |

## 排错

**查看日志：**
```sh
/jffs/addons/vex/scripts/vex.sh log
# 或实时查看
tail -f /jffs/addons/vex/vex.log
```

**检查 iptables 规则：**
```sh
/jffs/addons/vex/scripts/iptables.sh status
iptables -t nat -L VEX_TCP -n -v
iptables -t nat -L VEX_DNS -n -v
```

**DNS 检查：**
```sh
cat /jffs/configs/dnsmasq.d/vex.conf
nslookup google.com 127.0.0.1
```

**Web UI 无法访问：**
- 确认 `/www/cgi-bin/vex.cgi` 有执行权限：`chmod +x /www/cgi-bin/vex.cgi`
- 检查 CGI 脚本输出：`sh /www/cgi-bin/vex.cgi action=status`

## 常见问题

**Q: 局域网设备无法上网**  
A: 运行 `/jffs/addons/vex/scripts/iptables.sh stop` 临时关闭代理，确认是规则问题还是 Vex 服务问题。

**Q: DNS 没有走代理**  
A: 检查 `config.yaml` 中 `dns_port` 与 `dnsmasq.conf` 中监听端口是否一致。执行 `vex.sh reload` 重新生成 dnsmasq 配置。

**Q: 订阅更新失败**  
A: 检查路由器是否能访问订阅链接：`curl -v "订阅URL"`。如果是 HTTPS，确认路由器有 CA 证书（`curl` 加 `-k` 测试）。

**Q: 重启路由器后规则丢失**  
A: 检查 `/jffs/scripts/firewall-start` 和 `/jffs/scripts/services-start` 中是否有 Vex 的钩子。如果没有，重新运行 `sh install.sh`。
