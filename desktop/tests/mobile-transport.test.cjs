const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs'),path=require('node:path');

test('iOS内置资源与共享界面保持一致，新增主库不依赖远程HTML',()=>{
 const root=path.join(__dirname,'../..');
 for(const file of ['m.css','m.js'])assert.equal(fs.readFileSync(path.join(root,'ios-shell/web/shipping',file),'utf8'),fs.readFileSync(path.join(root,'desktop/src-tauri/mobile',file),'utf8'));
 const html=fs.readFileSync(path.join(root,'ios-shell/web/shipping/index.html'),'utf8');
 assert.match(html,/src="\.\/transport.js"/);assert.doesNotMatch(html,/src="https?:/);
});

test('主库传输接口受固定白名单约束，迁移保留旧快照和离线队列',()=>{
 const code=fs.readFileSync(path.join(__dirname,'../../ios-shell/web/shipping/transport.js'),'utf8');
 assert.match(code,/legacyBackup/);assert.match(code,/\/api\/master-sync/);assert.match(code,/不支持的接口/);
 assert.match(code,/tx.oncomplete/);assert.match(code,/outboxes/);
});
