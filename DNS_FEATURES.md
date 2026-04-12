# DNS 拦截功能文档

## 功能概述

xTune 现已支持 DNS 查询拦截和重定向。可以根据域名规则对 DNS 查询进行过滤、重定向或阻止。

## 核心模块

### 1. DNS 规则引擎 (`src/dns/mod.rs`)

#### DnsRuleAction 类型

```rust
pub enum DnsRuleAction {
    Redirect(IpAddr),  // 重定向到指定 IP
    Block,             // 阻止查询
    Pass,              // 通过（默认）
}
```

#### 规则定义

```rust
pub struct DnsRule {
    pub pattern: String,      // 域名模式
    pub action: DnsRuleAction // 执行的动作
}
```

#### 模式匹配规则

- `example.com` - 精确匹配
- `*.example.com` - 匹配子域名（如 sub.example.com）
- `*` - 匹配所有域名

#### 使用示例

```rust
// 创建规则引擎
let mut engine = DnsRuleEngine::new();

// 添加阻止广告域名的规则
engine.add_rule(DnsRule {
    pattern: "*.ads.com".to_string(),
    action: DnsRuleAction::Block,
});

// 添加重定向规则
engine.add_rule(DnsRule {
    pattern: "*.example.com".to_string(),
    action: DnsRuleAction::Redirect("192.168.1.1".parse().unwrap()),
});

// 查询匹配的规则
if let Some(action) = engine.match_rule("sub.ads.com") {
    println!("匹配到规则: {:?}", action);
}
```

### 2. DNS 数据包解析 (`src/dns/parser.rs`)

#### DnsPacket 结构

```rust
pub struct DnsPacket {
    pub id: u16,                    // DNS 查询 ID
    pub is_query: bool,             // 是否为查询（true）或响应（false）
    pub questions: Vec<DnsQuestion>, // DNS 问题
    pub answers: Vec<DnsAnswer>,     // DNS 回答
    pub raw: Vec<u8>,               // 原始数据
}
```

#### 支持的查询类型

| 类型 | 代码 | 说明 |
|------|------|------|
| A | 1 | IPv4 地址 |
| AAAA | 28 | IPv6 地址 |
| CNAME | 5 | 别名 |
| MX | 15 | 邮件交换 |
| NS | 2 | 名称服务器 |
| TXT | 16 | 文本记录 |

#### DNS 查询解析

```rust
let dns_packet = DnsPacket::parse(&packet_data)?;

// 检查是否为 DNS 查询
if dns_packet.is_query {
    // 获取第一个查询的域名
    if let Some(domain) = dns_packet.get_first_domain() {
        println!("DNS 查询: {}", domain);
    }
}
```

## 工作流程

```
UDP 包（端口 53）
       ↓
DNS 数据包解析
       ↓
提取域名信息
       ↓
规则引擎匹配
       ↓
执行规则动作:
├─ Redirect: 返回新的 IP 地址
├─ Block: 返回 NXDOMAIN 错误
└─ Pass: 转发到上游 DNS
       ↓
日志记录和统计
```

## 集成到代理

DNS 拦截自动集成在 UDP 处理中：

```rust
async fn handle_udp(&self, packet: IpPacket) {
    match packet.dst_port {
        Some(53) => {
            // DNS 查询处理
            if let Some(dns_packet) = DnsPacket::parse(&packet.payload) {
                if let Some(domain) = dns_packet.get_first_domain() {
                    log::info!("[DNS] Query: {} (ID: {})", domain, dns_packet.id);
                }
            }
        }
        // ... 其他 UDP 处理
    }
}
```

## 配置示例

### 阻止跟踪域名

```rust
let mut engine = DnsRuleEngine::new();

let rules = vec![
    DnsRule {
        pattern: "*.doubleclick.net".to_string(),
        action: DnsRuleAction::Block,
    },
    DnsRule {
        pattern: "*.google-analytics.com".to_string(),
        action: DnsRuleAction::Block,
    },
];

engine.load_rules(rules);
```

### 本地开发环境

```rust
let rules = vec![
    DnsRule {
        pattern: "*.local".to_string(),
        action: DnsRuleAction::Redirect("127.0.0.1".parse().unwrap()),
    },
    DnsRule {
        pattern: "dev.example.com".to_string(),
        action: DnsRuleAction::Redirect("192.168.1.100".parse().unwrap()),
    },
];
```

### 默认规则

程序包含内置的默认规则：

```rust
pub fn create_default_rules() -> Vec<DnsRule> {
    vec![
        // 阻止广告域名
        DnsRule {
            pattern: "*.doubleclick.net".to_string(),
            action: DnsRuleAction::Block,
        },
        // 阻止分析跟踪
        DnsRule {
            pattern: "*.google-analytics.com".to_string(),
            action: DnsRuleAction::Block,
        },
        // 重定向恶意广告
        DnsRule {
            pattern: "*.ads.com".to_string(),
            action: DnsRuleAction::Redirect("127.0.0.1".parse().unwrap()),
        },
    ]
}
```

## 测试

运行 DNS 相关测试：

```bash
cargo test dns
```

测试覆盖：
- ✅ DNS 查询包解析
- ✅ DNS 响应包解析
- ✅ 查询类型转换
- ✅ 域名模式匹配
- ✅ 规则引擎匹配

## 日志输出

DNS 查询会产生如下日志：

```
[DNS] Query: google.com (ID: 1234)
[DNS] Query: ads.example.com (ID: 1235)
```

## 性能特性

- **匹配速度**：O(n) - n 为规则数
- **内存占用**：每条规则 ~200 字节
- **支持的规则数**：无限制（受内存限制）

## 限制和已知问题

1. **DNS 压缩**
   - 当前实现对 DNS 消息压缩支持有限
   - 大多数常见的 DNS 包可正常解析

2. **DNS 响应生成**
   - 当前仅支持查询检测和日志
   - 响应包的生成需要在后续版本实现

3. **性能优化**
   - 规则匹配未采用 Trie 结构
   - 可通过规则排序优化常用规则的匹配速度

4. **监听接口**
   - 仅支持 UDP 53 端口的 DNS 查询
   - TCP DNS（port 53）暂不支持

## 下一步计划

### 短期
- [ ] 生成 DNS 响应包
- [ ] 支持 TCP DNS (port 53)
- [ ] 配置文件规则加载

### 中期
- [ ] 使用 Trie 结构优化匹配
- [ ] 缓存常用查询结果
- [ ] 统计被拦截的查询数

### 长期
- [ ] WebUI 规则管理
- [ ] DNS 黑名单更新
- [ ] 自定义 DNS 响应内容

## 调试技巧

### 启用详细日志

```bash
RUST_LOG=debug cargo run
```

### 过滤 DNS 日志

```bash
tail -f xtune.log | grep DNS
```

### 测试 DNS 查询

```bash
# Linux/macOS
nslookup example.com

# 或使用 dig
dig example.com

# 或使用 host
host example.com
```

## API 参考

### DnsRuleEngine

```rust
pub fn new() -> Self
pub fn add_rule(&mut self, rule: DnsRule)
pub fn load_rules(&mut self, rules: Vec<DnsRule>)
pub fn match_rule(&self, domain: &str) -> Option<DnsRuleAction>
pub fn get_rules_count(&self) -> usize
```

### DnsPacket

```rust
pub fn parse(data: &[u8]) -> Option<Self>
pub fn is_dns_query(&self) -> bool
pub fn get_first_domain(&self) -> Option<String>
```

## 示例代码

完整的 DNS 规则配置示例：

```rust
use xTune::dns::{DnsRuleEngine, DnsRule, DnsRuleAction};

fn setup_dns_rules() -> DnsRuleEngine {
    let mut engine = DnsRuleEngine::new();

    // 阻止广告
    engine.add_rule(DnsRule {
        pattern: "*.ads.*.com".to_string(),
        action: DnsRuleAction::Block,
    });

    // 重定向恶意域名
    engine.add_rule(DnsRule {
        pattern: "*.malware.com".to_string(),
        action: DnsRuleAction::Redirect("127.0.0.1".parse().unwrap()),
    });

    // 本地开发
    engine.add_rule(DnsRule {
        pattern: "*.local".to_string(),
        action: DnsRuleAction::Redirect("192.168.1.100".parse().unwrap()),
    });

    engine
}
```

---

更多信息请参考 [PROXY_FEATURES.md](PROXY_FEATURES.md) 和 [QUICKSTART.md](QUICKSTART.md)
