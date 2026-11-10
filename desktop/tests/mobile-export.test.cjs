const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function setup(rows, copyMode = 'ok') {
  const elements = new Map(), copies = [];
  function element(id) {
    if (elements.has(id)) return elements.get(id);
    const classes = new Set(id === 'export-panel' ? ['hidden'] : []);
    const el = { value: '', textContent: '', innerHTML: '', disabled: false, children: [], listeners: {}, style: {},
      classList: { add: c => classes.add(c), remove: c => classes.delete(c), contains: c => classes.has(c), toggle: (c, v) => v ? classes.add(c) : classes.delete(c) },
      addEventListener: (event, fn) => { el.listeners[event] = fn; },
      querySelector: () => element('ticket-option'), focus() {}, select() { el.selected = true; }, setSelectionRange() {},
    };
    elements.set(id, el); return el;
  }
  const window = { shippingTransport: {
    cached: () => rows, sheet: () => new Promise(() => {}), status: () => '离线清单',
    exportInfo: () => ({ source: 'http://192.168.1.242:17650', cachedAt: 1000, offline: true, pending: 2 }),
  }, addEventListener() {}, scrollTo() {} };
  window.parent = window;
  const document = { getElementById: element, execCommand: () => false };
  const navigator = { clipboard: { writeText: async text => { if (copyMode === 'fail') throw new Error('denied'); copies.push(text); } } };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../src-tauri/mobile/m.js'), 'utf8'), {
    window, document, navigator, location: { search: '', origin: 'http://localhost' }, URLSearchParams, Date,
    setTimeout: () => 1, clearTimeout() {}, setInterval: () => 1,
  });
  return { element, copies, transport:window.shippingTransport, click: id => element(id).listeners.click() };
}
const row = { waybill: '6812705863', inbound: 'I1', name: '商品', customer: '九宸', qty: 3, checked: true, issues: ['history_archive'] };
test('电脑同步直接调用主库接口，不触发复制且忽略导出范围',async()=>{
  const input=[row,{...row,waybill:'W2',checked:false}],h=setup(input);let calls=0;
  h.transport.sheet=async()=>{calls++;return {rows:input,offline:false,conflicts:[]};};
  h.click('export-open');await h.click('export-sync');
  assert.equal(calls,1);assert.equal(h.copies.length,0);
  assert.match(h.element('export-sync-info').textContent,/已同步到当前电脑/);
  assert.equal(h.element('export-sync').disabled,false);
});
test('离线保留不能误报电脑同步成功',async()=>{
  const h=setup([row]);h.transport.sheet=async()=>({rows:[row],offline:true});
  h.click('export-open');await h.click('export-sync');
  assert.match(h.element('export-sync-info').textContent,/尚未同步到电脑/);
  assert.equal(h.copies.length,0);
});

test('默认只导出已出，保留重复明细、单号与来源，不修改数据', async () => {
  const input = [row, { ...row }, { ...row, checked: false, qty: 8, waybill: 'W2' }];
  const before = JSON.stringify(input), h = setup(input);
  h.click('export-open');
  const text = h.element('export-text').value;
  assert.match(text, /2 行 · 6 件 · 已出 6 件 · 未出 0 件/);
  assert.equal(text.split('\n').filter(line => line.startsWith('6812705863\t')).length, 2);
  assert.match(text, /192\.168\.1\.242/); assert.match(text, /离线缓存.*待同步操作 2 条/);
  assert.match(text, /含存档/); assert.doesNotMatch(text, /W2/);
  await h.click('export-copy'); assert.equal(h.copies[0], text);
  assert.equal(JSON.stringify(input), before);
});

test('全部清单包含缺单号及未出记录，单元格换行不会破坏表格列数', () => {
  const h = setup([{ ...row, name: '商品\tA\nB', waybill: '', checked: false }]);
  h.click('export-open'); assert.equal(h.element('export-copy').disabled, true);
  h.element('export-scope').value = 'all'; h.element('export-scope').listeners.change();
  const lines = h.element('export-text').value.split('\n');
  assert.equal(lines.at(-1).split('\t').length, 8);
  assert.match(lines.at(-1), /商品 A B/); assert.match(lines.at(-1), /未出货/);
  assert.equal(h.element('export-copy').disabled, false);
});

test('剪贴板被拒绝时显示选中文本供手动复制，不能误报成功', async () => {
  const h = setup([row], 'fail'); h.click('export-open'); await h.click('export-copy');
  assert.equal(h.copies.length, 0); assert.equal(h.element('export-text').selected, true);
  assert.match(h.element('export-hint').textContent, /自动复制未成功.*长按/);
});

test('当前票导出不会混入其它票，打开后固定快照', () => {
  const input = [row, { ...row, waybill: 'W2' }], h = setup(input);
  h.element('tickets').listeners.click({ target: { closest: selector => selector === '.ticket' ? { getAttribute: () => row.waybill } : null } });
  h.click('doc-export');
  assert.match(h.element('export-text').value, /当前票 6812705863/);
  assert.doesNotMatch(h.element('export-text').value, /W2/);
  input[0].qty = 999;
  h.element('export-scope').listeners.change();
  assert.match(h.element('export-text').value, /1 行 · 3 件/);
});

test('默认按票，全部明细使用两行布局、客户在商品前、完整入库号并按行筛选',()=>{
 const h=setup([{...row,arrival:'已到货',remarks:'EXX'},{...row,name:'未出商品',checked:false,waybill:'发深圳',issues:['special_shenzhen']}]);
 assert.match(h.element('tickets').innerHTML,/class="ticket/);
 h.element('layout-switch').listeners.click({target:{closest:()=>({getAttribute:()=> 'table'})}});
 assert.doesNotMatch(h.element('tickets').innerHTML,/<table/);
 assert.match(h.element('tickets').innerHTML,/class="cust">九宸.*class="name">/);
 assert.match(h.element('tickets').innerHTML,/IB064|I1/);assert.match(h.element('tickets').innerHTML,/发深圳/);
 h.element('seg').listeners.click({target:{closest:()=>({getAttribute:()=> 'todo'})}});
 assert.match(h.element('tickets').innerHTML,/未出商品/);
 assert.doesNotMatch(h.element('tickets').innerHTML,/6812705863/);
});

test('手机刷新先通知电脑检查云表格，完成后才读取并显示最新清单',async()=>{
 const h=setup([row]);const calls=[];
 h.element('layout-switch').listeners.click({target:{closest:()=>({getAttribute:()=> 'table'})}});
 h.transport.request=async(path,options)=>{calls.push([path,options.method]);return {ok:true,json:async()=>({id:1,state:'done'})};};
 h.transport.sheet=async()=>{calls.push(['sheet']);return {rows:[{...row,name:'云端新商品'}]};};
 await h.click('refresh');
 assert.equal(calls[0][0],'/api/refresh');assert.equal(calls[0][1],'POST');assert.equal(calls[1][0],'sheet');
 assert.match(h.element('tickets').innerHTML,/云端新商品/);assert.match(h.element('sync').textContent,/云表格已刷新/);
 assert.equal(h.element('refresh').disabled,false);
});

test('电脑云刷新失败时保留原清单并显示真实错误，不提示刷新成功',async()=>{
 const h=setup([row]);
 h.transport.request=async()=>({ok:true,json:async()=>({id:1,state:'error',error:'WPS登录已过期'})});
 await h.click('refresh');assert.match(h.element('sync').textContent,/WPS登录已过期/);
 assert.doesNotMatch(h.element('sync').textContent,/已刷新/);assert.match(h.element('tickets').innerHTML,/6812705863/);
});

test('单号记忆面板显示分批原表行号及箱数，不出现资料冲突JSON',()=>{
 const h=setup([row]);h.transport.waybillOnly=true;
 h.transport.syncInfo=()=>({computer:'家里电脑',pending:0,lastSync:1,waybillNotes:[{id:'note',name:'组合工具垫',inbound:'IB1',waybill:'6812700001',needsMatch:true,candidates:[{syncId:'a',sourceRow:273,qty:20,checked:true},{syncId:'b',sourceRow:291,qty:15,checked:false}]}]});
 h.click('master-sync-open');const html=h.element('master-conflicts').innerHTML;
 assert.match(html,/原表第 273 行/);assert.match(html,/20 箱 · 已出/);assert.match(html,/原表第 291 行/);assert.match(html,/15 箱 · 未出/);
 assert.match(html,/6812700001/);assert.doesNotMatch(html,/采用手机值|actualWeight|JSON/);
 assert.match(h.element('master-sync-info').textContent,/手机单独记住单号/);
});
