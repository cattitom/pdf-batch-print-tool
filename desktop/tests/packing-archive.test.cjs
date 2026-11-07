const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');
const source = fs.readFileSync(path.join(__dirname, '../src/pages/packing/archiveRows.ts'), 'utf8');
const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
const moduleState = { exports: {} };
vm.runInNewContext(code, { module: moduleState, exports: moduleState.exports });
const { withArchiveRows, HISTORY_ISSUE } = moduleState.exports;
const row = (overrides = {}) => ({ waybill: '6812705712', inbound: 'IB001', name: '商品', customer: '客户', destination: '美西', qty: 12, checked: false, issues: [], ...overrides });
test('原表删行后仍保留数量、入库号和出货状态，并标注存档', () => {
  const archived = row({ checked: true });
  const result = withArchiveRows([], [archived]);
  assert.equal(result.length, 1);
  assert.equal(result[0].inbound, 'IB001');
  assert.equal(result[0].qty, 12);
  assert.equal(result[0].checked, true);
  assert.ok(result[0].issues.includes(HISTORY_ISSUE));
  assert.equal(archived.issues.length, 0);
});
test('原表优先且不重复计算，恢复原表时移除历史标注', () => {
  const current = row({ qty: 20 });
  const result = withArchiveRows([current], [row(), row()]);
  assert.equal(result.length, 1);
  assert.equal(result[0].qty, 20);
  assert.ok(!result[0].issues.includes(HISTORY_ISSUE));
});
test('补入库号后不再补出旧的缺号记录', () => {
  assert.equal(withArchiveRows([row()], [row({ inbound: '' })]).length, 1);
});
test('6812705712 原表清空单号后 16 行 192 件只保留一份且恢复已出', () => {
  const quantities = [4,25,10,40,20,20,2,2,4,50,2,2,2,5,2,2];
  const archived = quantities.map((qty, i) => row({ name: `商品${i}`, inbound: `IB${i}`, qty, checked: true }));
  const source = archived.map((r) => ({ ...r, waybill: '', checked: false }));
  const result = withArchiveRows(source, archived);
  assert.equal(result.length, 16);
  assert.equal(result.reduce((sum, r) => sum + r.qty, 0), 192);
  assert.ok(result.every((r) => r.waybill === '6812705712' && r.checked));
  assert.ok(result.every((r) => r.issues.includes('waybill_archive')));
  assert.equal(result.filter((r) => !r.waybill).length, 0);
  assert.equal(source[0].waybill, '');
});
test('同名无入库号、数量变化、多个候选票号及原表重复行不自动恢复单号', () => {
  for (const [source, archived] of [
    [[row({ waybill: '', inbound: '' })], [row({ inbound: '' })]],
    [[row({ waybill: '', qty: 13 })], [row()]],
    [[row({ waybill: '' })], [row(), row({ waybill: '其他票' })]],
    [[row({ waybill: '' }), row({ waybill: '' })], [row()]],
  ]) {
    const result = withArchiveRows(source, archived);
    assert.ok(result.slice(0, source.length).every((r) => r.waybill === ''));
  }
});
test('原表已有其他单号不覆盖，恢复单号尊重最新撤回状态', () => {
  assert.equal(withArchiveRows([row({ waybill: '新票' })], [row()])[0].waybill, '新票');
  const checks = { [JSON.stringify(['6812705712', 'IB001', '商品'])]: false };
  const result = withArchiveRows([row({ waybill: '' })], [row({ checked: true })], checks);
  assert.equal(result.length, 1);
  assert.equal(result[0].checked, false);
});