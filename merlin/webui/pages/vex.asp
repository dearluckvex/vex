<!DOCTYPE html>
<html>
<head>
<meta http-equiv="Content-Type" content="text/html; charset=utf-8">
<meta http-equiv="Pragma" content="no-cache">
<meta http-equiv="Expires" content="-1">
<link rel="shortcut icon" href="images/favicon.png">
<title>Vex Proxy</title>
<link rel="stylesheet" type="text/css" href="/ext/vex/vex.css">
<script>
/* ── API helper ──────────────────────────────────────────────────── */
function api(action, data, cb) {
    var xhr = new XMLHttpRequest();
    var method = data ? 'POST' : 'GET';
    xhr.open(method, '/cgi-bin/vex.cgi?action=' + action, true);
    xhr.setRequestHeader('Content-Type', 'application/json');
    xhr.onreadystatechange = function() {
        if (xhr.readyState === 4) {
            try { cb(JSON.parse(xhr.responseText)); }
            catch(e) { cb({ok: false, msg: 'Parse error: ' + xhr.responseText}); }
        }
    };
    xhr.send(data ? JSON.stringify(data) : null);
}

/* ── Status refresh ──────────────────────────────────────────────── */
var refreshTimer;
function refreshStatus() {
    api('status', null, function(d) {
        var dot  = document.getElementById('status-dot');
        var txt  = document.getElementById('status-text');
        var btn  = document.getElementById('toggle-btn');
        if (d.running) {
            dot.className  = 'dot running';
            txt.textContent = '运行中  PID ' + d.pid;
            btn.textContent = '停止';
            btn.className   = 'btn btn-danger';
        } else {
            dot.className  = 'dot stopped';
            txt.textContent = '已停止';
            btn.textContent = '启动';
            btn.className   = 'btn btn-success';
        }
        document.getElementById('socks-port').textContent = d.socks_port || '1080';
        document.getElementById('http-port').textContent  = d.http_port  || '1087';
        document.getElementById('node-count').textContent = d.node_count || '0';
    });
}

function toggleService() {
    api('status', null, function(d) {
        var action = d.running ? 'stop' : 'start';
        showMsg('请稍候...');
        api(action, null, function(r) {
            showMsg(r.msg || (r.ok ? '成功' : '失败'));
            setTimeout(refreshStatus, 1500);
        });
    });
}

/* ── Node list ───────────────────────────────────────────────────── */
function loadNodes() {
    api('nodes', null, function(d) {
        var sel = document.getElementById('node-select');
        sel.innerHTML = '';
        if (!d.nodes || d.nodes.length === 0) {
            sel.innerHTML = '<option value="">— 暂无节点，请导入订阅 —</option>';
            return;
        }
        d.nodes.forEach(function(name, i) {
            var opt = document.createElement('option');
            opt.value = i;
            opt.textContent = '[' + i + '] ' + name;
            if (i === parseInt(d.active)) opt.selected = true;
            sel.appendChild(opt);
        });
    });
}

function setNode() {
    var idx = parseInt(document.getElementById('node-select').value);
    api('set_node', {index: idx}, function(r) {
        showMsg(r.msg || (r.ok ? '节点已切换' : '切换失败'));
        setTimeout(refreshStatus, 1500);
    });
}

/* ── Config editor ───────────────────────────────────────────────── */
function loadConfig() {
    api('config', null, function(d) {
        document.getElementById('config-editor').value =
            (d.config || '').replace(/\|/g, '\n');
    });
}

function saveConfig() {
    var conf = document.getElementById('config-editor').value;
    var b64  = btoa(unescape(encodeURIComponent(conf)));
    api('save_config', {config: b64}, function(r) {
        showMsg(r.msg || (r.ok ? '已保存' : '保存失败'));
    });
}

/* ── Log viewer ──────────────────────────────────────────────────── */
function loadLog() {
    api('log', null, function(d) {
        document.getElementById('log-view').textContent =
            (d.log || '').replace(/\|/g, '\n');
    });
}

/* ── Tab switching ───────────────────────────────────────────────── */
function showTab(id) {
    ['tab-overview','tab-config','tab-log'].forEach(function(t) {
        document.getElementById(t).style.display = 'none';
        document.getElementById('nav-' + t).className = 'nav-item';
    });
    document.getElementById(id).style.display = 'block';
    document.getElementById('nav-' + id).className = 'nav-item active';
    if (id === 'tab-config') loadConfig();
    if (id === 'tab-log')    loadLog();
}

/* ── Misc ────────────────────────────────────────────────────────── */
function showMsg(msg) {
    var el = document.getElementById('msg');
    el.textContent = msg;
    el.style.display = 'block';
    setTimeout(function() { el.style.display = 'none'; }, 3000);
}

window.onload = function() {
    refreshStatus();
    loadNodes();
    refreshTimer = setInterval(refreshStatus, 5000);
    showTab('tab-overview');
};
</script>

<style>
body { font-family: Arial, sans-serif; background: #f4f4f4; margin: 0; padding: 0; }
.header { background: #1a1a2e; color: #fff; padding: 14px 24px; display: flex; align-items: center; }
.header h1 { margin: 0; font-size: 20px; }
.header span.sub { margin-left: 10px; font-size: 12px; opacity: .6; }
.nav { background: #16213e; display: flex; padding: 0 20px; }
.nav-item { color: #aaa; padding: 12px 18px; cursor: pointer; font-size: 14px; border-bottom: 3px solid transparent; }
.nav-item:hover { color: #fff; }
.nav-item.active { color: #0f3460; background: #f4f4f4; border-bottom-color: #e94560; color: #333; font-weight: bold; }
.content { padding: 20px; max-width: 900px; margin: 0 auto; }
.card { background: #fff; border-radius: 8px; padding: 20px; margin-bottom: 16px; box-shadow: 0 1px 4px rgba(0,0,0,.1); }
.card h3 { margin: 0 0 14px; font-size: 15px; color: #333; border-bottom: 1px solid #eee; padding-bottom: 8px; }
.dot { display: inline-block; width: 10px; height: 10px; border-radius: 50%; margin-right: 8px; }
.dot.running { background: #27ae60; box-shadow: 0 0 6px #27ae60; }
.dot.stopped { background: #e74c3c; }
.info-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; margin: 14px 0; }
.info-item { background: #f8f9fa; border-radius: 6px; padding: 12px; text-align: center; }
.info-item .label { font-size: 11px; color: #888; }
.info-item .value { font-size: 20px; font-weight: bold; color: #333; margin-top: 4px; }
.btn { padding: 8px 20px; border: none; border-radius: 6px; cursor: pointer; font-size: 14px; font-weight: bold; }
.btn-success { background: #27ae60; color: #fff; }
.btn-danger  { background: #e74c3c; color: #fff; }
.btn-primary { background: #2980b9; color: #fff; }
.btn:hover   { opacity: .85; }
select { padding: 8px 12px; border: 1px solid #ddd; border-radius: 6px; width: 100%; font-size: 14px; margin-bottom: 12px; }
textarea { width: 100%; height: 320px; font-family: monospace; font-size: 13px; border: 1px solid #ddd; border-radius: 6px; padding: 10px; box-sizing: border-box; resize: vertical; }
#log-view { background: #1a1a2e; color: #a8ff78; font-family: monospace; font-size: 12px; padding: 14px; border-radius: 6px; height: 320px; overflow-y: auto; white-space: pre-wrap; word-break: break-all; }
#msg { background: #2980b9; color: #fff; padding: 10px 16px; border-radius: 6px; margin-bottom: 12px; display: none; }
.row { display: flex; align-items: center; gap: 10px; }
</style>
</head>

<body>
<div class="header">
  <h1>⚡ Vex</h1>
  <span class="sub">跨平台代理客户端 · 梅林路由器插件</span>
</div>

<div class="nav">
  <div class="nav-item" id="nav-tab-overview" onclick="showTab('tab-overview')">总览</div>
  <div class="nav-item" id="nav-tab-config"   onclick="showTab('tab-config')">配置</div>
  <div class="nav-item" id="nav-tab-log"      onclick="showTab('tab-log')">日志</div>
</div>

<div class="content">
  <div id="msg"></div>

  <!-- ── Overview Tab ── -->
  <div id="tab-overview">
    <div class="card">
      <h3>服务状态</h3>
      <div class="row">
        <span class="dot stopped" id="status-dot"></span>
        <span id="status-text">检测中...</span>
        <button class="btn btn-success" id="toggle-btn" onclick="toggleService()">启动</button>
      </div>
      <div class="info-grid">
        <div class="info-item">
          <div class="label">SOCKS5 端口</div>
          <div class="value" id="socks-port">—</div>
        </div>
        <div class="info-item">
          <div class="label">HTTP 代理端口</div>
          <div class="value" id="http-port">—</div>
        </div>
        <div class="info-item">
          <div class="label">节点数量</div>
          <div class="value" id="node-count">—</div>
        </div>
      </div>
    </div>

    <div class="card">
      <h3>节点选择</h3>
      <select id="node-select"><option>加载中...</option></select>
      <div class="row">
        <button class="btn btn-primary" onclick="setNode()">切换节点</button>
        <button class="btn btn-primary" onclick="loadNodes()">刷新列表</button>
      </div>
    </div>
  </div>

  <!-- ── Config Tab ── -->
  <div id="tab-config" style="display:none">
    <div class="card">
      <h3>配置文件编辑器</h3>
      <p style="font-size:12px;color:#888;">
        配置文件路径：<code>/jffs/addons/vex/config.yaml</code><br>
        修改订阅、节点、端口等设置后点击保存，服务将自动重启。
      </p>
      <textarea id="config-editor" placeholder="加载中..."></textarea>
      <br><br>
      <button class="btn btn-primary" onclick="saveConfig()">保存配置</button>
    </div>
  </div>

  <!-- ── Log Tab ── -->
  <div id="tab-log" style="display:none">
    <div class="card">
      <h3>运行日志</h3>
      <button class="btn btn-primary" onclick="loadLog()" style="margin-bottom:10px;">刷新日志</button>
      <div id="log-view">加载中...</div>
    </div>
  </div>

</div>
</body>
</html>
