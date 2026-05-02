# 优化指南

> 本文从代码审查和架构分析中提炼优化策略，每项均包含：**问题根源 → 影响评估 → 具体实现方案 → 推荐测试**。
> 所有条目已按"实施收益 / 实施成本"排序，优先处理高收益低成本的改进。

---

## 目录

1. [实施路线图（全局视图）](#1-实施路线图全局视图)
2. [BUG-FIX-1：Relay 统计字节修复](#2-bug-fix-1relay-统计字节修复)
3. [BUG-FIX-2：Router Cache 全量清除 → LRU](#3-bug-fix-2router-cache-全量清除--lru)
4. [BUG-FIX-3：Hysteria2 keep_alive 边界值修正](#4-bug-fix-3hysteria2-keep_alive-边界值修正)
5. [BUG-FIX-4：speed_test_node 测速端点修复](#5-bug-fix-4speed_test_node-测速端点修复)
6. [BUG-FIX-5：latency_ms 失败时写 None 而非 9999](#6-bug-fix-5latency_ms-失败时写-none-而非-9999)
7. [PERF-1：ConnPool max_age 降低与存活探测](#7-perf-1connpool-max_age-降低与存活探测)
8. [PERF-2：http_latency_test 复用 Keep-Alive 连接](#8-perf-2http_latency_test-复用-keep-alive-连接)
9. [PERF-3：Router 写锁降级（读改写 → 乐观写）](#9-perf-3router-写锁降级读改写--乐观写)
10. [STAB-1：UDP ASSOCIATE 返回正确错误码](#10-stab-1udp-associate-返回正确错误码)
11. [STAB-2：代理健康检查与自动重启](#11-stab-2代理健康检查与自动重启)
12. [STAB-3：DNS stale-while-revalidate inflight 共享](#12-stab-3dns-stale-while-revalidate-inflight-共享)
13. [FEAT-1：测速 GUI 集成](#13-feat-1测速-gui-集成)
14. [FEAT-2：订阅定时自动刷新](#14-feat-2订阅定时自动刷新)
15. [TEST-1：补充测试覆盖](#15-test-1补充测试覆盖)

---

## 1. 实施路线图（全局视图）

```mermaid
graph LR
    subgraph P1["第一批（Bug修复，低风险高收益）"]
        B1["BUG-FIX-1\nRelay 统计修复"]
        B2["BUG-FIX-2\nRouter LRU cache"]
        B3["BUG-FIX-3\nHy2 keep_alive"]
        B5["BUG-FIX-5\nlatency_ms = None"]
    end

    subgraph P2["第二批（性能优化）"]
        P21["PERF-1\nConnPool max_age"]
        P22["PERF-2\nlatency keep-alive"]
        P23["PERF-3\nRouter 锁降级"]
    end

    subgraph P3["第三批（稳定性）"]
        S1["STAB-1\nUDP ASSOCIATE"]
        S2["STAB-2\n健康检查"]
    end

    subgraph P4["第四批（功能）"]
        F1["FEAT-1\n测速 GUI"]
        F2["FEAT-2\n订阅自动刷新"]
        B4["BUG-FIX-4\nspeed_test 端点"]
    end

    B4 --> F1
    B1 --> F1
    B2 --> P22
    P21 --> S2
```

**优先级说明**：
- P1 Bug 修复对现有用户体验影响最大，且代码改动小（均 < 30 行）
- P2 性能优化在流量高峰时有感，但当前场景下用户感知可能有限
- P3/P4 为新功能或边缘情况改善

---

## 2. BUG-FIX-1：Relay 统计字节修复

**文件**：`crates/vex-core/src/proxy/relay.rs`

**根因**（见 [deep-analysis.mdx §3](./deep-analysis.mdx#3-relay-统计计数-bug跨-poll-丢失字节)）：
`transfer_one` 内的 `transferred: u64` 是局部变量，每次 `poll()` 重置为 0。当 `poll_write` 返回 `Pending`，已写字节数被丢弃。

**影响**：GUI 的 ↑/↓ 流量统计几乎永远为 0 或极小值，对大文件传输误差 > 99%。

### 实现方案

修改 `transfer_one` 签名，通过 `&mut u64` 参数跨 poll 累积：

```rust
// 修改前
fn transfer_one<R, W>(
    cx: &mut Context<'_>,
    reader: &mut R,
    writer: &mut W,
    buf: &mut CopyBuf,
    done: &mut bool,
) -> Poll<io::Result<u64>>

// 修改后
fn transfer_one<R, W>(
    cx: &mut Context<'_>,
    reader: &mut R,
    writer: &mut W,
    buf: &mut CopyBuf,
    done: &mut bool,
    total: &mut u64,      // ← 新增：跨 poll 的累积计数器
) -> Poll<io::Result<()>> // ← 返回值不再含字节数
```

在 `Relay` struct 中，`a_to_b` / `b_to_a` 本身充当累积计数器，直接传入：

```rust
// Relay::poll 中
let a_to_b_poll = if !this.a_done || this.a_buf.pos < this.a_buf.cap {
    transfer_one(cx, this.a, this.b, &mut this.a_buf, &mut this.a_done, &mut this.a_to_b)
} else {
    Poll::Ready(Ok(()))
};
```

在 `transfer_one` 内，每次写入成功立即累积：

```rust
fn transfer_one<R, W>(
    cx: &mut Context<'_>,
    reader: &mut R,
    writer: &mut W,
    buf: &mut CopyBuf,
    done: &mut bool,
    total: &mut u64,
) -> Poll<io::Result<()>> {
    loop {
        if buf.pos < buf.cap {
            let n = ready!(Pin::new(&mut *writer).poll_write(cx, &buf.buf[buf.pos..buf.cap]))?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::WriteZero, "write zero")));
            }
            buf.pos += n;
            *total += n as u64;   // ← 每次写入成功立即累积到外部
            if buf.pos == buf.cap { buf.pos = 0; buf.cap = 0; }
            continue;
        }
        if *done {
            ready!(Pin::new(&mut *writer).poll_flush(cx))?;
            return Poll::Ready(Ok(()));
        }
        let mut read_buf = ReadBuf::new(&mut buf.buf[..]);
        match ready!(Pin::new(&mut *reader).poll_read(cx, &mut read_buf)) {
            Ok(()) => {
                let n = read_buf.filled().len();
                if n == 0 {
                    *done = true;
                    ready!(Pin::new(&mut *writer).poll_shutdown(cx))?;
                    return Poll::Ready(Ok(()));
                }
                buf.cap = n;
            }
            Err(e) => return Poll::Ready(Err(e)),
        }
    }
}
```

### 测试

```rust
#[tokio::test]
async fn test_relay_byte_counting() {
    use tokio::io::duplex;

    let data = vec![42u8; 256 * 1024]; // 256 KB
    let (mut a_client, mut a_server) = duplex(65536);
    let (mut b_client, mut b_server) = duplex(65536);

    // 写入端
    tokio::spawn(async move {
        a_server.write_all(&data).await.unwrap();
        // a_server 关闭会触发 EOF
    });

    // 读取端（sink）
    tokio::spawn(async move {
        tokio::io::copy(&mut b_server, &mut tokio::io::sink()).await.unwrap();
    });

    let (a_to_b, b_to_a) = relay_bidirectional(&mut a_client, &mut b_client).await.unwrap();

    assert_eq!(a_to_b, 256 * 1024, "应统计 256KB");
    assert_eq!(b_to_a, 0);
}
```

---

## 3. BUG-FIX-2：Router Cache 全量清除 → LRU

**文件**：`crates/vex-core/src/router/engine.rs`

**根因**（见 [deep-analysis.mdx §1](./deep-analysis.mdx#1-路由-cache-的真实策略全量清除而非-lru)）：
`cache.clear()` 在容量满时清空所有 4096 条，造成大量缓存 miss 与写锁争用。

**影响**：在域名多样的代理场景（大量不同域名访问），定期触发"清空后全部重建"的惊群效应，表现为周期性延迟抖动。

### 实现方案

**Step 1**：在 `vex-core/Cargo.toml` 添加 `lru` 依赖：

```toml
[dependencies]
lru = "0.12"
```

**Step 2**：修改 `engine.rs`：

```rust
// 修改前
use std::collections::HashMap;
struct Router {
    cache: tokio::sync::RwLock<HashMap<String, RouteAction>>,
}
// 初始化
cache: tokio::sync::RwLock::new(HashMap::new()),
// 写入（line 158-162）
if cache.len() >= ROUTE_CACHE_CAP {
    cache.clear();  // ← 删除此块
}
cache.insert(host.to_string(), action.clone());

// 修改后
use lru::LruCache;
use std::num::NonZeroUsize;
struct Router {
    cache: tokio::sync::RwLock<LruCache<String, RouteAction>>,
}
// 初始化
cache: tokio::sync::RwLock::new(
    LruCache::new(NonZeroUsize::new(ROUTE_CACHE_CAP).unwrap())
),
// 读取（route() 快速路径）
if let Some(action) = cache.read().await.peek(host) {
    return action.clone();
}
// 写入（不再需要 len 检查，LruCache 自动驱逐）
cache.write().await.put(host.to_string(), action.clone());
```

> **注意**：`LruCache::get()` 会更新 LRU 顺序（需要 `&mut self`），因此读取路径需使用 `peek()`（只读引用）或将读锁升级为写锁。简单方案：读取时用写锁 + `get()`（有 LRU 更新语义）；性能敏感方案：读时用 `peek()`（读锁），写时用 `put()`（写锁，`RwLock<LruCache>` 不支持同一次 get+put 原子操作，但 peek + 写时 put 是安全的）。

### 测试

```rust
#[tokio::test]
async fn test_router_cache_lru_eviction() {
    let router = Router::new(empty_rules());
    // 插入 ROUTE_CACHE_CAP + 1 个不同域名
    for i in 0..=ROUTE_CACHE_CAP {
        router.route(&format!("host{}.example.com", i)).await;
    }
    // cache 大小不超过 ROUTE_CACHE_CAP
    let cache = router.cache.read().await;
    assert!(cache.len() <= ROUTE_CACHE_CAP);
}
```

---

## 4. BUG-FIX-3：Hysteria2 keep_alive 边界值修正

**文件**：`crates/vex-core/src/proxy/hysteria2.rs`

**根因**（见 [deep-analysis.mdx §7](./deep-analysis.mdx#7-quic-传输参数与-idle-timeout-含义)）：
Hysteria2 的 `keep_alive_interval = 15s`，`max_idle_timeout = 30s`。15s = 30s / 2，单次 PING 丢包就可能超过 idle timeout。

```mermaid
timeline
    title QUIC 保活时序对比
    section TUIC (安全)
        0s: 连接建立
        10s: PING 1
        20s: PING 2
        30s: PING 3 (max_idle_timeout 从上次成功起算)
    section Hysteria2 (边界)
        0s: 连接建立
        15s: PING 1
        30s: PING 2 (如果 PING 1 丢包, 此时恰好超时!)
    section Hysteria2 修复后
        0s: 连接建立
        10s: PING 1
        20s: PING 2
        30s: PING 3
```

### 实现方案

一行改动：

```rust
// hysteria2.rs - create_connection() 中
// 修改前
transport.keep_alive_interval(Some(Duration::from_secs(15)));

// 修改后
transport.keep_alive_interval(Some(Duration::from_secs(10)));  // 与 TUIC 一致
```

同时更新常量注释：

```rust
// QUIC transport: keep_alive < idle_timeout/2 → safe margin = 10s (TUIC 策略一致)
// max_idle_timeout = 30s, keep_alive = 10s → 3x 保活机会
```

---

## 5. BUG-FIX-4：speed_test_node 测速端点修复

**文件**：`crates/vex-core/src/proxy/speedtest.rs`

**根因**（见 [deep-analysis.mdx §4](./deep-analysis.mdx#4-speed_test_node-测速目标错误)）：
`/generate_204` 返回 HTTP 204 No Content，无响应体，读取字节数约 200 字节，计算的 `download_kbps` 永远约 0。

### 实现方案

```rust
// 修改前（speedtest.rs speed_test_node）
let dl_request =
    b"GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n";
let dl_host = "www.gstatic.com";

// 修改后
// Cloudflare speed test：返回指定字节数的随机数据，适合测速
const SPEED_TEST_HOST: &str = "speed.cloudflare.com";
const SPEED_TEST_BYTES: u64 = 5 * 1024 * 1024; // 5 MB
let dl_request = format!(
    "GET /__down?bytes={} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
    SPEED_TEST_BYTES, SPEED_TEST_HOST
).into_bytes();
let dl_host = SPEED_TEST_HOST;
```

并添加超时限制，避免慢速节点无限等待：

```rust
// 限时读取（10s 内读多少算多少）
let dl_timeout = Duration::from_secs(10);
let dl_start = Instant::now();
let mut total_bytes: u64 = 0;

loop {
    if dl_start.elapsed() >= dl_timeout { break; }
    match tokio::time::timeout(
        dl_timeout - dl_start.elapsed(),
        stream.read(&mut buf),
    ).await {
        Ok(Ok(n)) if n > 0 => total_bytes += n as u64,
        _ => break,
    }
}
```

---

## 6. BUG-FIX-5：latency_ms 失败时写 None 而非 9999

**文件**：`crates/vex-gui/src/app.rs`

**问题**：测试失败时 `node.latency_ms = Some(9999)` 被持久化到 config.yaml，下次启动仍显示 9999ms，用户看到节点"永久超时"。

**实现方案**：

```rust
// 在 test_node_latency 的结果回调中（app.rs）
// 修改前
let ms = result.unwrap_or(9999);
this.nodes[index].latency_ms = Some(ms);

// 修改后
match result {
    Ok(ms) => this.nodes[index].latency_ms = Some(ms),
    Err(_) => this.nodes[index].latency_ms = None,  // 失败不持久化，下次启动不显示过期结果
}
```

同时修改节点列表排序逻辑，使 `None`（未测试）排在已测试节点后面，`Some(9999)`（超时）排在最后：

```rust
// 排序键
fn sort_key(node: &Node) -> (u8, u32) {
    match node.latency_ms {
        None => (1, 0),          // 未测试，排中间
        Some(ms) => (0, ms),     // 已测试，按延迟升序
    }
}
```

---

## 7. PERF-1：ConnPool max_age 降低与存活探测

**文件**：`crates/vex-core/src/proxy/pool.rs`

**问题**：`max_age = 30s` 意味着最多可能使用一个 29.9s 的 TCP 连接。由于 TCP 连接在服务端可能在 30s 前就关闭（服务器 keepalive 超时），使用过期连接会导致第一次写入失败，触发 RetryOutbound 重连（浪费 200ms）。

### 方案 A：降低 max_age（低风险）

```rust
const MAX_AGE: Duration = Duration::from_secs(20);  // 30s → 20s
```

留出 10s 的安全裕量，避免在服务器端刚好超时时使用旧连接。

### 方案 B：TCP keepalive 探测（中等复杂度）

```rust
// pool.rs - get() 中，取出连接后进行轻量探测
async fn probe_connection(stream: &mut BoxProxyStream) -> bool {
    // 发送 0 字节写入触发 TCP 栈检测（非标准，OS 支持不一）
    // 或：检查连接是否可读（select! read + timeout(0)）
    let check = tokio::time::timeout(
        Duration::from_millis(10),
        // 尝试 peek：读取 0 字节，检测 EOF
        poll_fn(|cx| Pin::new(stream as &mut dyn AsyncRead).poll_read(cx, &mut ReadBuf::new(&mut []))),
    ).await;
    match check {
        Ok(Ok(())) => true,   // EOF = 0 字节，连接仍活跃
        Ok(Err(_)) => false,  // 错误，连接已断开
        Err(_) => true,       // 超时（10ms），没有数据，连接活跃
    }
}
```

**推荐**：先实施方案 A（一行改动），评估效果后再考虑方案 B。

---

## 8. PERF-2：http_latency_test 复用 Keep-Alive 连接

**文件**：`crates/vex-core/src/proxy/speedtest.rs`

**问题**：当前预热阶段建立连接后关闭，测量阶段再建立新连接（TCP+TLS 握手开销 50-150ms）。对 TCP 协议，预热几乎无意义。

### 实现方案

使用 `Connection: keep-alive`，预热后在同一连接上发送第二次请求：

```rust
pub async fn http_latency_test(outbound: &dyn Outbound) -> Result<u32> {
    // 单次连接，复用 keep-alive
    let mut stream = timeout(
        Duration::from_secs(HTTP_LATENCY_TIMEOUT_SECS),
        outbound.connect("www.gstatic.com", 80),
    ).await??;

    // 预热请求（同一连接）
    let warmup_req = b"GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: keep-alive\r\n\r\n";
    let _ = stream.write_all(warmup_req).await;
    let mut warmup_buf = [0u8; 256];
    let _ = timeout(Duration::from_secs(3), stream.read(&mut warmup_buf)).await;

    // 计时测量（同一连接，已预热）
    let start = Instant::now();
    let measure_req = b"GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n";
    stream.write_all(measure_req).await?;
    let mut measure_buf = [0u8; 256];
    stream.read(&mut measure_buf).await?;
    Ok(start.elapsed().as_millis() as u32)
}
```

**权衡**：QUIC 协议已自动受益（同一 QUIC session，预热建立 session，测量用新流）。TCP 协议现在也能消除 TCP 握手开销。

---

## 9. PERF-3：Router 写锁降级（读改写 → 乐观写）

**文件**：`crates/vex-core/src/router/engine.rs`

**问题**：当前 `route()` 在 cache miss 时：读锁（检查）→ 解锁 → 计算 action → 写锁（插入）。
这个流程是正确的，但在写锁期间其他读请求被阻塞。

### 优化思路

```rust
// 当前流程（读 → 解 → 算 → 写）
// 多个 goroutine 可能同时计算相同域名的 action，然后同时写入（重复计算但无误）

// 优化：写入前再次检查（双检锁）
async fn cache_and_return(cache: &RwLock<...>, host: &str, action: RouteAction) -> RouteAction {
    let mut w = cache.write().await;
    // 双检：可能其他线程已经写入了
    if let Some(existing) = w.peek(host) {
        return existing.clone();
    }
    w.put(host.to_string(), action.clone());
    action
}
```

**影响**：避免在写锁期间做重复的域名-action 映射（小优化，当前已有 RwLock，主要收益是减少重复写入）。

---

## 10. STAB-1：UDP ASSOCIATE 返回正确错误码

**文件**：`crates/vex-core/src/proxy/socks5.rs`

**问题**：SOCKS5 CMD_ASSOCIATE（UDP 中继）被接受（返回成功响应），但实际没有 UDP 转发逻辑。客户端收到"成功"后发送 UDP 数据，永远无响应，造成应用层超时或异常。

### 实现方案

**方案 A（快速，诚实）**：返回 "Command not supported" 错误码：

```rust
// socks5.rs
CMD_ASSOCIATE => {
    // 告知客户端不支持 UDP ASSOCIATE
    conn.write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await?;
    // REP=0x07: Command not supported
    return Ok(());
}
```

**方案 B（完整实现，高成本）**：实现真正的 UDP 中继（需要 UDP socket 绑定、数据报转发、NAT 表管理）。适合未来长期支持 UDP 应用（DNS、视频通话等直接 UDP 流量）。

**推荐**：先实施方案 A（防止客户端误解），再规划方案 B。

---

## 11. STAB-2：代理健康检查与自动重启

**文件**：`crates/vex-gui/src/app.rs`

**问题**：代理启动后，如果本地 SOCKS5/HTTP 端口无响应（服务崩溃、端口被占用），GUI 依然显示"Connected"。

### 实现方案

在 `start_proxy` 成功后，启动后台健康检查任务：

```rust
fn start_health_check(&self, cx: &mut Context<Self>, session_id: u64) {
    let handle = self.tokio_handle.clone();
    let socks_port = self.socks_port;
    let weak = cx.weak_handle();

    cx.spawn(async move |_cx| {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.tick().await;  // 跳过第一次（启动时刚验证过）

        loop {
            interval.tick().await;

            // 轻量探测：TCP connect + 立即关闭
            let alive = tokio::time::timeout(
                Duration::from_secs(3),
                TcpStream::connect(format!("127.0.0.1:{}", socks_port)),
            ).await.is_ok_and(|r| r.is_ok());

            if !alive {
                let _ = weak.update(&mut handle.enter(), |this, cx| {
                    if this.proxy_session_id == session_id {
                        tracing::warn!("Health check failed: local proxy port unresponsive");
                        this.proxy_validation_status = "⚠ Proxy health check failed".to_string();
                        cx.notify();
                        // 可选：触发自动重启
                        // this.restart_proxy_with_current_state(cx);
                    }
                });
            }
        }
    }).detach();
}
```

---

## 12. STAB-3：DNS stale-while-revalidate inflight 共享

**文件**：`crates/vex-core/src/dns/mod.rs`

**问题**（见 [deep-analysis.mdx §2](./deep-analysis.mdx#2-dns-cache-的真实策略真正的-lru)）：
stale-while-revalidate 的后台刷新创建了一个新的临时 `DnsResolver` 实例，不与主实例共享 inflight。可能导致同一域名同时有两个 DNS 查询（主实例 + 后台刷新实例）。

### 优化方案

将后台刷新改为调用主实例的 `resolve_uncached()`，并正确参与 inflight 去重：

```rust
// 当前：创建临时实例
let resolver_clone = DnsResolver { /* 新实例 */ };
tokio::spawn(async move { resolver_clone.resolve_uncached(domain).await; });

// 改后：持有对主实例的弱引用，通过主实例发起查询（参与 inflight 去重）
let self_arc = self.inner.clone();  // Arc<DnsResolverInner>
tokio::spawn(async move {
    let _ = DnsResolverInner::resolve_uncached(&self_arc, &domain).await;
});
```

**影响**：低优先级，实际场景中 DNS 并发冲突的概率很低，但消除它可提高 DNS 一致性。

---

## 13. FEAT-1：测速 GUI 集成

**依赖**：BUG-FIX-4（speed_test_node 端点修复）先完成。

**设计方案**：

```mermaid
sequenceDiagram
    participant UI as 节点列表 UI
    participant App as AppState
    participant Speed as speed_test_node()

    UI->>App: 点击"测速"按钮（单节点）
    App->>App: node.speed_kbps = None（清除旧值）
    App->>App: speed_testing.insert(index)
    App->>App: cx.notify()（显示 spinner）

    App->>Speed: spawn tokio task
    Speed-->>App: Ok(SpeedResult { upload_kbps, download_kbps })

    App->>App: node.speed_kbps = Some(download_kbps)
    App->>App: speed_testing.remove(index)
    App->>App: cx.notify()（显示 "X.X MB/s"）
```

**UI 展示**：在节点列表的延迟旁边增加一列"速度"，格式为 `X.X MB/s`。

**批量测速**：与批量延迟测试类似，使用 `Semaphore(cap=3)`（测速比延迟测试更耗带宽，适当降低并发）。

---

## 14. FEAT-2：订阅定时自动刷新

**文件**：`crates/vex-gui/src/app.rs`

### 设计方案

```mermaid
flowchart TD
    A["AppState::new()"] --> B{"subscriptions.is_empty()?"}
    B -->|"否"| C["spawn subscription_refresh_loop"]
    C --> D["sleep(refresh_interval, default=3600s)"]
    D --> E["fetch_subscription()"]
    E --> F{"成功?"}
    F -->|"是"| G["合并新节点\n（保留本地添加的节点）"]
    F -->|"否"| H["记录错误日志\n保留旧节点"]
    G & H --> I["cx.notify()"]
    I --> D
```

**关键设计**：
- 合并策略：以订阅节点名称为 key，新节点覆盖旧节点（保留用户手动添加的、名称不在订阅中的节点）
- 刷新间隔：可配置（默认 60 分钟），支持手动触发
- 错误处理：静默（不弹窗）；在 UI 上显示"上次刷新：X 分钟前"或"刷新失败"

---

## 15. TEST-1：补充测试覆盖

优先添加以下测试（按价值排序）：

### T1：relay 统计字节准确性（验证 BUG-FIX-1）

```rust
#[tokio::test]
async fn test_relay_bytes_counted_across_polls() {
    // 见 §2 中的测试代码
}
```

### T2：router cache LRU（验证 BUG-FIX-2）

```rust
#[tokio::test]
async fn test_router_cache_does_not_flush_all() {
    // 见 §3 中的测试代码
}
```

### T3：RetryOutbound 退避计时

```rust
#[tokio::test]
async fn test_retry_outbound_backoff() {
    let fail_count = Arc::new(AtomicUsize::new(0));
    let outbound = AlwaysFailOutbound { count: fail_count.clone() };
    let retry = RetryOutbound::new(outbound, 3);

    let start = Instant::now();
    let _ = retry.connect("dummy", 80).await;
    let elapsed = start.elapsed();

    // 3 次失败：0ms + 200ms + 400ms ≈ 600ms（加上协议超时，但无网络实际等待）
    assert!(elapsed >= Duration::from_millis(590), "应有退避等待");
    assert_eq!(fail_count.load(Ordering::Relaxed), 3, "应有 3 次尝试");
}
```

### T4：resolve_server_addrs IPv4 优先排序

```rust
#[test]
fn test_resolve_ipv4_first() {
    let addrs = vec![
        "::1:8080".parse().unwrap(),       // IPv6
        "127.0.0.1:8080".parse().unwrap(), // IPv4
    ];
    let sorted = sort_addrs_ipv4_first(addrs);
    assert!(sorted[0].is_ipv4(), "IPv4 应排在前");
}
```

### T5：ConnPool get() 并发安全

```rust
#[tokio::test]
async fn test_pool_concurrent_get() {
    let pool = ConnPool::new(mock_factory(), 4, Duration::from_secs(30));
    // 同时发起 20 个 get()，验证 pool 不超容量
    let handles: Vec<_> = (0..20).map(|_| {
        let p = pool.clone();
        tokio::spawn(async move { p.get().await })
    }).collect();
    for h in handles { let _ = h.await; }
    // 验证活跃连接数 ≤ capacity
}
```

---

## 优化效果预测

| 优化项 | 修改行数 | 用户可感知改善 | 技术风险 |
|--------|---------|-------------|---------|
| BUG-FIX-1 relay 统计 | ~30 行 | GUI 流量数字准确 | 低 |
| BUG-FIX-2 LRU cache | ~15 行 + 1 依赖 | 长期运行不抖动 | 低 |
| BUG-FIX-3 keep_alive | 1 行 | 空闲连接更稳定 | 极低 |
| BUG-FIX-4 测速端点 | ~20 行 | 测速功能可用 | 低 |
| BUG-FIX-5 latency None | ~5 行 | 节点状态更准确 | 极低 |
| PERF-1 max_age | 1 行 | 减少失效连接错误 | 低 |
| PERF-2 keep-alive | ~40 行 | 延迟测试更准 | 低 |
| STAB-1 UDP 错误码 | ~5 行 | UDP 应用不卡死 | 极低 |
| STAB-2 健康检查 | ~40 行 | 崩溃有视觉反馈 | 低 |
| FEAT-1 测速 GUI | ~80 行 | 新功能 | 中 |
| FEAT-2 订阅自动刷新 | ~60 行 | 新功能 | 中 |

---

*基于 commit `e5d05d8` 的代码分析*

---

> 基于对当前实现的深度分析，系统整理**性能优化**、**稳定性改进**、**功能缺口**和**已知 Bug**，并提出优先级建议。

---

## 目录

- [性能优化机会](#性能优化机会)
- [稳定性改进](#稳定性改进)
- [功能缺口](#功能缺口)
- [已知 Bug / Edge Case](#已知-bug--edge-case)
- [优先级总表](#优先级总表)

---

## 性能优化机会

### P1：路由 LRU 缓存未实现驱逐策略

**位置**：`crates/vex-core/src/router/engine.rs`

**问题**：注释声称 cache 是"4096 entry LRU"，但实际实现是普通 `HashMap`：

```rust
cache: RwLock<HashMap<String, RouteAction>>,  // 当前实现
```

没有条目驱逐逻辑。在节点较多、域名多样的使用场景（如用作公共代理）下，cache 会无限增长，造成内存泄漏。

**建议**：引入 `lru` crate（或手写 `LinkedHashMap`），限制 cache 大小为 4096 并驱逐最久未用条目。

```toml
# Cargo.toml
lru = "0.12"
```

```rust
use lru::LruCache;
cache: RwLock<LruCache<String, RouteAction>>,
// 初始化：LruCache::new(NonZeroUsize::new(4096).unwrap())
```

---

### ~~P1：DNS 缓存同样缺少 LRU 驱逐~~（经代码审查，已正确实现，移除此项）

经代码审查（`dns/mod.rs` 第 204-217 行），DNS cache 已正确实现：先 `retain` 清过期条目，再 `min_by_key(last_used)` 驱逐最久未用条目。详见 [deep-analysis.mdx §2](./deep-analysis.mdx)。

---

### P2：ConnPool 容量偏小（TCP 协议）

**位置**：`crates/vex-core/src/proxy/pool.rs`

**当前**：`capacity = 4`，`max_age = 30s`

**问题**：在高并发场景（如浏览器同时打开多个标签页），4 个预建连接可能不足，导致频繁的 TCP+TLS 握手。

**建议**：
- 增加默认容量至 8-16
- 允许通过 config.yaml 配置
- 考虑根据协议类型区分容量（VMess 可能需要更多）

---

### P2：http_latency_test 预热连接未复用

**位置**：`crates/vex-core/src/proxy/speedtest.rs`

**当前**：预热阶段建立连接，读取响应，然后**关闭**连接，再建立第二个连接计时。

**问题**：对 TCP 协议，每次都付出 TCP+TLS 握手成本（约 50-150ms）。预热连接关闭后，测量连接重新握手，预热的价值大打折扣。

**建议**：对 TCP 协议，在同一个连接上先读预热响应，然后直接发第二次请求计时（Connection: keep-alive）。QUIC 协议已受益（预热建立 QUIC session，测量用新 stream）。

---

### P2：批量延迟测试的 spawn 效率

**位置**：`crates/vex-gui/src/app.rs` — `test_all_latency()`

**当前**：

```rust
fn test_all_latency(&mut self, cx: &mut Context<Self>) {
    for i in 0..self.nodes.len() {
        self.test_node_latency(i, cx);  // 依次 spawn，无 batch
    }
}
```

**问题**：100 个节点时，产生 100 个 GPUI task，每个 task 内部再 spawn tokio task。层级过多，有调度开销。

**建议**：预先过滤已在测试中的节点（`latency_testing.contains(i)` skip），减少无效任务。或者一次性 spawn 单个 tokio task 管理所有节点的测试 Future，通过 FuturesUnordered 驱动。

---

### P3：TLS 根证书有两份独立的 OnceLock

**位置**：`transport.rs`（craftls 路径）和 `hysteria2.rs` / `tuic.rs`（rustls 0.23 路径）

**问题**：两套 TLS 栈各自缓存一份 webpki 根证书（约 200KB），浪费内存。

**建议**：将根证书提取到 `vex-core` 的公共模块，各协议共享引用。

---

### P3：ProxyStats 字节计数时机

**位置**：relay.rs + socks5.rs/http.rs

**当前**：`add_bytes()` 在 relay 结束后一次性更新（返回 `(bytes_up, bytes_down)`），不是实时的。

**建议**：如果需要实时流量监控（如速率显示），需改为在 relay 循环中每次 write 后调用。当前对"总流量统计"足够，无需改动；但 GUI 如果后续要显示实时速率则需重构。

---

## 稳定性改进

### P1：ConnPool max_age 与服务器 keepalive 不匹配

**位置**：`pool.rs` — `max_age = 30s`

**问题**：如果服务端 TCP keepalive 或 idle timeout < 30s，池中的连接可能已被服务端关闭，但客户端不知道（TCP 半关闭）。`pool.get()` 返回死连接，导致首次请求失败。

**现状**：TCP-based 协议有 `RetryOutbound(3)` 兜底，失败后重试能绕过此问题。但仍会增加延迟（200ms-2s）。

**建议**：
1. 降低 `max_age` 至 15-20s（更保守）
2. 或在 `pool.get()` 时做存活探测（发送空数据包检查）

---

### P1：open_bi 重试仅 2 次

**位置**：`tuic.rs` / `hysteria2.rs` — connect() 中的 open_bi 重试循环

**当前**：最多 2 次 attempt（1 次正常 + 1 次 stale 连接检测后重连）。

**边界情况**：如果 stale 连接重连后，新连接也因为服务器刚重启而立即超时，用户仍然看到错误。

**建议**：考虑增加到 3 次，或在 `create_connection` 失败时提供更清晰的错误信息（"服务器重启中，请重试"而非通用错误）。

---

### P1：verify_local_http_proxy 超时分配不均匀

**位置**：`app.rs` — `verify_local_http_proxy_once()`

**当前**：6s 整体超时中：
- Phase 1 (TCP connect to local): 2s 硬编码
- Phase 2 (CONNECT tunnel): 剩余时间
- Phase 3 (TLS handshake): 剩余时间

**问题**：对国际节点，TCP 到本地代理很快（<1ms），但 CONNECT 隧道建立可能需要 3-4s，TLS 握手可能再需要 1-2s。如果 CONNECT 刚好用了 4s，TLS 只剩 <2s，可能超时，导致误报"验证失败"。

**建议**：调整 Phase 1 超时为 500ms（本地连接应该极快），把更多时间留给 Phase 2+3；或使用独立超时而非共享。

---

### P2：SOCKS5 CMD_ASSOCIATE 响应不规范

**位置**：`socks5.rs`

**当前**：收到 CMD_ASSOCIATE 后返回固定 BND_ADDR（127.0.0.1:0 或服务器绑定地址），实际 UDP 转发未实现。部分 UDP 应用（如游戏、DNS over SOCKS5）会接受 CONNECT 失败，但某些客户端认为 ASSOCIATE 成功后直接发 UDP，导致静默丢包。

**建议**：要么正确实现 UDP ASSOCIATE，要么明确返回 `REP=0x07`（Command not supported）让客户端知道不支持。

---

### P2：系统代理清除逻辑

**位置**：`app.rs` + `main.rs` — 退出时 cleanup

**当前**：退出时总是调用 `sysproxy::Sysproxy::set_enabled(false)`，无论 app 是否曾设置系统代理。

**问题**：如果用户手动在系统设置中配置了代理（不通过 app），app 退出时会意外清除用户的代理配置。

**建议**：仅当 `system_proxy_managed_by_app == true` 时才清除系统代理。当前代码路径应当已有此判断，但需验证 panic hook 路径是否也正确判断了此标志。

---

### P2：reconnect 期间的并发连接请求

**位置**：`tuic.rs` / `hysteria2.rs` — `get_connection()` 写锁路径

**场景**：多个 tokio task 同时调用 `get_connection()`，发现连接已关闭，全部排队等待写锁。第一个 task 获得写锁并成功重连；后续 task 获得写锁时会双检发现连接已存在，但**仍然各自执行了一次 `authenticate()`**（如果 create_connection 和 authenticate 在写锁外执行）。

**需要验证**：确认当前实现中 `authenticate()` 是否在写锁持有期间调用（安全），还是在写锁外调用（可能重复认证）。

---

### P3：latency_ms = 9999 不区分"失败"和"极慢"

**位置**：`app.rs` — test_node_latency 结果处理

**当前**：测试失败时存储 `9999ms`，UI 显示为"9999 ms"或"Timeout"。

**问题**：从 UI 无法区分"完全不可达"vs."可达但延迟高于 10s（极端情况）"。此外，9999ms 会被 `sort_nodes_by_latency()` 排到底部，这是正确的，但 UI 反馈不够清晰。

**建议**：
- 用 `None` 表示"未测试"，用特殊值（如 `u32::MAX`）表示"测试失败"
- UI 区分展示：未测试显示 "-"，失败显示 "✗"，成功显示数字

---

## 功能缺口

### P1：UDP 代理未实现

**现状**：TUIC v5 和 Hysteria2 协议在协议层面支持 UDP 转发，node 配置也有 `udp: bool` 字段，但：
1. SOCKS5 服务器 CMD_ASSOCIATE 未实现
2. TUIC/Hysteria2 的 UDP 帧发送/接收逻辑未实现

**影响**：DNS over proxy、UDP 应用（游戏、VoIP）无法工作，尽管协议支持。

**实现路径**：
1. Socks5Server 实现 UDP ASSOCIATE + 本地 UDP 中转
2. TuicOutbound 实现 UDP packet 封装（TUIC v5 UDP 帧格式）
3. Hysteria2Outbound 实现 UDP 封装

---

### P1：无代理健康检查

**现状**：代理启动后只有一次验证（`verify_local_http_proxy`），之后不再检测。

**问题**：代理进程崩溃或 QUIC 连接永久断开后，用户无感知，继续以为代理在运行（流量实际失败）。

**建议**：
- 每隔 30-60s 执行轻量健康检查（TCP ping 到本地代理端口）
- 检测到代理不可用时：自动重启 or 显示警告
- `ProxyService` 通过 `watch::Receiver<ProxyState>` 可检测 server task 是否 panic

---

### P1：订阅无定时自动刷新

**现状**：订阅节点仅在 app 启动时或用户手动点击"刷新"时更新。

**建议**：
- 添加 `subscription_refresh_interval: Option<Duration>` 配置项（如 6h）
- 在后台 spawn 定时任务，到期自动刷新并 merge 节点

---

### P2：无连接级日志

**现状**：Log 视图显示 tracing 输出（协议握手、错误），但无法看到"哪些请求通过了代理"、"哪些被路由到直连"。

**建议**：在 SOCKS5/HTTP handle 函数中记录：
```
[PROXY] google.com:443 → TUIC Node1 (123ms)
[DIRECT] baidu.com:443 → Direct
[REJECT] ads.example.com:443 → Rejected
```

---

### P2：无速率/吞吐量测试

**现状**：延迟测试 (`http_latency_test`) 只测延迟，`speed_test_node()` 函数在 speedtest.rs 中存在但 GUI 中未集成。

**建议**：在节点列表中增加"测速"按钮，调用 `speed_test_node()` 并展示下载速率（Mbps）。

---

### P2：无 IPv6 偏好设置（GUI）

**现状**：TUIC/Hysteria2 已实现 IPv4 优先（`resolve_server_addrs` 排序），但用户无法在 GUI 中切换为 IPv6 优先。某些 ISP IPv6 质量更好。

**建议**：在 Settings 中增加"IPv6 优先"开关，传递到 `resolve_server_addrs` 排序逻辑。

---

### P2：无本地代理认证

**现状**：SOCKS5/HTTP 本地服务无用户名/密码认证。

**风险**：如果 `listen_addr = "0.0.0.0"`（或局域网可访问），任何局域网设备均可使用代理。

**建议**：
- 增加可选的 SOCKS5 用户名/密码认证（RFC 1929）
- 增加 HTTP Proxy-Authorization 头检查
- 默认仅监听 `127.0.0.1`（当前默认，但用户可能误改）

---

### P3：无代理链（Proxy Chain）

**现状**：流量：本地 → 远端代理 → 目标。不支持：本地 → 代理A → 代理B → 目标。

**使用场景**：通过不同出口国家的二级代理访问特定地区内容。

**建议**：在 factory.rs 中允许 `outbound` 本身使用另一个 outbound 作为 transport（嵌套 outbound）。

---

### P3：GUI 无自定义 DNS 设置入口

**现状**：DNS 配置（DoH、分流等）只能通过手动编辑 `config.yaml` 修改，GUI 中没有对应的设置面板。

**建议**：在 Settings 视图增加 DNS 配置区域：
- 主/备 DNS 服务器
- 是否启用 DoH
- 国内外分流开关

---

### BUG-1：路由 cache 策略：全量清除（非 LRU），触发惊群效应

**位置**：`router/engine.rs` 第 158-162 行

**真实行为**：当 cache 达到 4096 条时，`cache.clear()` 清除**所有**条目（不是驱逐 1 条），导致：
- 下次 4096 个请求全部 cache miss
- 多个线程同时写入 cache（写锁争用）
- 在域名多样的代理场景，此 flush 会周期性触发

严重程度：**中**（周期性性能抖动）

**修复**：引入 `lru` crate，使用 `LruCache::new(4096)`，每次插入自动驱逐 LRU 条目。

---

### BUG-2：relay 统计字节计数几乎永远为 0

**位置**：`relay.rs` — `transfer_one()` 和 `Relay::poll()`

**根因**：`transfer_one` 内部的 `transferred` 是局部变量。每次 `ready!` 宏遇到 `Poll::Pending`，函数立即返回，局部 `transferred` 被丢弃。`Relay::poll` 中只在 `Poll::Ready(Ok(n))` 时累积 `this.a_to_b += n`，但 `transfer_one` 只在完全结束（reader EOF + flush）时才返回 `Ready`。

**效果**：`stats.add_bytes()` 收到的字节数是最后一次 flush 的字节数（接近 0），而非实际传输总量。GUI 中的 ↑/↓ 流量显示**基本无效**。

严重程度：**中**（统计功能完全失效，但不影响代理正确性）

**修复**：修改 `transfer_one` 签名，接受 `&mut u64` 参数在每次 `poll_write` 成功后立即累积，跨 poll 保持。详见 [deep-analysis.mdx §3](./deep-analysis.mdx)。

---

### BUG-3：speed_test_node 下载速率永远为 0

**位置**：`speedtest.rs` — `speed_test_node()`

**根因**：测速目标为 `GET /generate_204`，此端点返回 `204 No Content`（无响应体），读取字节数约 200 字节（仅 HTTP 头），`download_kbps` 计算结果约 0 KB/s。

严重程度：**低**（此函数未在 GUI 集成，影响范围有限）

**修复**：将测速目标改为有实际内容的端点（如 `speed.cloudflare.com/__down?bytes=10485760`）。

---

### BUG-4：DNS inflight 条目在所有 caller 被 cancel 时不被清理

**位置**：`dns/mod.rs` — `resolve()` 方法

**描述**：cleanup 在 `get_or_init` 之后执行。如果所有等待该域名的 caller 都被 cancel（timeout），inflight HashMap 中的 Arc<OnceCell> 永久存活（无引用计数降到 0）。下次查询同一域名会复用此旧 OnceCell，行为正确但有轻微内存开销。

严重程度：**低**（域名集合有限，DNS 查询很少被 cancel）

---

### BUG-5（原 BUG-3）：pool.fill() 并发正确性——已确认无 Bug

经代码审查，`schedule_fill()` 的 `filling` 原子变量确保同一时刻只有一个 fill() 运行，`fill()` 内的二次检查防止超额填充。**此项移除**。

### BUG-6：HTTP 代理未过滤 hop-by-hop 头

**位置**：`http.rs` — HTTP GET/POST 转发路径

**描述**：HTTP/1.1 规范要求代理删除 hop-by-hop 头（`Proxy-Connection`, `Proxy-Authorization`, `Transfer-Encoding`, `TE`, `Trailer`, `Upgrade`, `Connection`）。当前直接转发所有头，可能导致部分 HTTP/1.1 服务器行为异常。

**影响**：HTTPS CONNECT 隧道（主要路径）不受影响；仅影响 HTTP 明文转发。

---

### BUG-7：node.latency_ms 测试失败后永久显示 9999

**位置**：`app.rs` — test_node_latency 结果写入 + persist

**描述**：测试失败时设置 `node.latency_ms = Some(9999)` 并 `persist_gui_state()`，保存到 config.yaml。下次启动后，节点仍显示 9999ms，直到再次测试。

**影响**：临时网络故障导致节点被永久标记为"超时"状态，排序时排到末尾，用户体验不佳。

**建议**：测试失败时设 `node.latency_ms = None`（而非 9999），或持久化时过滤掉失败结果。

---

### BUG-8：restart_proxy_with_current_state 无上限重试

**位置**：`app.rs` — restart_proxy_with_current_state()

**当前**：

```rust
for _ in 0..20 {
    tokio::time::sleep(Duration::from_millis(100)).await;
    // 尝试 restart...
}
```

20 次 × 100ms = 2s 内尝试 20 次。如果每次都触发 QUIC 握手（7s 超时），可能排队堆积。

**建议**：添加指数退避，或在连续失败 N 次后放弃并显示错误。

---

### BUG-9：TUN emergency_restore_routes 竞态

**位置**：`main.rs` — panic hook + Ctrl+C handler

**描述**：panic hook 和 Ctrl+C handler 都调用 `emergency_restore_routes()`，两者可能并发执行（如 panic 发生在 Ctrl+C 处理期间），导致路由命令执行两次。在某些 OS 上重复删除路由会返回错误，可能被误解为"路由未恢复"。

**建议**：使用 `AtomicBool` 确保 cleanup 只执行一次。

---

## 优先级总表

| 编号 | 类别 | 描述 | 优先级 | 难度 |
|------|------|------|--------|------|
| P-1 | 性能/Bug | 路由 cache：全量清除 → LRU | P1 | 低 |
| ~~P-2~~ | ~~性能~~ | ~~DNS cache LRU 驱逐~~ | ~~P1~~ | ~~低~~ |
| P-3 | 稳定性 | ConnPool max_age 降低/存活探测 | P1 | 中 |
| P-4 | 稳定性 | verify 超时分配调整 | P1 | 低 |
| P-5 | 功能 | UDP ASSOCIATE 实现 | P1 | 高 |
| P-6 | 功能 | 代理健康检查 + 自动重启 | P1 | 中 |
| P-B2 | Bug | relay 统计字节计数修复 | P1 | 中 |
| P-B3 | Bug | speed_test_node 测速端点修复 | P1 | 低 |
| P-8 | 稳定性 | SOCKS5 CMD_ASSOCIATE 返回正确错误码 | P2 | 低 |
| P-9 | 性能 | ConnPool 容量可配置 | P2 | 低 |
| P-10 | 功能 | 订阅定时自动刷新 | P2 | 中 |
| P-11 | 功能 | 连接级日志 | P2 | 低 |
| P-12 | 功能 | 速率测试 GUI 集成 | P2 | 中 |
| P-13 | 功能 | 本地代理认证 | P2 | 中 |
| P-B4 | Bug | DNS inflight cancel 清理（低风险） | P3 | 低 |
| P-B7 | Bug | latency_ms 失败后 None 而非 9999 | P2 | 低 |
| P-B6 | Bug | HTTP hop-by-hop 头过滤 | P2 | 低 |
| P-17 | 稳定性 | open_bi 重试增加到 3 次 | P3 | 低 |
| P-18 | 功能 | IPv6 优先开关（GUI） | P3 | 低 |
| P-19 | 功能 | DNS 自定义设置（GUI） | P3 | 中 |
| P-20 | 功能 | 代理链支持 | P3 | 高 |
| P-B9 | Bug | TUN cleanup 单次执行保证 | P3 | 低 |
| P-22 | Bug | system proxy 仅 managed 时才清除（验证） | P3 | 低 |
| P-23 | 性能 | TLS 根证书统一缓存 | P3 | 低 |
| P-24 | 性能 | http_latency_test 复用 keep-alive 连接 | P3 | 中 |

---

*分析基于 commit `3646ea7`*
