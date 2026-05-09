# 梅林路由器插件

Vex 支持作为插件运行在搭载 **AsusWRT-Merlin** 固件的华硕路由器上，实现局域网内所有设备的透明代理——无需在每台设备上单独配置。

效果类似 fancyss，但支持 Vex 的全部协议：Shadowsocks、VMess、VLess、TUIC、Trojan、Hysteria2。

---

## 前置条件

- 华硕路由器，已刷 **AsusWRT-Merlin** 固件（推荐 388.x+）
- 路由器管理页面已启用 **JFFS 分区** 和 **自定义脚本**
  （管理 → 系统 → Persistent JFFS2 partition → 启用）
- CPU 架构：`armv7`（大多数路由器）或 `aarch64`（部分新款路由器）

### 查看路由器 CPU 架构

SSH 登录路由器后执行：
```bash
uname -m
# armv7l  → 下载 armv7 包
# aarch64 → 下载 aarch64 包
```

---

## 安装

### 1. 下载插件包

从 [GitHub Releases](https://github.com/dearluckvex/vex/releases) 下载对应架构的插件包：

| 架构 | 包名 | 适用型号 |
|------|------|---------|
| armv7 | `vex-merlin-armv7.tar.gz` | RT-AC86U、RT-AC68U 等大多数型号 |
| aarch64 | `vex-merlin-aarch64.tar.gz` | RT-AX88U、GT-AX11000 等新款型号 |

### 2. 上传到路由器

```bash
# 方式一：使用 scp（在你的电脑上执行）
scp vex-merlin-armv7.tar.gz admin@192.168.1.1:/tmp/

# 方式二：通过路由器 Web UI 上传（使用文件管理器或 SSH）
```

### 3. SSH 登录路由器并安装

```bash
# SSH 登录路由器
ssh admin@192.168.1.1

# 解压
cd /tmp
tar -xzf vex-merlin-armv7.tar.gz

# 安装
sh vex-merlin-armv7/install.sh
```

安装完成后会看到：
```
[Vex] ✓ Binary installed at /jffs/addons/vex/vex-cli
[Vex] ✓ Control scripts installed
[Vex] ✓ Web UI installed
[Vex] ✓ Hooked into firewall-start
[Vex] ✓ Hooked into services-start
[Vex] ✓ Vex installed successfully!
```

---

## 配置

编辑配置文件：
```bash
vi /jffs/addons/vex/config.yaml
```

### 最小配置（使用订阅）

```yaml
listen_addr: "0.0.0.0"    # 路由器模式，监听所有接口
socks_port: 1080
http_port: 1087

subscriptions:
  - name: "我的订阅"
    url: "https://example.com/subscribe?token=xxx"
    format: "auto"

active_node: 0

rules:
  - rule_type: "ip-cidr"
    pattern: "192.168.0.0/16"
    target: "direct"
  - rule_type: "geoip"
    pattern: "CN"
    target: "direct"
```

### 最小配置（手动填节点）

```yaml
listen_addr: "0.0.0.0"
socks_port: 1080
http_port: 1087

nodes:
  - name: "My-HK"
    server: "hk.example.com"
    port: 443
    protocol:
      type: shadowsocks
      cipher: aes-256-gcm
      password: "your-password"

active_node: 0
```

---

## 启动与管理

```bash
# 启动
/jffs/addons/vex/scripts/vex.sh start

# 停止
/jffs/addons/vex/scripts/vex.sh stop

# 重启
/jffs/addons/vex/scripts/vex.sh restart

# 查看状态
/jffs/addons/vex/scripts/vex.sh status

# 查看日志
/jffs/addons/vex/scripts/vex.sh log
```

### 开机自启

安装时已自动注册到 `/jffs/scripts/services-start`，路由器重启后会自动启动。

---

## Web 管理界面

访问：**http://192.168.1.1/ext/vex/vex.asp**

> 将 `192.168.1.1` 替换为你的路由器 IP

界面功能：
- **总览**：查看运行状态、端口信息、节点列表，一键启停
- **配置**：在线编辑 `config.yaml`，保存后自动重启
- **日志**：实时查看运行日志

---

## 透明代理工作原理

```
局域网设备 (192.168.1.x)
    │
    ▼  (iptables REDIRECT)
路由器 :1087 (Vex HTTP 代理)
    │
    ├── 中国 IP / 局域网 → 直连出口
    └── 境外流量 → 代理节点 → 目标服务器
```

- **iptables**：将局域网 TCP 流量重定向到 Vex HTTP 代理端口
- **dnsmasq**：DNS 查询转发给 Vex 内置 DNS 解析器，实现 DNS 防污染
- **路由规则**：GeoIP + 自定义规则决定直连或代理

---

## 卸载

```bash
sh /jffs/addons/vex/uninstall.sh
```

或手动执行：
```bash
/jffs/addons/vex/scripts/vex.sh stop
rm -rf /jffs/addons/vex
rm -rf /www/ext/vex
rm -f /www/cgi-bin/vex.cgi
# 删除 /jffs/scripts/ 中的 vex 相关行
```

---

## 故障排查

### 代理无效果

1. 确认 Vex 正在运行：`/jffs/addons/vex/scripts/vex.sh status`
2. 确认 iptables 规则存在：`iptables -t nat -L VEX_PREROUTING -n`
3. 查看日志：`/jffs/addons/vex/scripts/vex.sh log`

### 路由器重启后不自动启动

检查 `/jffs/scripts/services-start` 是否包含 Vex 启动命令：
```bash
cat /jffs/scripts/services-start | grep vex
```
若不存在，重新运行安装脚本或手动添加：
```bash
echo '/jffs/addons/vex/scripts/vex.sh start' >> /jffs/scripts/services-start
chmod +x /jffs/scripts/services-start
```

### Web UI 无法访问

确认 `/www/ext/vex/vex.asp` 和 `/www/cgi-bin/vex.cgi` 文件存在，且 CGI 有可执行权限：
```bash
ls -la /www/ext/vex/ /www/cgi-bin/vex.cgi
chmod +x /www/cgi-bin/vex.cgi
```

### 节点列表为空

编辑 `/jffs/addons/vex/config.yaml`，添加订阅或手动节点，然后重启服务。

---

## 支持的路由器型号（参考）

| 型号 | CPU | 架构 |
|------|-----|------|
| RT-AC86U | BCM4906 (ARM Cortex-A53) | armv7 |
| RT-AC68U | BCM4708 (ARM Cortex-A9) | armv7 |
| RT-AC3100 | BCM4709C0 | armv7 |
| RT-AX88U | BCM4908 (ARM Cortex-A53) | aarch64 |
| GT-AX11000 | BCM4908 | aarch64 |
| RT-AX86U | BCM4908 | aarch64 |

> 其他型号请通过 `uname -m` 确认架构后选择对应包。
