const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');

function harness() {
  const slots = []; let cursor = 0;
  const initial = {sheet:'手选表', sheets:['手选表','新表'], fver:'1', path:'old.xls'};
  const settings = {kdocs_applied_cache:JSON.stringify(initial)};
  const saved = []; const calls = []; const applied = []; const errors = [];
  const candidate = {sheet:'手选表', sheets:['手选表','新表'], fver:'2', path:'new.xls', changed:false,
    packing:{items:[{name:'new'}]}};
  const api = {
    allSettings:async()=>({...settings}), snapshotKdocsCache:async(p)=>p,
    setSetting:async(k,v)=>{ saved.push([k,JSON.parse(v)]); settings[k]=v; },
    syncKdocsPacking:async(...args)=>{calls.push(args);return candidate;},
    parsePackingXls:async(p,s)=>({items:[{name:p,sheet:s}]}),
  };
  const react = {
    useState(initial) { const i=cursor++; if (!(i in slots)) slots[i]=initial;
      return [slots[i],value=>{slots[i]=typeof value==='function'?value(slots[i]):value;}]; },
    useRef(initial) { const i=cursor++; return slots[i] ||= {current:initial}; },
    useCallback(fn) { const i=cursor++; return slots[i] ||= fn; },
    useEffect() { cursor++; },
  };
  const source=fs.readFileSync(path.join(__dirname,'../src/pages/packing/useCloudSync.ts'),'utf8');
  const code=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText;
  const module={exports:{}};
  vm.runInNewContext(code,{module,exports:module.exports,require:n=>n==='react'?react:{api}});
  const options={autoSync:false,onApplied:s=>applied.push(s),onHint:()=>{},onError:e=>errors.push(e),
    errorText:e=>String(e),hasExternalFile:()=>false,hasRows:()=>true};
  const render=()=>{cursor=0;return module.exports.useCloudSync(options);};
  return {api,candidate,calls,saved,applied,errors,render,settings,options};
}

test('后台保持选表，待更新不替换当前元数据，确认后才应用',async()=>{
  const h=harness(); await h.render().sync(undefined,{background:true});
  assert.equal(h.calls[0][0],'手选表'); assert.equal(h.calls[0][1],true);
  assert.equal(h.applied.length,0); assert.equal(h.render().cloudSheet,'手选表');
  assert.equal(h.render().cloudUpdate,'2');
  assert.ok(h.saved.every(([,cache])=>cache.path==='old.xls'));
  await h.render().sync('手选表');
  assert.equal(h.calls.length,1); assert.equal(h.applied.length,1);
  assert.equal(JSON.parse(h.settings.kdocs_applied_cache).path,'new.xls');
});

test('尚未确认的更新不会改变离线恢复使用的路径',async()=>{
  const h=harness(); await h.render().sync(undefined,{background:true});
  await h.render().openCached();
  assert.equal(h.applied[0].items[0].name,'old.xls');
  assert.equal(h.applied[0].items[0].sheet,'手选表');
});

test('缓存提交失败时不应用候选数据，也不改变当前表名',async()=>{
  const h=harness(); await h.render().sync(undefined,{background:true});
  h.api.setSetting=async()=>{throw new Error('数据库写入失败');};
  await h.render().sync('手选表');
  assert.equal(h.applied.length,0); assert.equal(h.errors.length,1);
  assert.equal(h.render().cloudSheet,'手选表');
});

test('同步运行中不启动第二个手动请求',async()=>{
  const h=harness(); let release;
  h.api.syncKdocsPacking=async(...args)=>{h.calls.push(args);await new Promise(r=>release=r);return h.candidate;};
  const first=h.render().sync();
  while(!release) await new Promise(r=>setImmediate(r));
  await h.render().sync('新表'); assert.equal(h.calls.length,1);
  release(); await first;
});

test('自动更新和启动兜底均不覆盖当前本地文件',async()=>{
  const h=harness(); h.options.autoSync=true; h.options.hasExternalFile=()=>true;
  await h.render().sync(undefined,{background:true,applyAnyway:true});
  assert.equal(h.applied.length,0); assert.equal(h.render().cloudUpdate,'2');
});
