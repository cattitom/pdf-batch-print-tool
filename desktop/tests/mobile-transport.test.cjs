const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const vm=require('node:vm');
const token='a'.repeat(64), url='http://192.168.1.2:17650/?t='+token;
const row={waybill:'W1',inbound:'I1',name:'商品',checked:false,version:3,datasetId:'data1',recordId:'r1',sourceId:'s1'};
function setup(){
 const store=new Map([['shipping-shell.current',JSON.stringify({url})],['shipping-shell.cache',JSON.stringify({url,at:1,rows:[row]})]]);
 const calls=[],window={};let handler=async()=>{throw new TypeError('offline');};
 const script=fs.readFileSync(path.join(__dirname,'../../ios-shell/web/shipping/transport.js'),'utf8');
 vm.runInNewContext(script,{window,URL,AbortController,Promise,TypeError,Date,setTimeout,clearTimeout,localStorage:{getItem:k=>store.get(k)||null,setItem:(k,v)=>store.set(k,v)},fetch:async(u,o)=>{calls.push([u,o]);return handler(u,o);}});
 return {api:window.shippingTransport,store,calls,setHandler:fn=>handler=fn,read:key=>JSON.parse(store.get('shipping-shell.'+key)||'null')};
}
const response=(status,data={})=>({ok:status>=200&&status<300,status,json:async()=>data});
test('离线立即读取本地记录，不请求电脑页面',async()=>{
 const h=setup();assert.equal(h.api.cached().length,1);assert.equal(h.calls.length,0);
 const data=await h.api.sheet();assert.equal(data.offline,true);assert.equal(data.rows[0].inbound,'I1');
 assert.ok(h.calls.every(([u])=>new URL(u).pathname.startsWith('/api/')));
});
test('离线切换合并操作且保留原始版本，重连后补传再读最新清单',async()=>{
 const h=setup();await h.api.check({...row},true);await h.api.check({...row},false);await h.api.check({...row},true);
 assert.equal(h.read('pending').length,1);assert.equal(h.read('pending')[0].version,3);
 assert.equal(h.read('cache').rows[0].checked,true);assert.match(h.api.status(),/待同步/);
 h.setHandler(async(u,o)=> new URL(u).pathname==='/api/check'?response(200):response(200,{rows:[{...row,checked:true,version:4}]}));
 const data=await h.api.sheet();assert.equal(data.offline,false);assert.equal(h.read('pending').length,0);assert.equal(data.rows[0].checked,true);
});
test('版本冲突保留操作并明确提示，不覆盖电脑最新状态',async()=>{
 const h=setup();await h.api.check({...row},true);
 h.setHandler(async(u)=>new URL(u).pathname==='/api/check'?response(409,{error:'版本改变'}):response(200,{rows:[{...row,checked:false,version:9}]}));
 const data=await h.api.sheet();assert.equal(data.rows[0].checked,false);assert.equal(h.read('pending')[0].conflict,true);assert.match(h.api.status(),/冲突/);
});
test('换电脑不发送原电脑的离线操作，也不把旧快照显示成新电脑数据',async()=>{
 const h=setup();await h.api.check({...row},true);h.store.set('shipping-shell.current',JSON.stringify({url:'http://192.168.1.3:17650/?t='+token}));
 assert.equal(h.api.cached().length,0);h.calls.length=0;
 await assert.rejects(h.api.sheet());assert.ok(h.calls.every(([u])=>new URL(u).pathname!=='/api/check'));
 assert.equal(h.read('pending').length,1);
});
test('凭证失效不能伪装为保存成功，缺号码不得离线出货',async()=>{
 const h=setup();h.setHandler(async()=>response(403));
 await assert.rejects(h.api.check({...row},true),/配对已失效/);assert.equal(h.read('pending'),null);
 await assert.rejects(h.api.check({...row,inbound:''},true),/必须都填写/);
});
test('iOS 内置资源与共享界面保持一致，构建产物不依赖远程 HTML',()=>{
 const root=path.join(__dirname,'../..');
 for(const file of ['m.css','m.js'])assert.equal(fs.readFileSync(path.join(root,'ios-shell/web/shipping',file),'utf8'),fs.readFileSync(path.join(root,'desktop/src-tauri/mobile',file),'utf8'));
 const html=fs.readFileSync(path.join(root,'ios-shell/web/shipping/index.html'),'utf8');assert.match(html,/src="\.\/transport.js"/);assert.doesNotMatch(html,/src="https?:/);
});