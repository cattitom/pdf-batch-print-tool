const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),ts=require('typescript');
const source=fs.readFileSync(path.join(__dirname,'../src/pages/packing/customerMessage.ts'),'utf8');
const code=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText;
const moduleValue={exports:{}};
vm.runInNewContext(code,{module:moduleValue,exports:moduleValue.exports});
const {parseCustomerMessage,customerMessageText,customerMessageError,defaultCustomerMessage,readCustomerMessage,customerMessageKey,customerMessageOverrides}=moduleValue.exports;
const row={customer:'陈东龙',name:'面部刮痧',inbound:'IB015261107RS',waybill:'W1',destination:'休斯顿',qty:10};
const example='陈东龙\nIB015261107RS\n面部刮痧  1件装 ， 285/箱\n53×44×42/6000=16.324\n实17.6，共（10 件）';
test('解析客户示例并重新计算体积重，不把每箱285误当成总件数',()=>{
 const data={...defaultCustomerMessage(row),...parseCustomerMessage(example)};
 assert.equal(data.specification,'1件装');assert.equal(data.perBox,'285');assert.equal(data.packages,'10');
 assert.equal(customerMessageError(data),null);
 assert.equal(customerMessageText(data),'陈东龙\nIB015261107RS\n面部刮痧  1件装，285/箱\n53×44×42/6000=16.324\n实17.6，共（10 件）');
});
test('忽略旧消息里错误的计算结果，使用实际尺寸重新计算',()=>{
 const data={...defaultCustomerMessage(row),...parseCustomerMessage(example.replace('16.324','999'))};
 assert.match(customerMessageText(data),/6000=16\.324/);
});
test('尺寸不全、负数和非整数箱数不可作为完整客户资料复制',()=>{
 const base=defaultCustomerMessage(row);
 for(const delta of [{length:'53'},{length:'-1',width:'44',height:'42'},{actualWeight:'abc'},{perBox:'2.5'},{packages:'1.5'}]) assert.ok(customerMessageError({...base,...delta}));
 assert.doesNotMatch(customerMessageText(base),/6000=|实\d/);
});
test('没有入库号或规格时不编造内容、不出现undefined',()=>{
 const text=customerMessageText({...defaultCustomerMessage(row),inbound:''});
 assert.equal(text,'陈东龙\n面部刮痧\n共（10 件）');
 assert.throws(()=>parseCustomerMessage('随便一段文字'),/没有识别/);
});
test('客户资料可恢复，坏数据不会静默覆盖，以不同货物身份分开保存',()=>{
 const data={...defaultCustomerMessage(row),length:'53',width:'44',height:'42'};
 const saved=readCustomerMessage(JSON.stringify({schema:1,data,updatedAt:'2026-11-08'}),defaultCustomerMessage(row));
 assert.equal(saved.data.length,'53');assert.equal(saved.updatedAt,'2026-11-08');
 assert.throws(()=>readCustomerMessage('{"schema":1,"data":{}}',data));
 assert.notEqual(customerMessageKey(row),customerMessageKey({...row,customer:'另一客户'}));
 assert.notEqual(customerMessageKey(row),customerMessageKey({...row,inbound:'OTHER'}));
});


test('WPS规格与尺寸重量自动预填，产品名称内的包装信息自动拆分',()=>{
 const data=defaultCustomerMessage({...row,name:'面部刮痧  1件装 ， 285/箱',cargo:{length:'53',width:'44',height:'42',actualWeight:'17.6',sourceVolume:'16.32'}});
 assert.equal(data.product,'面部刮痧');assert.equal(data.specification,'1件装');assert.equal(data.perBox,'285');
 assert.equal(data.length,'53');assert.equal(data.actualWeight,'17.6');assert.match(customerMessageText(data),/6000=16\.324/);
});
test('只保存人工修改，云表更新未修改的字段而不覆盖人工实重',()=>{
 const base=defaultCustomerMessage({...row,cargo:{length:'53',width:'44',height:'42',actualWeight:'17.6'}});
 const manual={...base,actualWeight:'18'};
 const overrides=customerMessageOverrides(manual,base);
 assert.deepEqual(JSON.parse(JSON.stringify(overrides)),{actualWeight:'18'});
 const fresh=defaultCustomerMessage({...row,qty:12,cargo:{length:'55',width:'44',height:'42',actualWeight:'17.8'}});
 const restored=readCustomerMessage(JSON.stringify({schema:2,overrides}),fresh).data;
 assert.equal(restored.length,'55');assert.equal(restored.packages,'12');assert.equal(restored.actualWeight,'18');
});
