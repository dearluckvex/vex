<!DOCTYPE html>
<html>
<head>
<meta http-equiv="Content-Type" content="text/html; charset=utf-8">
<meta http-equiv="Pragma" content="no-cache">
<meta http-equiv="Expires" content="-1">
<link rel="shortcut icon" href="images/favicon.png">
<title>Vex Proxy</title>
<style>
* { box-sizing: border-box; }
body { font-family: Arial, sans-serif; background: #f0f2f5; margin: 0; padding: 0; font-size: 13px; }
.header { background: #1a1a2e; color: #fff; padding: 10px 20px; display: flex; align-items: center; gap: 10px; }
.header h1 { margin: 0; font-size: 17px; }
.header .sub { font-size: 11px; opacity: .6; }
.nav { background: #16213e; display: flex; padding: 0 16px; }
.nav-item { color: #aaa; padding: 10px 16px; cursor: pointer; font-size: 13px; border-bottom: 3px solid transparent; user-select: none; }
.nav-item:hover { color: #fff; }
.nav-item.active { color: #e94560; border-bottom-color: #e94560; font-weight: bold; background: rgba(255,255,255,.04); }
.content { padding: 14px; max-width: 960px; margin: 0 auto; }
.card { background: #fff; border-radius: 8px; padding: 14px 16px; margin-bottom: 12px; box-shadow: 0 1px 3px rgba(0,0,0,.08); }
.card h3 { margin: 0 0 10px; font-size: 11px; color: #888; border-bottom: 1px solid #f0f0f0; padding-bottom: 7px; text-transform: uppercase; letter-spacing: .5px; }
.dot { display: inline-block; width: 8px; height: 8px; border-radius: 50%; margin-right: 5px; flex-shrink: 0; vertical-align: middle; }
.dot.running { background: #27ae60; box-shadow: 0 0 4px #27ae60; }
.dot.stopped { background: #e74c3c; }
.dot.ok  { background: #27ae60; }
.dot.warn { background: #f39c12; }
.dot.off { background: #bbb; }
.info-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 8px; margin: 10px 0 4px; }
.info-item { background: #f8f9fb; border-radius: 6px; padding: 8px 10px; text-align: center; }
.info-item .label { font-size: 10px; color: #999; text-transform: uppercase; letter-spacing: .3px; }
.info-item .value { font-size: 16px; font-weight: bold; color: #333; margin-top: 3px; }
.btn { padding: 6px 14px; border: none; border-radius: 5px; cursor: pointer; font-size: 12px; font-weight: bold; transition: opacity .15s; }
.btn:hover { opacity: .82; }
.btn-success { background: #27ae60; color: #fff; }
.btn-danger  { background: #e74c3c; color: #fff; }
.btn-primary { background: #2980b9; color: #fff; }
.btn-sm { padding: 3px 9px; font-size: 11px; }
select, input[type=text], input[type=url] { padding: 6px 9px; border: 1px solid #ddd; border-radius: 5px; font-size: 12px; background: #fff; }
select { width: 100%; }
input[type=text], input[type=url] { width: 100%; }
textarea { width: 100%; height: 320px; font-family: monospace; font-size: 12px; border: 1px solid #ddd; border-radius: 6px; padding: 10px; resize: vertical; }
#log-view { background: #0d1117; color: #58a6ff; font-family: monospace; font-size: 11px; padding: 12px; border-radius: 6px; height: 360px; overflow-y: auto; white-space: pre-wrap; word-break: break-all; }
#msg { padding: 8px 13px; border-radius: 5px; margin-bottom: 10px; display: none; font-size: 12px; }
#msg.ok   { background: #d4edda; color: #155724; border: 1px solid #c3e6cb; }
#msg.err  { background: #f8d7da; color: #721c24; border: 1px solid #f5c6cb; }
#msg.info { background: #d1ecf1; color: #0c5460; border: 1px solid #bee5eb; }
.row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.status-bar { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; margin-bottom: 10px; }
.status-indicators { display: flex; gap: 12px; font-size: 11px; color: #666; flex-wrap: wrap; margin-left: auto; }
.status-ind { display: flex; align-items: center; gap: 3px; }
.node-row { display: flex; align-items: center; gap: 8px; padding: 5px 0; border-bottom: 1px solid #f5f5f5; }
.node-row:last-child { border-bottom: none; }
.node-name { flex: 1; font-size: 12px; color: #333; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.node-badge { font-size: 10px; padding: 1px 6px; border-radius: 10px; background: #e8f5e9; color: #27ae60; font-weight: bold; flex-shrink: 0; }
.lat { font-size: 11px; width: 52px; text-align: right; color: #999; flex-shrink: 0; }
.lat.good { color: #27ae60; } .lat.mid { color: #e67e22; } .lat.bad { color: #e74c3c; }
.sub-row { display: flex; align-items: center; gap: 8px; padding: 7px 0; border-bottom: 1px solid #f5f5f5; }
.sub-row:last-child { border-bottom: none; }
.sub-info { flex: 1; min-width: 0; }
.sub-name { font-size: 12px; font-weight: bold; color: #333; }
.sub-url  { font-size: 11px; color: #888; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.form-row { display: grid; grid-template-columns: 160px 1fr 110px auto; gap: 8px; align-items: end; margin-bottom: 8px; }
.form-label { font-size: 11px; color: #666; margin-bottom: 3px; }
.divider { border: none; border-top: 1px solid #f0f0f0; margin: 10px 0; }
.mode-pill { display: inline-flex; border: 1px solid #ddd; border-radius: 5px; overflow: hidden; font-size: 11px; }
.mode-pill span { padding: 5px 12px; cursor: pointer; color: #666; border-right: 1px solid #ddd; }
.mode-pill span:last-child { border-right: none; }
.mode-pill span.active { background: #2980b9; color: #fff; }
.mode-pill span:hover:not(.active) { background: #f5f5f5; }
.uptime-row { font-size: 11px; color: #999; margin-top: 6px; }
</style>
<script>
/* ── API ───────────────────────────────────────────────────────────── */
function api(action, data, cb) {
    var xhr = new XMLHttpRequest();
    xhr.open(data ? 'POST' : 'GET', '/cgi-bin/vex.cgi?action=' + action, true);
    xhr.setRequestHeader('Content-Type', 'application/json');
    xhr.onreadystatechange = function() {
        if (xhr.readyState === 4) {
            try { cb(JSON.parse(xhr.responseText)); }
            catch(e) { cb({ok: false, msg: 'Parse error'}); }
        }
    };
    xhr.send(data ? JSON.stringify(data) : null);
}

var msgTimer;
function showMsg(msg, type) {
    clearTimeout(msgTimer);
    var el = document.getElementById('msg');
    el.textContent = msg;
    el.className = type || 'info';
    el.style.display = 'block';
    msgTimer = setTimeout(function() { el.style.display = 'none'; }, 4000);
}

/* ── Status ────────────────────────────────────────────────────────── */
function refreshStatus() {
    api('status', null, function(d) {
        var dot = document.getElementById('status-dot');
        var txt = document.getElementById('status-text');
        var btn = document.getElementById('toggle-btn');
        if (d.running) {
            dot.className = 'dot running';
            txt.textContent = '运行中  PID ' + d.pid;
            btn.textContent = '停止服务'; btn.className = 'btn btn-danger';
        } else {
            dot.className = 'dot stopped';
            txt.textContent = '已停止';
            btn.textContent = '启动服务'; btn.className = 'btn btn-success';
        }
        document.getElementById('socks-port').textContent = d.socks_port || '1080';
        document.getElementById('http-port').textContent  = d.http_port  || '1087';
        document.getElementById('node-count').textContent = d.node_count || '0';
        document.getElementById('sub-count').textContent  = d.sub_count  || '0';
        if (d.version) document.getElementById('ver-badge').textContent = 'v' + d.version;
        if (d.mode) setActivePill(d.mode);
    });
    api('stats', null, function(d) {
        if (d.uptime) document.getElementById('uptime-val').textContent = d.uptime;
        var dnsEl = document.getElementById('dns-dot');
        var fwEl  = document.getElementById('fw-dot');
        dnsEl.className = 'dot ' + (d.dns_active ? 'ok' : 'off');
        fwEl.className  = 'dot ' + (d.fw_active  ? 'ok' : 'off');
        document.getElementById('dns-lbl').textContent = d.dns_active ? 'DNS 已接管' : 'DNS 未激活';
        document.getElementById('fw-lbl').textContent  = d.fw_active  ? '防火墙已激活' : '防火墙未激活';
    });
}

function toggleService() {
    api('status', null, function(d) {
        showMsg('请稍候...', 'info');
        api(d.running ? 'stop' : 'start', null, function(r) {
            showMsg(r.msg || (r.ok ? '操作成功' : '操作失败'), r.ok ? 'ok' : 'err');
            setTimeout(refreshStatus, 1800);
        });
    });
}

/* ── Proxy mode ────────────────────────────────────────────────────── */
function setActivePill(mode) {
    ['tun','socks','system'].forEach(function(m) {
        var el = document.getElementById('pill-' + m);
        if (el) el.className = (m === mode) ? 'active' : '';
    });
}
function setMode(mode) {
    api('set_mode', {mode: mode}, function(r) {
        showMsg(r.msg || (r.ok ? '模式已切换' : '切换失败'), r.ok ? 'ok' : 'err');
        if (r.ok) { setActivePill(mode); setTimeout(refreshStatus, 2000); }
    });
}

/* ── Nodes ─────────────────────────────────────────────────────────── */
function loadNodes() {
    api('nodes', null, function(d) {
        var list = document.getElementById('node-list');
        list.innerHTML = '';
        if (!d.nodes || d.nodes.length === 0) {
            list.innerHTML = '<div style="color:#999;padding:6px 0;">暂无节点，请在"订阅管理"中添加订阅</div>';
            return;
        }
        d.nodes.forEach(function(name, i) {
            var isActive = (i === parseInt(d.active));
            var row = document.createElement('div');
            row.className = 'node-row';
            row.innerHTML =
                '<span class="node-name" title="' + escHtml(name) + '">' + escHtml('[' + i + '] ' + name) + '</span>' +
                (isActive ? '<span class="node-badge">当前</span>' : '') +
                '<span class="lat" id="lat-' + i + '">—</span>' +
                '<button class="btn btn-primary btn-sm" onclick="testNode(' + i + ')">测速</button>' +
                '<button class="btn btn-primary btn-sm" onclick="switchNode(' + i + ')"' + (isActive ? ' disabled' : '') + '>切换</button>';
            list.appendChild(row);
        });
    });
}

function switchNode(idx) {
    api('set_node', {index: idx}, function(r) {
        showMsg(r.msg || (r.ok ? '节点已切换' : '切换失败'), r.ok ? 'ok' : 'err');
        if (r.ok) setTimeout(function() { loadNodes(); refreshStatus(); }, 1800);
    });
}

function testNode(idx) {
    var latEl = document.getElementById('lat-' + idx);
    if (latEl) latEl.textContent = '...';
    api('speedtest', {index: idx}, function(r) {
        if (!latEl) return;
        if (r.ok && r.latency >= 0) {
            var ms = r.latency;
            latEl.textContent = ms + 'ms';
            latEl.className = 'lat ' + (ms < 150 ? 'good' : ms < 400 ? 'mid' : 'bad');
        } else {
            latEl.textContent = '超时'; latEl.className = 'lat bad';
        }
    });
}

function testAllNodes() {
    api('nodes', null, function(d) {
        if (!d.nodes) return;
        d.nodes.forEach(function(_, i) { setTimeout(function() { testNode(i); }, i * 200); });
    });
}

/* ── Subscriptions ─────────────────────────────────────────────────── */
function loadSubs() {
    api('subscriptions', null, function(d) {
        var list = document.getElementById('sub-list');
        list.innerHTML = '';
        if (!d.subs || d.subs.length === 0) {
            list.innerHTML = '<div style="color:#999;padding:6px 0;">暂无订阅，请在上方添加</div>';
            return;
        }
        d.subs.forEach(function(s, i) {
            var row = document.createElement('div');
            row.className = 'sub-row';
            row.innerHTML =
                '<div class="sub-info">' +
                  '<div class="sub-name">' + escHtml(s.name) +
                    ' <span style="color:#aaa;font-weight:normal;font-size:10px;">[' + escHtml(s.format || 'auto') + ']</span>' +
                  '</div>' +
                  '<div class="sub-url" title="' + escHtml(s.url) + '">' + escHtml(s.url) + '</div>' +
                '</div>' +
                '<button class="btn btn-danger btn-sm" onclick="delSub(' + i + ')">删除</button>';
            list.appendChild(row);
        });
    });
}

function addSub() {
    var name   = document.getElementById('sub-name').value.trim();
    var url    = document.getElementById('sub-url').value.trim();
    var format = document.getElementById('sub-format').value;
    if (!name) { showMsg('请填写订阅名称', 'err'); return; }
    if (!url)  { showMsg('请填写订阅 URL', 'err'); return; }
    api('add_sub', {name: name, url: url, format: format}, function(r) {
        showMsg(r.msg || (r.ok ? '订阅已添加' : '添加失败'), r.ok ? 'ok' : 'err');
        if (r.ok) {
            document.getElementById('sub-name').value = '';
            document.getElementById('sub-url').value  = '';
            loadSubs();
        }
    });
}

function delSub(idx) {
    if (!confirm('确认删除该订阅？')) return;
    api('del_sub', {index: idx}, function(r) {
        showMsg(r.msg || (r.ok ? '已删除' : '删除失败'), r.ok ? 'ok' : 'err');
        if (r.ok) loadSubs();
    });
}

function updateSubs() {
    showMsg('正在更新订阅，请稍候...', 'info');
    api('update_subs', null, function(r) {
        showMsg(r.msg || (r.ok ? '订阅更新完成' : '更新失败'), r.ok ? 'ok' : 'err');
        if (r.ok) setTimeout(function() { loadNodes(); refreshStatus(); }, 2500);
    });
}

/* ── Config ────────────────────────────────────────────────────────── */
function loadConfig() {
    api('config', null, function(d) {
        document.getElementById('config-editor').value = (d.config || '').replace(/\|/g, '\n');
    });
}
function saveConfig() {
    var conf = document.getElementById('config-editor').value;
    var b64  = btoa(unescape(encodeURIComponent(conf)));
    api('save_config', {config: b64}, function(r) {
        showMsg(r.msg || (r.ok ? '配置已保存，服务将自动重启' : '保存失败'), r.ok ? 'ok' : 'err');
    });
}

/* ── Log ───────────────────────────────────────────────────────────── */
var logAutoTimer;
function loadLog() {
    api('log', null, function(d) {
        var el = document.getElementById('log-view');
        el.textContent = (d.log || '').replace(/\|/g, '\n');
        el.scrollTop = el.scrollHeight;
    });
}
function clearLog() {
    if (!confirm('确认清空日志？')) return;
    api('logs_clear', null, function(r) {
        showMsg(r.ok ? '日志已清空' : (r.msg || '清空失败'), r.ok ? 'ok' : 'err');
        if (r.ok) loadLog();
    });
}
function toggleLogAuto() {
    clearInterval(logAutoTimer);
    if (document.getElementById('log-auto').checked) {
        loadLog();
        logAutoTimer = setInterval(loadLog, 3000);
    }
}

/* ── Tabs ──────────────────────────────────────────────────────────── */
var TABS = ['tab-overview','tab-subs','tab-config','tab-log'];
function showTab(id) {
    TABS.forEach(function(t) {
        document.getElementById(t).style.display = 'none';
        document.getElementById('nav-' + t).className = 'nav-item';
    });
    document.getElementById(id).style.display = 'block';
    document.getElementById('nav-' + id).className = 'nav-item active';
    if (id === 'tab-overview') loadNodes();
    if (id === 'tab-subs')     loadSubs();
    if (id === 'tab-config')   loadConfig();
    if (id === 'tab-log')      loadLog();
}

function escHtml(s) {
    return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

window.onload = function() {
    refreshStatus();
    loadNodes();
    setInterval(refreshStatus, 6000);
    showTab('tab-overview');
};
</script>
</head>

<body>
<div class="header">
  <h1>⚡ Vex</h1>
  <span class="sub">梅林路由器代理插件</span>
  <span id="ver-badge" style="margin-left:auto;font-size:11px;opacity:.45;"></span>
</div>

<div class="nav">
  <div class="nav-item" id="nav-tab-overview" onclick="showTab('tab-overview')">总览</div>
  <div class="nav-item" id="nav-tab-subs"     onclick="showTab('tab-subs')">订阅管理</div>
  <div class="nav-item" id="nav-tab-config"   onclick="showTab('tab-config')">配置</div>
  <div class="nav-item" id="nav-tab-log"      onclick="showTab('tab-log')">日志</div>
</div>

<div class="content">
  <div id="msg"></div>

  <!-- ── Overview ── -->
  <div id="tab-overview">
    <div class="card">
      <h3>服务状态</h3>
      <div class="status-bar">
        <span class="dot stopped" id="status-dot"></span>
        <span id="status-text" style="font-weight:bold;font-size:13px;">检测中...</span>
        <button class="btn btn-success" id="toggle-btn" onclick="toggleService()">启动服务</button>
        <div class="status-indicators">
          <span class="status-ind"><span class="dot off" id="dns-dot"></span><span id="dns-lbl">DNS 未知</span></span>
          <span class="status-ind"><span class="dot off" id="fw-dot"></span><span id="fw-lbl">防火墙未知</span></span>
        </div>
      </div>
      <div class="info-grid">
        <div class="info-item"><div class="label">SOCKS5 端口</div><div class="value" id="socks-port">—</div></div>
        <div class="info-item"><div class="label">HTTP 端口</div><div class="value" id="http-port">—</div></div>
        <div class="info-item"><div class="label">节点数</div><div class="value" id="node-count">—</div></div>
        <div class="info-item"><div class="label">订阅数</div><div class="value" id="sub-count">—</div></div>
      </div>
      <div class="uptime-row">运行时长：<span id="uptime-val">—</span></div>
    </div>

    <div class="card">
      <h3>代理模式</h3>
      <div class="mode-pill">
        <span id="pill-tun"    onclick="setMode('tun')">TUN 透明代理</span>
        <span id="pill-socks"  onclick="setMode('socks')">仅 SOCKS5/HTTP</span>
        <span id="pill-system" onclick="setMode('system')">系统代理</span>
      </div>
      <div style="font-size:11px;color:#aaa;margin-top:7px;">切换模式后服务将自动重启以生效</div>
    </div>

    <div class="card">
      <h3>节点列表</h3>
      <div id="node-list"><div style="color:#999;padding:6px 0;">加载中...</div></div>
      <hr class="divider">
      <div class="row">
        <button class="btn btn-primary btn-sm" onclick="loadNodes()">刷新列表</button>
        <button class="btn btn-primary btn-sm" onclick="testAllNodes()">全部测速</button>
      </div>
    </div>
  </div>

  <!-- ── Subscriptions ── -->
  <div id="tab-subs" style="display:none">
    <div class="card">
      <h3>添加订阅</h3>
      <div class="form-row">
        <div>
          <div class="form-label">名称</div>
          <input type="text" id="sub-name" placeholder="我的订阅">
        </div>
        <div>
          <div class="form-label">订阅 URL</div>
          <input type="url" id="sub-url" placeholder="https://example.com/subscribe?token=xxx">
        </div>
        <div>
          <div class="form-label">格式</div>
          <select id="sub-format">
            <option value="auto">Auto（自动）</option>
            <option value="clash">Clash</option>
            <option value="v2ray">V2Ray</option>
            <option value="singbox">SingBox</option>
          </select>
        </div>
        <div style="padding-bottom:1px;">
          <button class="btn btn-success" onclick="addSub()">添加</button>
        </div>
      </div>
    </div>

    <div class="card">
      <h3>当前订阅</h3>
      <div id="sub-list"><div style="color:#999;padding:6px 0;">加载中...</div></div>
      <hr class="divider">
      <div class="row">
        <button class="btn btn-primary" onclick="updateSubs()">更新所有订阅</button>
        <span style="font-size:11px;color:#999;">更新后将自动重启服务以应用新节点</span>
      </div>
    </div>
  </div>

  <!-- ── Config ── -->
  <div id="tab-config" style="display:none">
    <div class="card">
      <h3>配置文件编辑器</h3>
      <div style="font-size:11px;color:#888;margin-bottom:10px;">
        路径：<code>/jffs/addons/vex/config.yaml</code>　保存后服务自动重启
      </div>
      <textarea id="config-editor" placeholder="加载中..."></textarea>
      <div class="row" style="margin-top:10px;">
        <button class="btn btn-primary" onclick="saveConfig()">保存配置</button>
        <button class="btn btn-primary" onclick="loadConfig()">重新加载</button>
      </div>
    </div>
  </div>

  <!-- ── Log ── -->
  <div id="tab-log" style="display:none">
    <div class="card">
      <h3>运行日志</h3>
      <div class="row" style="margin-bottom:10px;">
        <button class="btn btn-primary btn-sm" onclick="loadLog()">刷新</button>
        <button class="btn btn-danger btn-sm"  onclick="clearLog()">清空日志</button>
        <label style="font-size:12px;margin-left:6px;">
          <input type="checkbox" id="log-auto" onchange="toggleLogAuto()"> 自动刷新（3s）
        </label>
      </div>
      <div id="log-view">加载中...</div>
    </div>
  </div>

</div>
</body>
</html>
