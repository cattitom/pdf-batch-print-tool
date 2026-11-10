const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');

function harness() {
  const slots = []; let cursor = 0;
  const effects=[], timers=new Map(), events=new Map();let now=1000000, timerId=0;
  const document={hidden:false,addEventListener:(n,fn)=>events.set(n,fn),removeEventListener:n=>events.delete(n)};
  const window={
    setTimeout:(fn,ms)=>{const id=++timerId;timers.set(id,{fn,due:now+ms,interval:false});return id;},
    clearTimeout:id=>timers.delete(id),
    setInterval:(fn,ms)=>{const id=++timerId;timers.set(id,{fn,due:now+ms,interval:true});return id;},
    clearInterval:id=>timers.delete(id),addEventListener:(n,fn)=>events.set(n,fn),removeEventListener:n=>events.delete(n),
  };
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
    useEffect(fn) { cursor++;effects.push(fn); },
  };
  const source=fs.readFileSync(path.join(__dirname,'../src/pages/packing/useCloudSync.ts'),'utf8');
  const code=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText;
  const module={exports:{}};
  vm.runInNewContext(code,{module,exports:module.exports,window,document,Date:{now:()=>now},require:n=>n==='react'?react:{api}});
  const options={autoSync:false,onApplied:s=>applied.push(s),onHint:()=>{},onError:e=>errors.push(e),
    errorText:e=>String(e),hasExternalFile:()=>false,hasRows:()=>true};
  const render=()=>{cursor=0;return module.exports.useCloudSync(options);};
  const flush=()=>new Promise(r=>setImmediate(r));
  return {api,candidate,calls,saved,applied,errors,render,settings,options,heartbeatDelay:module.exports.heartbeatDelay,
    effects,timers,events,document,flush,setNow:v=>{now=v;},
    nextDelay:()=>Math.min(...[...timers.values()].filter(t=>!t.interval).map(t=>t.due-now)),
    advance:async ms=>{now+=ms;for(const[id,t]of [...timers])if(!t.interval&&t.due<=now){timers.delete(id);await t.fn();}await flush();},
  };
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

test('开启自动同步后后台应用新版本，保持当前选表',async()=>{
 const h=harness();h.options.autoSync=true;
 await h.render().sync(undefined,{background:true});
 assert.equal(h.calls[0][0],'手选表');assert.equal(h.applied.length,1);
 assert.equal(JSON.parse(h.settings.kdocs_applied_cache).fver,'2');
 assert.equal(h.render().cloudUpdate,null);
});

test('保存出货期间暂缓应用，完成后下一次检查自动应用',async()=>{
 const h=harness();h.options.autoSync=true;h.options.canApply=()=>false;
 await h.render().sync(undefined,{background:true});assert.equal(h.applied.length,0);
 assert.equal(JSON.parse(h.settings.kdocs_applied_cache).fver,'1');
 h.options.canApply=()=>true;
 await h.render().sync(undefined,{background:true});assert.equal(h.applied.length,1);
});

test('等待异步铺表成功后才完成同步，失败还原已应用版本',async()=>{
 const h=harness();h.options.autoSync=true;let finish;
 h.options.onApplied=()=>new Promise(r=>{finish=r;});
 const pending=h.render().sync(undefined,{background:true});
 while(!finish)await new Promise(r=>setImmediate(r));
 assert.equal(h.render().syncing,true);
 finish(false);await pending;
 assert.equal(JSON.parse(h.settings.kdocs_applied_cache).fver,'1');
 assert.equal(h.render().syncing,false);
});

test('手机手动刷新绕过待确认旧候选，强制检查远端并返回失败',async()=>{
 const h=harness();await h.render().sync(undefined,{background:true});
 assert.equal(h.calls.length,1);
 await h.render().sync('手选表',{refreshRemote:true,throwOnError:true});
 assert.equal(h.calls.length,2);assert.equal(h.calls[1][1],true);
 h.api.syncKdocsPacking=async()=>{throw new Error('WPS登录已过期');};
 await assert.rejects(h.render().sync('手选表',{refreshRemote:true,throwOnError:true}),/WPS登录已过期/);
});

test('动态心跳活跃时逐步降频、空闲放慢、失败指数退避并封顶',()=>{
 const {heartbeatDelay:delay}=harness();
 assert.equal(delay(true,0,0),60000);
 assert.equal(delay(true,1,0),120000);
 assert.equal(delay(true,2,0),300000);
 assert.equal(delay(true,99,0),300000);
 assert.equal(delay(false,0,0),600000);
 assert.equal(delay(true,0,1),60000);
 assert.equal(delay(true,0,2),120000);
 assert.equal(delay(true,0,3),240000);
 assert.equal(delay(false,0,99),900000);
});

test('手动刷新立即执行，原心跳到期时不会紧接着再次检查',async()=>{
 const h=harness();h.options.autoSync=true;h.render();h.effects.at(-1)();await h.flush();
 assert.equal(h.nextDelay(),60000);
 await h.advance(59000);assert.equal(h.calls.length,0);
 await h.render().sync(undefined,{refreshRemote:true,throwOnError:true});assert.equal(h.calls.length,1);
 await h.advance(1000);assert.equal(h.calls.length,1);assert.equal(h.nextDelay(),59000);
 await h.advance(59000);assert.equal(h.calls.length,2);
});

test('空闲改为慢心跳，手机活跃能唤醒等待；关闭心跳不创建计时器',async()=>{
 const off=harness();off.render();off.effects.at(-1)();await off.flush();assert.equal(off.timers.size,0);
 const h=harness();h.options.autoSync=true;h.render();h.effects.at(-1)();await h.flush();
 h.setNow(1120000);h.document.hidden=true;h.events.get('visibilitychange')();await h.flush();
 assert.equal(h.nextDelay(),480000);
 h.options.mobileActive=async()=>true;
 await [...h.timers.values()].find(t=>t.interval).fn();await h.flush();
 assert.equal(h.nextDelay(),1000);
});
