1. 我需要实现一款rust vpn的客户端，支持tuic，ss，vmless等常见协议的代理,gui使用rust-gpui，可以使用gpui-component组件，要求支持跨平台，windows，mac，linux都要支持，要适配rust shoes服务端。
2. 支持云端导入配置，支持karing，v2ray，clash等配置导入
3. 已知karing的配置https://rssgs.ei38d3if3.xyz/index/26a96a5629c7bcf27d9e612ab663335f包含可以使用的节点为马来西亚节点
4. 实现的软件可以减少管理员权限的依赖
5. 都是实现后可以通过核心代码支持linux路由器的部署
6. 先规划技术方案，落地为文档，逐渐渐进式的实现，并且每一步实现之后更新文档或者设计方案