/* 本地清单的数据层：页面随 App 安装，电脑只提供 JSON 接口。 */
(function () {
  'use strict';
  var CACHE = 'shipping-shell.cache', PENDING = 'shipping-shell.pending', CONNECTION = 'shipping-shell.current';
  var busy = Promise.resolve();
  var offline = false;
  function read(key, fallback) { try { return JSON.parse(localStorage.getItem(key)) || fallback; } catch (_) { return fallback; } }
  function write(key, value) { localStorage.setItem(key, JSON.stringify(value)); }
  function id(r) { return JSON.stringify([r.waybill, r.inbound, r.name]); }
  function endpoint() {
    var entry = read(CONNECTION, null), u;
    if (!entry || !entry.url) throw new Error('尚未配对电脑，请在设备与连接中扫码');
    u = new URL(entry.url);
    if (!/^https?:$/.test(u.protocol) || !/^[a-f0-9]{64}$/i.test(u.searchParams.get('t') || '')) throw new Error('配对信息无效，请重新扫码');
    return u;
  }
  function sameComputer(a, b) { try { return new URL(a).origin === new URL(b).origin; } catch (_) { return false; } }
  async function request(path, options) {
    if (!['/api/sheet', '/api/check', '/api/claim'].includes(path)) throw new Error('不支持的接口');
    var u = endpoint(), controller = new AbortController();
    var timer = setTimeout(function () { controller.abort(); }, 5000);
    try {
      return await fetch(u.origin + path + '?t=' + encodeURIComponent(u.searchParams.get('t')), Object.assign({}, options, { signal: controller.signal, cache: 'no-store', credentials: 'omit' }));
    } finally { clearTimeout(timer); }
  }
  async function requireOk(res) {
    if (res.ok) return res;
    var data = await res.json().catch(function () { return {}; });
    var error = new Error(res.status === 403 ? '配对已失效，请重新扫码' : (data.error || '请求失败'));
    error.status = res.status;
    throw error;
  }
  function connectionUrl() { try { return endpoint().href; } catch (_) { return ''; } }
  function cacheForConnection() {
    var cache = read(CACHE, null), url = connectionUrl();
    return cache && (!url || sameComputer(cache.url, url)) ? cache : null;
  }
  function forConnection(op, url) {
    // 旧版离线队列未带地址，只能用于其所属快照电脑。
    return sameComputer(op.computer || (read(CACHE, {}) || {}).url, url);
  }
  async function flush() {
    var url = connectionUrl(), pending = read(PENDING, []);
    for (var op of pending) {
      if (op.conflict || !forConnection(op, url)) continue;
      var res = await request('/api/check', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(op) });
      if (res.status === 409 || res.status === 400) { op.conflict = true; write(PENDING, pending); continue; }
      await requireOk(res);
      pending = pending.filter(function (item) { return item !== op; });
      write(PENDING, pending);
    }
  }
  function serialize(fn) { var next = busy.catch(function () {}).then(fn); busy = next; return next; }
  function pendingForCurrent() { var url = connectionUrl(); return read(PENDING, []).filter(function (op) { return !url || forConnection(op, url); }); }
  function status() {
    var ops = pendingForCurrent(), conflicts = ops.filter(function (op) { return op.conflict; }).length;
    if (conflicts) return conflicts + ' 条操作冲突 · 请刷新核对后重新操作';
    if (ops.length) return '已存手机 · ' + ops.length + ' 条待同步';
    return offline ? '离线清单 · 显示上次保存记录' : '';
  }
  function sheet() {
    return serialize(async function () {
      try {
        await flush();
        var res = await requireOk(await request('/api/sheet'));
        var data = await res.json();
        if (!Array.isArray(data.rows)) throw new Error('电脑返回的清单格式不正确');
        write(CACHE, { at: Date.now(), url: endpoint().href, rows: data.rows });
        offline = false;
        data.offline = false;
        return data;
      } catch (error) {
        offline = true;
        var cache = cacheForConnection();
        if (error.status || !cache) throw error;
        return { rows: cache.rows || [], offline: true, cachedAt: cache.at };
      }
    });
  }
  function check(row, checked) {
    return serialize(async function () {
      if (checked && (row.issues || []).indexOf("special_amazon") === -1 && (!String(row.waybill || '').trim() || !String(row.inbound || '').trim())) throw new Error('入库号和单号必须都填写，才能出货');
      var cache = cacheForConnection();
      var op = { waybill:row.waybill, inbound:row.inbound, name:row.name, checked:checked, sourceId:row.sourceId, recordId:row.recordId, datasetId:row.datasetId, version:row.version, computer:connectionUrl() || (cache && cache.url) };
      try {
        var existing = pendingForCurrent().some(function (item) { return id(item) === id(op); });
        if (existing) throw new TypeError('已有离线操作，合并到待同步队列');
        await requireOk(await request('/api/check', { method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify(op) }));
        offline = false;
        if (cache) { cache.rows = cache.rows.map(function (r) { return id(r) === id(row) ? Object.assign({}, r, {checked:checked,version:row.version + 1}) : r; }); write(CACHE, cache); }
        return { offline:false };
      } catch (error) {
        if (error.status || !cache) throw error;
        var pending = read(PENDING, []), previous = pending.find(function (p) { return id(p) === id(op) && sameComputer(p.computer || cache.url, op.computer); });
        // 保留第一次操作的版本；重复离线切换不能伪造递增版本。
        if (previous && !previous.conflict) { previous.checked = checked; }
        else {
          pending = pending.filter(function (p) { return p !== previous; });
          pending.push(op);
        }
        write(PENDING, pending);
        cache.rows = cache.rows.map(function (r) { return id(r) === id(row) ? Object.assign({}, r, {checked:checked}) : r; });
        write(CACHE, cache);
        offline = true;
        return { offline:true };
      }
    });
  }
  function cached() { var cache = cacheForConnection(); return cache && cache.rows || []; }
  window.shippingTransport = { request:request, sheet:sheet, check:check, cached:cached, status:status };
})();