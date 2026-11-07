const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const vm=require('node:vm');
const ts=require('typescript');
const archived={waybill:'6812705712',inbound:'IB1',name:'商品',customer:'客户',destination:'美西',qty:192,checked:true,issues:[]};
const source={...archived,waybill:'',checked:false};
function setup(){
 const slots=[],effects=[];let cursor=0,resolveSettings,rejectSettings;
 const settingsPromise=new Promise((resolve,reject)=>{resolveSettings=resolve;rejectSettings=reject;});
 const react={
  useState(initial){const i=cursor++;if(!(i in slots))slots[i]=initial;return[slots[i],v=>{slots[i]=typeof v==='function'?v(slots[i]):v;}];},
  useRef(initial){const i=cursor++;return slots[i]||=( {current:initial} );},
  useCallback(fn){const i=cursor++;return slots[i]||=fn;},
  useEffect(fn,deps){const i=cursor++;if(!slots[i]||deps.some((d,n)=>d!==slots[i][n]))effects.push(fn);slots[i]=deps;},
 };
 const api={allSettings:()=>settingsPromise,setSetting:async()=>{}};
 function load(file){const m={exports:{}};const s=fs.readFileSync(path.join(__dirname,'../src/pages/packing',file),'utf8');const code=ts.transpileModule(s,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText;
 vm.runInNewContext(code,{module:m,exports:m.exports,require:n=>n==='react'?react:n==='./archiveRows'?load('archiveRows.ts'):{api,errorText:String}});return m.exports;}
 const hook=load('usePackingArchive.ts').usePackingArchive;
 const render=(rows=[])=>{cursor=0;const result=hook(rows);while(effects.length)effects.shift()();return result;};
 const resolve=async()=>{resolveSettings({'packing_archive_v1:6812705712':JSON.stringify({waybill:'6812705712',items:[archived]})});await settingsPromise;await Promise.resolve();};
 return{render,resolve,reject:async()=>{rejectSettings(Error('读取失败'));await settingsPromise.catch(()=>{});await Promise.resolve();}};
}
test('首帧创建的原表恢复回调也必须使用后来读出的存档，不依赖云同步',async()=>{
 const h=setup(),initial=h.render();const restore=initial.mergeArchiveRows;
 await h.resolve();
 const result=restore([source]);
 assert.equal(result.length,1);assert.equal(result[0].waybill,'6812705712');assert.equal(result[0].qty,192);assert.equal(result[0].checked,true);
});
test('原表先恢复、存档后加载时，重新合并立即恢复单号',async()=>{
 const h=setup(),initial=h.render();const first=initial.mergeArchiveRows([source]);assert.equal(first[0].waybill,'');
 await h.resolve();const loaded=h.render();assert.equal(loaded.archiveItems.length,1);
 const next=loaded.mergeArchiveRows(first);assert.equal(next.length,1);assert.equal(next[0].waybill,'6812705712');
});
test('存档先显示再载入原表不会丢失，也不会重复计数',async()=>{
 const h=setup(),initial=h.render();await h.resolve();
 assert.equal(initial.mergeArchiveRows([]).length,1);
 const next=initial.mergeArchiveRows([source]);assert.equal(next.length,1);assert.equal(next.reduce((n,r)=>n+r.qty,0),192);
});
test('读取失败明确报告错误，仍允许显示原表',async()=>{
 const h=setup(),initial=h.render();await h.reject();const state=h.render();assert.match(state.archiveError,/读取历史存档失败/);
 assert.equal(initial.mergeArchiveRows([source])[0].waybill,'');
});