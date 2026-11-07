const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { createHash } = require('node:crypto');
const root = path.join(__dirname, '../..');
const script = fs.readFileSync(path.join(root, 'ios-shell/web/update-loader.js'), 'utf8');
const builtin = JSON.parse(fs.readFileSync(path.join(root, 'ios-shell/web/builtin-update.json'), 'utf8'));
const KEY = 'shipping-shell.ui-update.v1', paired = 'http://192.168.1.2:17650/?t=' + 'a'.repeat(64);
function bundle(label) {
  const pkg = JSON.parse(builtin.payload); pkg.files['app.html'] += '<!-- ' + label + ' -->';
  const payload = JSON.stringify(pkg);
  return { version: createHash('sha256').update(payload).digest('hex'), payload };
}
function setup(initial = {}, storage) {
  const store = storage || new Map([[KEY, JSON.stringify({ builtinVersion: builtin.version, ...initial })], ['shipping-shell.pending', '[{"name":"待回传"}]'], ['shipping-shell.cache','{"rows":[1]}']]);
  const timers = new Map(), listeners = {}, calls = []; let id = 0, html = '', reloads = 0, quota = false;
  let handler = async () => { throw new Error('电脑不在线'); };
  const document = { documentElement: {}, body: { textContent: '', appendChild() {} },
    importNode: n => n, replaceChild: n => { html = n.html; }, querySelectorAll: () => [],
    createElement: () => ({}), getElementById: () => document.frame };
  const window = { __SHIPPING_UPDATE_TEST__: true, location: { reload() { reloads++; } },
    __TAURI__: { core: { invoke: async (name, {payload}) => { assert.equal(name, 'hash_update_payload'); return createHash('sha256').update(payload).digest('hex'); } } },
    addEventListener: (name, fn) => { listeners[name] = fn; }, dispatchEvent() {} };
  const response = data => ({ok:true,status:200,json:async()=>data,text:async()=>JSON.stringify(data)});
  vm.runInNewContext(script, { window, document, DOMParser: class { parseFromString(html) { return {documentElement:{html}}; } },
    localStorage: { getItem: k => store.get(k) || null, setItem(k,v) { if (quota) throw new Error('空间不足'); store.set(k,v); }, removeItem: k => store.delete(k) },
    URL, AbortController, CustomEvent: class {}, TextEncoder, Uint8Array, Date, console,
    setTimeout(fn, ms) { const key=++id; timers.set(key,{fn,ms}); return key; }, clearTimeout: key => timers.delete(key),
    fetch: async (url, options) => { calls.push(url); return url === './builtin-update.json' ? response(builtin) : handler(url,options); }
  });
  return { api: window.shippingUpdates, store, calls, document, listeners, timers,
    get html() {return html;}, get reloads() {return reloads;}, read:()=>JSON.parse(store.get(KEY)),
    quota:()=>{quota=true;}, serve(update) { handler=async url => response(url.includes('/manifest') ? {schema:1,shellApi:1,version:update.version,bytes:Buffer.byteLength(update.payload)} : update); },
    setHandler(fn) { handler=fn; }, fireTimers() { for(const [key,{fn,ms}] of [...timers]) if(ms===10000){timers.delete(key);fn();} }
  };
}
test('首次启动无需电脑，使用内置页面且不动离线数据',async()=>{
  const h=setup(); await h.api.boot(); h.api.ready(); assert.match(h.html,/今天的工作/);
  assert.equal(h.calls.length,1); assert.equal(h.store.get('shipping-shell.pending'),'[{"name":"待回传"}]');
});
test('整包下载后只暂存，下次启动才切换并确认健康',async()=>{
  const update=bundle('next'); const h=setup(); await h.api.boot(); h.api.ready(); h.serve(update);
  await h.api.check(paired,true); assert.equal(h.read().pending.version,update.version); assert.doesNotMatch(h.html,/<!-- next -->/);
  const next=setup({},h.store); await next.api.boot(); assert.match(next.html,/<!-- next -->/); assert.equal(next.read().trial,update.version);
  next.api.ready(); assert.equal(next.read().trial,null); assert.equal(next.read().active.version,update.version);
});
test('未配对不请求更新；网络失败继续使用原页',async()=>{
  const h=setup(); await h.api.boot(); h.api.ready(); await h.api.check('http://192.168.1.9/',true); assert.equal(h.calls.length,1);
  await h.api.check(paired,true); assert.match(h.api.status(),/可继续使用/); assert.equal(h.read().pending,undefined);
});
test('摘要不符、协议不兼容、缺文件不会污染已安装版本',async()=>{
  for(const mutate of [u=>({...u,payload:u.payload+' '}),u=>{const p=JSON.parse(u.payload);p.shellApi=99;const payload=JSON.stringify(p);return {version:createHash('sha256').update(payload).digest('hex'),payload};},u=>{const p=JSON.parse(u.payload);delete p.files['shipping/m.js'];const payload=JSON.stringify(p);return {version:createHash('sha256').update(payload).digest('hex'),payload};}]) {
    const h=setup();await h.api.boot();h.api.ready();h.serve(mutate(bundle('bad')));await h.api.check(paired,true);
    assert.equal(h.read().pending,undefined);assert.match(h.api.status(),/可继续使用/);
  }
});
test('缓存空间不足不切换页面、不清除清单与待回传操作',async()=>{
  const h=setup();await h.api.boot();h.api.ready();h.serve(bundle('quota'));h.quota();await h.api.check(paired,true);
  assert.equal(h.read().pending,undefined);assert.match(h.api.status(),/空间不足/);assert.equal(h.store.get('shipping-shell.cache'),'{"rows":[1]}');
});
test('新页面启动无确认时回退上一版，失败版本不重复下载',async()=>{
  const previous=bundle('stable'),next=bundle('broken');const h=setup({active:previous,pending:next});
  await h.api.boot();h.fireTimers();assert.equal(h.reloads,1);assert.equal(h.read().active.version,previous.version);
  const fallback=setup({},h.store);await fallback.api.boot();fallback.api.ready();assert.match(fallback.html,/<!-- stable -->/);
  fallback.serve(next);await fallback.api.check(paired,true);assert.equal(fallback.read().pending,null);assert.match(fallback.api.status(),/曾启动失败/);
});
test('激活过程中被终止，下次启动也会恢复上一版',async()=>{
  const previous=bundle('stable'),next=bundle('crash');const h=setup({active:next,previous,trial:next.version});
  await h.api.boot();assert.match(h.html,/<!-- stable -->/);assert.ok(h.read().rejected.includes(next.version));
});
test('下载缓存损坏时保留当前稳定版本',async()=>{
  const previous=bundle('stable'),bad={...bundle('bad'),payload:'损坏'};const h=setup({active:previous,pending:bad});
  await h.api.boot();assert.match(h.html,/<!-- stable -->/);assert.equal(h.read().active.version,previous.version);
});
test('整包内联清单资源保持同一 origin，并等待清单页面就绪',async()=>{
  const update=bundle('stable');const h=setup({active:update});await h.api.boot();h.api.ready();
  const frame={style:{display:'block'},contentWindow:{}};h.document.frame=frame;h.api.loadFrame(frame);
  assert.match(frame.srcdoc,/shippingTransport/);assert.doesNotMatch(frame.srcdoc,/src="\.\/(m|transport)\.js"/);assert.doesNotMatch(frame.srcdoc,/href="\.\/m.css"/);
  h.listeners.message({source:frame.contentWindow,data:{source:'shipping-mobile',type:'ui-ready'}});h.fireTimers();assert.equal(h.reloads,0);
});
test('返回工作台不会把未加载的清单误判为更新失败',async()=>{
  const h=setup({active:bundle('stable')});await h.api.boot();h.api.ready();const frame={style:{display:'block'}};
  h.api.loadFrame(frame);frame.style.display='none';frame.srcdoc='';h.fireTimers();assert.equal(h.reloads,0);
});
test('恢复内置版与安装新基础包不会清除业务数据',async()=>{
  const h=setup({active:bundle('old')});await h.api.boot();h.api.ready();h.api.restoreBuiltin();assert.equal(h.reloads,1);assert.equal(h.read().active,undefined);
  const other=setup({builtinVersion:'old-install',active:bundle('old')});await other.api.boot();assert.doesNotMatch(other.html,/<!-- old -->/);assert.equal(other.store.get('shipping-shell.cache'),'{"rows":[1]}');
});


test('内置更新包与当前源文件完全一致，避免发布旧页面',async()=>{
 const {buildUpdate}=await import(require('node:url').pathToFileURL(path.join(root,'ios-shell/scripts/update-package.mjs')).href);
 assert.deepEqual(await buildUpdate(),builtin);
});
