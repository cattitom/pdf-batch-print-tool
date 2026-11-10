const {test}=require('node:test');const assert=require('node:assert/strict');const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const script=fs.readFileSync(path.join(__dirname,'../../ios-shell/web/shipping/transport.js'),'utf8');
const token='a'.repeat(64),company='http://192.168.1.242:17650/?t='+token,home='http://192.168.6.125:17650/?t='+token;
const row={waybill:'W',inbound:'I',name:'商品',customer:'九宸',qty:7,checked:false,issues:[]};
const clone=x=>JSON.parse(JSON.stringify(x));
function setup({storage,saved,failSave=false,pending=[]}={}){
 const store=storage||new Map([['shipping-shell.current',JSON.stringify({url:company})],['shipping-shell.cache',JSON.stringify({url:company,at:1,rows:[row]})],['shipping-shell.pending',JSON.stringify(pending)]]);
 const database=saved||{value:null};const calls=[];let fail=failSave;
 let handler=async()=>{throw new TypeError('offline');};
 const window={shippingMasterStorage:{load:async()=>clone(database.value),save:async v=>{if(fail)throw Error('quota');database.value=clone(v);}}};
 vm.runInNewContext(script,{window,URL,Uint8Array,AbortController,Promise,Date,Math,setTimeout,clearTimeout,localStorage:{getItem:k=>store.get(k)||null,setItem:(k,v)=>store.set(k,v)},fetch:async(u,o)=>{const body=o.body?JSON.parse(o.body):null;calls.push({url:u,body});return handler(u,o,body);}});
 return{api:window.shippingTransport,store,database,calls,setHandler:fn=>handler=fn,setFail:v=>{fail=v;}};
}
const ok=(rows,extra={})=>({ok:true,status:200,json:async()=>({protocol:1,mode:'shipment-ledger',computerId:'pc',revision:1,rows,conflicts:[],operations:[],...extra})});

test('旧主库迁移保留单号和完整备份，使用完整台账同步单号和出货状态',async()=>{
 const h=setup({pending:[{...row,checked:true}]});await h.api.ready;
 assert.equal(h.database.value.authorityMode,'waybill-notebook');
 assert.ok(h.database.value.notebookMigrationBackup);assert.equal(h.database.value.operations.length,0);
 assert.equal(h.api.syncInfo().waybillNotes[0].waybill,'W');
 await h.api.sheet();assert.equal(h.calls[0].body.mode,'shipment-ledger');assert.equal(h.calls[0].body.rows.length,1);
 assert.equal(h.calls[0].body.waybillNotes[0].waybill,'W');
 const restarted=setup({storage:h.store,saved:h.database});await restarted.api.ready;assert.equal(restarted.api.syncInfo().waybillNotes[0].waybill,'W');
});

test('35箱旧总行被云表20已出加15未出替代，旧单号保留而不阻断同步',async()=>{
 const h=setup();await h.api.ready;
 const split=[{...row,syncId:'first',sourceRow:273,seq:97,qty:20,checked:true,waybill:''},{...row,syncId:'second',sourceRow:291,seq:115,qty:15,checked:false,waybill:''}];
 h.setHandler(async()=>ok(split));await h.api.sheet();
 assert.equal(h.api.cached().length,2);assert.equal(h.api.cached().reduce((n,r)=>n+r.qty,0),35);
 assert.equal(h.api.cached()[0].checked,true);assert.equal(h.api.cached()[1].checked,false);assert.equal(h.api.syncInfo().conflicts.length,0);
 const note=h.api.syncInfo().waybillNotes[0];assert.equal(note.waybill,'W');assert.equal(note.needsMatch,true);assert.equal(note.candidates.length,2);
 await h.api.linkWaybill(note.id,'second');
 assert.ok(h.calls.at(-1).body.waybillNotes[0].targetIdentity.endsWith(',291]'));
});

test('手机离线保存单号与出货状态，货物资料禁止回写',async()=>{
 const h=setup();await h.api.ready;
 await h.api.edit(h.api.cached()[0],{waybill:'PHONE-NEW'});assert.equal(h.api.cached()[0].waybill,'PHONE-NEW');
 assert.ok(h.api.syncInfo().waybillNotes.some(n=>n.waybill==='PHONE-NEW'));
 await h.api.check(h.api.cached()[0],true);assert.equal(h.api.cached()[0].checked,true);
 await assert.rejects(h.api.edit(h.api.cached()[0],{customer:'不能改'}),/仅修改单号和出货状态/);
 assert.equal(h.api.cached()[0].customer,'九宸');assert.equal(h.api.cached()[0].checked,true);
});

test('手机保存失败不改行、不发请求，也不误报单号已保存',async()=>{
 const h=setup();await h.api.ready;h.setFail(true);
 await assert.rejects(h.api.edit(h.api.cached()[0],{waybill:'NEW'}),/quota/);
 assert.equal(h.api.cached()[0].waybill,'W');assert.equal(h.calls.length,0);
});

test('换电脑携带完整台账与单号记忆，旧请求分别保存',async()=>{
 const h=setup();await h.api.ready;await h.api.sheet();
 h.store.set('shipping-shell.current',JSON.stringify({url:home}));await h.api.sheet();
 assert.equal(h.api.syncInfo().otherPending,1);assert.ok(h.calls[1].body.waybillNotes.some(n=>n.waybill==='W'));assert.equal(h.calls[1].body.rows.length,1);
 h.setHandler(async()=>ok([{...row,syncId:'home-row'}],{computerId:'home'}));await h.api.sheet();
 assert.equal(h.api.syncInfo().pending,0);assert.equal(h.api.syncInfo().otherPending,1);
});

test('重试保留请求ID，新录入的单号不会被旧响应覆盖',async()=>{
 const h=setup();await h.api.ready;const id=h.api.cached()[0].syncId;
 await h.api.sheet();const first=h.calls[0].body;
 await h.api.edit(h.api.cached()[0],{waybill:'LATEST'});
 h.setHandler(async()=>ok([{...row,syncId:id}]));await h.api.sheet();
 assert.equal(h.calls.at(-1).body.requestId,first.requestId);assert.equal(h.api.cached()[0].waybill,'LATEST');
 assert.ok(h.api.syncInfo().waybillNotes.some(n=>n.waybill==='LATEST'));
});

test('电脑单号和手机记忆不同时各自保留，云表资料不弹JSON冲突',async()=>{
 const h=setup();await h.api.ready;h.setHandler(async()=>ok([{...row,syncId:'cloud',waybill:'CLOUD',customer:'云表客户',qty:19}]));
 await h.api.sheet();assert.equal(h.api.cached()[0].waybill,'CLOUD');assert.equal(h.api.cached()[0].customer,'云表客户');assert.equal(h.api.cached()[0].qty,19);
 const notes=h.api.syncInfo().waybillNotes;assert.ok(notes.some(n=>n.waybill==='W'));assert.ok(notes.some(n=>n.waybill==='CLOUD'));assert.equal(h.api.syncInfo().conflicts.length,0);
 assert.equal(h.api.exportInfo().source,'本地出货台账');assert.ok(!JSON.stringify(h.api.exportInfo()).includes(token));
});

test('旧协议或配对失效不丢单号记忆和手机快照',async()=>{
 const h=setup();await h.api.ready;h.setHandler(async()=>({ok:false,status:403,json:async()=>({})}));await h.api.sheet();
 assert.equal(h.api.syncInfo().waybillNotes[0].waybill,'W');assert.match(h.api.status(),/配对已失效/);
 h.setHandler(async()=>ok([],{mode:''}));await h.api.sheet();assert.equal(h.api.cached().length,1);assert.equal(h.api.syncInfo().waybillNotes[0].waybill,'W');
});

test('手机已有完整10行21件，云表半张响应不能清除已出票据历史',async()=>{
 const h=setup();await h.api.ready;
 const full=[4,1,6,1,2,1,2,2,1,1].map((qty,i)=>({...row,syncId:'id'+i,name:'商品'+i,inbound:'I'+i,qty,checked:true,sourceRow:382+i}));
 h.setHandler(async()=>ok(full));await h.api.sheet();
 h.setHandler(async()=>ok(full.slice(5)));await h.api.sheet();
 assert.equal(h.api.cached().length,10);assert.equal(h.api.cached().reduce((n,r)=>n+r.qty,0),21);
});

test('云表补填目的地后同一票不重复存档，真实新增的行仍独立保留',async()=>{
 const h=setup();await h.api.ready;
 await h.api.check(h.api.cached()[0],true);
 h.setHandler(async()=>ok([{...row,syncId:'merged',destination:'休斯顿',checked:true}]));
 await h.api.sheet();
 assert.equal(h.api.cached().length,1);assert.equal(h.api.cached()[0].destination,'休斯顿');
 h.setHandler(async()=>ok([{...row,syncId:'merged',destination:'休斯顿',checked:true},{...row,syncId:'extra',inbound:'I2',name:'新商品',destination:'休斯顿',checked:true,waybill:'W2'}]));
 await h.api.sheet();
 const cached=h.api.cached();
 assert.equal(cached.length,2);assert.ok(cached.some(r=>r.inbound==='I2'&&r.name==='新商品'));
});

test('旧版遗留的同货重复行在新版首次同步后清理',async()=>{
 // 旧版 transport 只按 syncId 匹配，电脑重分 ID 后会把旧副本盲目加回，形成同货重复行
 const legacy=[{...row,syncId:'legacy-a',checked:true},{...row,syncId:'legacy-b',checked:true}];
 const store=new Map([['shipping-shell.current',JSON.stringify({url:company})],['shipping-shell.cache',JSON.stringify({at:1,rows:legacy})],['shipping-shell.pending',JSON.stringify([])]]);
 const h=setup({storage:store});await h.api.ready;
 h.setHandler(async()=>ok([{...row,syncId:'server-1',checked:true}]));
 await h.api.sheet();
 const cached=h.api.cached();
 assert.equal(cached.length,1);assert.equal(cached.reduce((n,r)=>n+r.qty,0),7);
});
