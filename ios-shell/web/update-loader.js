/* 固定启动器：更新协议变更才需要重新安装 App。业务页面使用同一 origin，保留配对与离线数据。 */
(function () {
  'use strict';
  var KEY = 'shipping-shell.ui-update.v1', API = 1, MAX = 1024 * 1024;
  var FILES = ['app.html', 'shipping/index.html', 'shipping/m.css', 'shipping/m.js', 'shipping/transport.js'];
  var state = {}, running = null, builtin = null, checking = null, readyTimer = 0, frameTimer = 0;
  var lastCheck = 0, message = '尚未检查页面更新', started = false;
  function read(key) { try { return JSON.parse(localStorage.getItem(key)); } catch (_) { return null; } }
  function save(next) { localStorage.setItem(KEY, JSON.stringify(next)); state = next; }
  function notify(text) {
    message = text;
    window.dispatchEvent(new CustomEvent('shipping-update-status'));
  }
  async function hash(payload) {
    var invoke = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;
    if (invoke) return invoke('hash_update_payload', { payload: payload });
    if (!window.crypto || !window.crypto.subtle) throw new Error('当前环境不支持更新校验');
    var bytes = await window.crypto.subtle.digest('SHA-256', new TextEncoder().encode(payload));
    return Array.from(new Uint8Array(bytes)).map(function (v) { return v.toString(16).padStart(2, '0'); }).join('');
  }
  async function validate(bundle) {
    if (!bundle || !/^[a-f0-9]{64}$/.test(bundle.version || '') || typeof bundle.payload !== 'string' || bundle.payload.length > MAX) throw new Error('更新包格式不正确');
    if (await hash(bundle.payload) !== bundle.version) throw new Error('更新包校验失败，继续使用原版');
    var pkg = JSON.parse(bundle.payload);
    if (pkg.schema !== 1 || pkg.shellApi !== API) throw new Error('此更新需要安装新版 App');
    if (!pkg.files || Object.keys(pkg.files).length !== FILES.length || FILES.some(function (f) { return typeof pkg.files[f] !== 'string' || !pkg.files[f].trim(); })) throw new Error('更新文件不完整');
    if (!pkg.files['app.html'].includes('shippingUpdates.ready()') || !pkg.files['shipping/m.js'].includes('ui-ready')) throw new Error('更新缺少启动确认');
    return pkg;
  }
  function rejection(version) { return (state.rejected || []).includes(version); }
  function reject(next, version) { if (version) next.rejected = (next.rejected || []).concat(version).slice(-8); }
  function reset() {
    var next = { builtinVersion: builtin && builtin.version, rejected: (state.rejected || []).slice() };
    reject(next, state.active && state.active.version);
    reject(next, state.pending && state.pending.version);
    try { save(next); } catch (_) { localStorage.removeItem(KEY); }
    window.location.reload();
  }
  function failed() {
    clearTimeout(readyTimer); clearTimeout(frameTimer);
    if (!running || !state.active || running.version !== state.active.version) return;
    var next = Object.assign({}, state, { active: state.previous || null, previous: null, pending: null, trial: null });
    reject(next, running.version);
    try { save(next); } catch (_) { localStorage.removeItem(KEY); }
    window.location.reload();
  }
  async function request(url) {
    var controller = new AbortController(), timer = setTimeout(function () { controller.abort(); }, 8000);
    try {
      var res = await fetch(url, { signal: controller.signal, cache: 'no-store', credentials: 'omit', redirect: 'error' });
      if (!res.ok) throw new Error(res.status === 403 ? '配对已失效，请重新扫码' : res.status === 404 ? '电脑端暂不支持页面更新' : '电脑未能提供更新');
      // 文件体也在超时范围内，完整读取后才取消计时器。
      var text = await res.text();
      if (text.length > MAX + 100000) throw new Error('更新包超过大小限制');
      return JSON.parse(text);
    } finally { clearTimeout(timer); }
  }
  async function check(raw, force) {
    if (checking) return checking;
    if (!force && Date.now() - lastCheck < 600000) return;
    var u;
    try {
      u = new URL(raw);
      if (!/^https?:$/.test(u.protocol) || !/^[a-f0-9]{64}$/i.test(u.searchParams.get('t') || '')) throw new Error('请先扫码配对电脑');
    } catch (_) { notify('请先扫码配对电脑'); return; }
    lastCheck = Date.now();
    checking = (async function () {
      try {
        notify('正在检查页面更新…');
        var endpoint = u.origin + '/api/app-update', auth = '?t=' + encodeURIComponent(u.searchParams.get('t'));
        var manifest = await request(endpoint + '/manifest' + auth);
        if (manifest.schema !== 1 || manifest.shellApi !== API) throw new Error('此更新需要安装新版 App');
        if (!/^[a-f0-9]{64}$/.test(manifest.version || '') || !Number.isInteger(manifest.bytes) || manifest.bytes > MAX || manifest.bytes < 1) throw new Error('更新信息不正确');
        if (rejection(manifest.version)) { notify('这个版本曾启动失败，继续使用原版'); return; }
        if (running && manifest.version === running.version) {
          if (state.pending) save(Object.assign({}, state, { pending: null }));
          notify('页面已是最新版'); return;
        }
        if (state.pending && manifest.version === state.pending.version) { notify('新版已下载，下次打开生效'); return; }
        notify('正在下载页面更新…');
        var bundle = await request(endpoint + '/bundle' + auth + '&v=' + manifest.version);
        if (bundle.version !== manifest.version) throw new Error('电脑页面版本已变化，请重新检查');
        await validate(bundle);
        save(Object.assign({}, state, { pending: bundle }));
        notify('新版已下载，下次打开生效');
      } catch (e) { lastCheck = 0; notify((e && e.message || '更新失败') + '；当前页面可继续使用'); }
      finally { checking = null; }
    })();
    return checking;
  }
  function escapeClose(text, tag) { return text.replace(new RegExp('</' + tag, 'gi'), '<' + String.fromCharCode(92) + '/' + tag); }
  function inlineScript(text) { return '<script>' + escapeClose(text, 'script') + '</script>'; }
  function loadFrame(frame) {
    clearTimeout(frameTimer);
    var files = running.files;
    var html = files['shipping/index.html']
      .replace('<link rel="stylesheet" href="./m.css" />', function () { return '<style>' + escapeClose(files['shipping/m.css'], 'style') + '</style>'; })
      .replace('<script src="./transport.js"></script>', function () { return inlineScript(files['shipping/transport.js']); })
      .replace('<script src="./m.js"></script>', function () { return inlineScript(files['shipping/m.js']); });
    // srcdoc 与外层共享 origin，不搬迁、不清除 localStorage。
    frame.src = './shipping/index.html';
    frame.srcdoc = html;
    if (state.active && state.active.version === running.version) frameTimer = setTimeout(function () { if (frame.style.display !== 'none' && frame.srcdoc) failed(); }, 10000);
  }
  function ready() {
    clearTimeout(readyTimer);
    if (state.trial && running && state.trial === running.version) {
      try { save(Object.assign({}, state, { trial: null })); } catch (_) { /* 下次启动保守回退 */ }
    }
  }
  async function boot() {
    if (started) return;
    started = true;
    try {
      var response = await fetch('./builtin-update.json', { cache: 'no-store' });
      builtin = await response.json();
      var base = await validate(builtin);
      state = read(KEY) || {};
      // 安装新的基础包后，以新版内置页面为准，避免旧缓存盖住新安装包。
      if (state.builtinVersion !== builtin.version) {
        var fresh = { builtinVersion: builtin.version };
        try { save(fresh); } catch (_) { state = fresh; }
      }
      if (state.trial) {
        var rollback = Object.assign({}, state, { active: state.previous || null, previous: null, pending: null, trial: null });
        reject(rollback, state.trial);
        try { save(rollback); } catch (_) { state = {}; }
      }
      // 缓存损坏、空间不足都不能阻止打开内置版。
      var candidate = state.pending || state.active, pkg = null;
      if (candidate && !rejection(candidate.version)) {
        try {
          pkg = await validate(candidate);
          if (state.pending) save(Object.assign({}, state, { previous: state.active || null, active: candidate, pending: null, trial: candidate.version }));
        } catch (_) {
          var fallback = state.pending ? state.active : state.previous;
          var next = Object.assign({}, state, { active: null, pending: null, previous: null, trial: null });
          reject(next, candidate.version);
          if (fallback && !rejection(fallback.version)) {
            try { pkg = await validate(fallback); candidate = fallback; next.active = fallback; } catch (_) { pkg = null; }
          } else pkg = null;
          try { save(next); } catch (_) { state = {}; pkg = null; }
        }
      }
      running = Object.assign({ version: pkg ? candidate.version : builtin.version }, pkg || base);
      message = state.pending ? '新版已下载，下次打开生效' : '当前页面 ' + running.version.slice(0, 12);
      readyTimer = setTimeout(failed, 10000);
      // 保留当前 Window 和 Tauri 事件桥，只替换页面 DOM 并执行业务脚本。
      var parsed = new DOMParser().parseFromString(running.files['app.html'], 'text/html');
      document.replaceChild(document.importNode(parsed.documentElement, true), document.documentElement);
      Array.from(document.querySelectorAll('script')).forEach(function (old) {
        var script = document.createElement('script');
        Array.from(old.attributes).forEach(function (attr) { script.setAttribute(attr.name, attr.value); });
        script.textContent = old.textContent;
        old.replaceWith(script);
      });
    } catch (e) {
      document.body.textContent = '页面启动失败，请恢复内置版本后重试。';
      var button = document.createElement('button'); button.textContent = '恢复内置版本'; button.onclick = reset; document.body.appendChild(button);
    }
  }
  window.addEventListener('message', function (e) {
    var frame = document.getElementById('view');
    if (frame && e.source === frame.contentWindow && e.data && e.data.source === 'shipping-mobile' && e.data.type === 'ui-ready') clearTimeout(frameTimer);
  });
  window.shippingUpdates = {
    boot: boot, ready: ready, check: check, loadFrame: loadFrame, restoreBuiltin: reset,
    hasPending: function () { return !!state.pending; },
    applyPending: function () { if (state.pending) window.location.reload(); },
    status: function () { return message; },
    version: function () { return running ? running.version.slice(0, 12) : '内置版'; }
  };
  // 测试可单独调用 boot；生产固定启动页立即启动。
  if (!window.__SHIPPING_UPDATE_TEST__) boot();
})();
