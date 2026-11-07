const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const vm=require('node:vm');
const html=fs.readFileSync(path.join(__dirname,'../../ios-shell/web/index.html'),'utf8');
const fn=html.slice(html.indexOf('  function connectPeer('),html.indexOf('  function connectUrl('));
function setup(history=[]) {
  const connected=[];const rejected=[];
  const ctx={URL,loadAuto:()=>null,loadHistory:()=>history,
    showNotFound:m=>rejected.push(m),connectUrl:(...args)=>connected.push(args),saveAuto:()=>{}};
  vm.runInNewContext(fn,ctx);
  return {...ctx,connected,rejected};
}
test('发现电脑不等于配对，广播中的伪造口令也不能直接授权',()=>{
  const h=setup();
  h.connectPeer({host:'192.168.1.2',url:'http://192.168.1.2:17650/?t='+ 'a'.repeat(64),token:'a'.repeat(64)},false);
  assert.equal(h.connected.length,0);assert.match(h.rejected[0],/扫码|扫描/);
});
test('已扫码的相同源电脑可使用本机保存链接重连',()=>{
  const url='http://192.168.1.2:17650/?t='+ 'a'.repeat(64);
  const h=setup([{url}]);
  h.connectPeer({host:'192.168.1.2',url:'http://192.168.1.2:17650/'},false);
  assert.equal(h.connected[0][0],url);
});
test('不会把保存的配对凭证发送到不同端口或其他主机',()=>{
  const h=setup([{url:'http://192.168.1.2:17650/?t='+ 'a'.repeat(64)}]);
  h.connectPeer({host:'192.168.1.2',url:'http://192.168.1.2:17651/'},false);
  h.connectPeer({host:'192.168.1.3',url:'http://192.168.1.3:17650/'},false);
  assert.equal(h.connected.length,0);assert.equal(h.rejected.length,2);
});
