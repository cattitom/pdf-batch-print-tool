const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const vm=require('node:vm');
const ts=require('typescript');
function setup() {
  const slots=[]; let cursor=0; const calls=[];
  const row={waybill:'W',inbound:'I',name:'货品',checked:false};
  const options={rowsRef:{current:[row]},checkedRef:{current:{}},mobileVersionsRef:{current:[]},sheetGenerationRef:{current:'source-a'},
    setRows:value=>{options.rowsRef.current=value;},setSavedPath:()=>{},setError:()=>{}};
  const api={setPackingChecks:async(...args)=>calls.push(args),mobileRows:async()=>[]};
  const react={useState(initial){const i=cursor++;if(!(i in slots))slots[i]=initial;return[slots[i],v=>slots[i]=v];},
    useRef(initial){const i=cursor++;return slots[i] ||= {current:initial};},useEffect(){cursor++;}};
  function load(file) {
    const code=ts.transpileModule(fs.readFileSync(path.join(__dirname,'../src/pages/packing/',file),'utf8'),
      {compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText;
    const module={exports:{}};
    vm.runInNewContext(code,{module,exports:module.exports,require:n=> n==='react'?react:n==='./recordKey'?load('recordKey.ts'):{api,errorText:String,errorHint:()=>''}});
    return module.exports;
  }
  const {usePackingChecks}=load('usePackingChecks.ts');
  const render=()=>{cursor=0;return usePackingChecks(options);};
  return {api,options,calls,row,render};
}
test('数据库成功前不更新界面，失败可显式重试',async()=>{
  const h=setup();h.api.setPackingChecks=async()=>{throw new Error('写入失败');};
  assert.equal(await h.render().saveChecks([{...h.row,checked:true}]),false);
  assert.equal(h.options.rowsRef.current[0].checked,false);
  assert.match(h.render().checkFailure.message,/写入失败/);
  h.api.setPackingChecks=async()=>{};
  assert.equal(await h.render().saveChecks(h.render().checkFailure.changes,true),true);
  assert.equal(h.options.rowsRef.current[0].checked,true);
  assert.equal(h.render().checkFailure,null);
});
test('换表后拒绝重试旧操作',async()=>{
  const h=setup();h.options.sheetGenerationRef.current='source-b';
  assert.equal(await h.render().saveChecks([{...h.row,checked:true}],true,'source-a'),false);
  assert.equal(h.calls.length,0);
});
test('手机快照只更新界面，不发起数据库回写',()=>{
  const h=setup();h.render().applyMobileSnapshot([{...h.row,checked:true,sourceId:'source-a'}]);
  assert.equal(h.options.rowsRef.current[0].checked,true);assert.equal(h.calls.length,0);
});

test('缺任一号码均禁止出货，两个号码齐全才允许',async()=>{
  for(const ids of [{waybill:'',inbound:'I'},{waybill:'W',inbound:''},{waybill:' ',inbound:'I'}]) {
    const h=setup();
    assert.equal(await h.render().saveChecks([{...h.row,...ids,checked:true}]),false);
    assert.equal(h.calls.length,0);
  }
  const h=setup();
  assert.equal(await h.render().saveChecks([{...h.row,checked:true}]),true);
  assert.equal(h.calls.length,1);
});
test('缺号码的旧已出记录仍可撤回',async()=>{
  const h=setup();
  assert.equal(await h.render().saveChecks([{...h.row,waybill:'',inbound:'',checked:false}]),true);
  assert.equal(h.calls.length,1);
});