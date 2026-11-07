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