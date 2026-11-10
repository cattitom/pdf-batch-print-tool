const {test}=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm'),ts=require('typescript'),path=require('node:path');
const moduleValue={exports:{}};
vm.runInNewContext(ts.transpileModule(fs.readFileSync(path.join(__dirname,'../src/pages/packing/masterRows.ts'),'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText,{module:moduleValue,exports:moduleValue.exports});
const {applyMasterOverrides,masterIdentity}=moduleValue.exports;
const row={seq:10,inbound:'I',name:'唇线笔',destination:'仓库',customer:'杨总',qty:2,checked:false,issues:[],qtyRaw:'2',cargo:{},waybill:''};
test('重复原行不共用旧的宽泛同步ID和人工数量覆盖',()=>{
 const rows=[row,{...row,seq:16}],map={[JSON.stringify(['I','唇线笔','仓库'])]:{syncId:'old',qty:1}};
 const out=applyMasterOverrides(rows,JSON.stringify(map));assert.ok(out.every(r=>!r.syncId));assert.equal(out.reduce((n,r)=>n+r.qty,0),4);
});
test('行级修订只作用对应明细，唯一行仍兼容旧版修订',()=>{
 const rows=[row,{...row,seq:16}],map={[masterIdentity(row)]:{syncId:'first',customer:'修订客户'}};
 const out=applyMasterOverrides(rows,JSON.stringify(map));assert.equal(out[0].customer,'修订客户');assert.equal(out[1].customer,'杨总');assert.equal(out[1].syncId,undefined);
 const legacy={[JSON.stringify(['I','唇线笔','仓库'])]:{syncId:'legacy'}};assert.equal(applyMasterOverrides([row],JSON.stringify(legacy))[0].syncId,'legacy');
});

test('原表合箱不会被旧手机快照恢复成重复箱数',()=>{
 const shared={...row,sourceRow:375,quantityGroup:'I374:I375',qty:0,qtyRaw:'1',issues:['qty_shared']};
 const map={[masterIdentity(shared)]:{qty:1,qtyRaw:'1',checked:true,issues:[]}};
 const out=applyMasterOverrides([shared],JSON.stringify(map))[0];
 assert.equal(out.qty,0);assert.equal(out.checked,true);assert.ok(out.issues.includes('qty_shared'));
 assert.equal(out.sourceRow,375);assert.equal(out.quantityGroup,'I374:I375');
});

test('云表模式只应用手机单号，不应用旧数量客户或已出状态',()=>{
 const patch={[masterIdentity(row)]:{qty:99,checked:true,customer:'手机旧客户',waybill:'PHONE',manualFields:['checked','qty','waybill']}};
 const result=applyMasterOverrides([row],JSON.stringify(patch),true)[0];
 assert.equal(result.qty,2);assert.equal(result.checked,false);assert.equal(result.customer,'杨总');assert.equal(result.waybill,'PHONE');
 assert.equal(result.manualFields.join(','),'waybill');
});

test('云表已填单号优先，手机单号记忆不能覆盖云表号码',()=>{
 const cloud={...row,waybill:'CLOUD'};const patch={[masterIdentity(cloud)]:{waybill:'PHONE'}};
 assert.equal(applyMasterOverrides([cloud],JSON.stringify(patch),true)[0].waybill,'CLOUD');
});

test('台账模式恢复出货状态，云表货物资料仍为准',()=>{
 const patch={[masterIdentity(row)]:{qty:99,customer:'旧客户',waybill:'W',checked:true,manualFields:['qty','customer','waybill','checked']}};
 const result=applyMasterOverrides([row],JSON.stringify(patch),true,true)[0];
 assert.equal(result.qty,2);assert.equal(result.customer,'杨总');assert.equal(result.checked,true);assert.equal(result.waybill,'W');
 assert.equal(result.manualFields.join(','),'waybill,checked');
});
