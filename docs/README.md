# Vex 文档

Vex 是一款基于 Rust 的跨平台代理客户端，支持 Shadowsocks、VMess、VLess、TUIC、Trojan、Hysteria2 等协议。

## 快速导航

### 用户文档
| 文档 | 说明 |
|------|------|
| [快速开始](./getting-started.md) | 安装、配置与第一次运行 |
| [配置参考](./configuration.md) | 配置文件格式与所有选项说明 |
| [协议支持](./protocols.md) | 支持的代理协议与传输层详情 |

### 开发文档
| 文档 | 说明 |
|------|------|
| [开发指南](./development.md) | 编译、测试与贡献指南 |
| [技术架构](./architecture.md) | 模块设计、数据流与关键决策 |
| [项目路线图](./roadmap.md) | 功能规划与开发进度 |

| [梅林路由器插件](./merlin.md) | 在 AsusWRT-Merlin 路由器上部署透明代理 |

### 深度技术文档（面向贡献者）
| 文档 | 说明 |
|------|------|
| [内部实现详解](./internals/implementation.md) | 协议实现、连接生命周期、数据流 |
| [深度技术分析](./internals/analysis.md) | 并发模型、安全分析、Bug 分析 |
| [优化指南](./internals/optimization.md) | 已知问题修复与性能优化方案 |

## 项目结构

```
vex/
├── Cargo.toml                 # Workspace 根配置
├── crates/
│   ├── vex-core/              # 核心库（协议、配置、路由、DNS）
│   ├── vex-gui/               # GUI 客户端（bin: vex）
│   ├── vex-cli/               # CLI 客户端（bin: vex-cli）
│   └── craftls/               # 定制 rustls（TLS 指纹伪装）
├── config.yaml.example        # 配置示例
├── README.md                  # 项目主页
└── docs/                      # 文档（当前目录）
```
