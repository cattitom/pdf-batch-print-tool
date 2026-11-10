const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const ts=require('typescript');
const path=require('node:path');
const source=fs.readFileSync(path.join(__dirname,'../src/pages/packing/customerCorrections.ts'),'utf8');
const moduleState={exports:{}};
vm.runInNewContext(ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText,{module:moduleState,exports:moduleState.exports});
const {applyCustomerCorrections,customerCorrectionKey}=moduleState.exports;
const row={inbound:'IB191261107RU',name:'热风梳',destination:'休斯顿',customer:'张婷婷',qty:35,waybill:'6812705863',checked:true};
test('客户修订独立于旧客户和单号，刷新数量不会丢失且不改变出货状态',()=>{
 const corrections=JSON.stringify({[customerCorrectionKey(row)]:'杨总'});
 for(const input of [row,{...row,customer:'',waybill:'',qty:40}]) {
  const out=applyCustomerCorrections([input],corrections)[0];
  assert.equal(out.customer,'杨总'); assert.equal(out.checked,input.checked); assert.equal(out.waybill,input.waybill);assert.equal(out.qty,input.qty);
  assert.notEqual(out,input);
 }
 assert.equal(row.customer,'张婷婷');
 const different={...row,inbound:'OTHER'};
 assert.equal(applyCustomerCorrections([different],corrections)[0],different);
});
test('损坏、空白和非对象修订不污染原始数据',()=>{
 for(const raw of ['{','null','[]','1',JSON.stringify({[customerCorrectionKey(row)]:''})]) assert.equal(applyCustomerCorrections([row],raw)[0],row);
});
