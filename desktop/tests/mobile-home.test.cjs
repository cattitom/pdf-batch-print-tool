const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const vm=require('node:vm');
function setup(){
 const html=fs.readFileSync(path.join(__dirname,'../../ios-shell/web/app.html'),'utf8');
 const elements=new Map(),calls=[];let resolveDiscovery;
 function element(id){
  if(elements.has(id))return elements.get(id);
  const classes=new Set();
  if(['gate','fab','settings','local','home-back','found','notfound','scanning'].includes(id))classes.add('hide');
  const el={style:{},src:'',value:'',listeners:{},children:[],classList:{add:c=>classes.add(c),remove:c=>classes.delete(c),contains:c=>classes.has(c),toggle(c,on){if(on)classes.add(c);else classes.delete(c);}},
   addEventListener(n,fn){this.listeners[n]=fn;},appendChild(v){this.children.push(v);},setAttribute(){},removeAttribute(n){if(n==='src')this.src='';}};
  elements.set(id,el);return el;
 }
 const document={getElementById:element,body:element('body'),hidden:false,addEventListener(){},createElement:()=>element(Symbol())};
 const window={__TAURI__:{core:{invoke:(name)=>{calls.push(name);return new Promise(resolve=>{resolveDiscovery=resolve;});}}},addEventListener(){}};
 const storage=new Map();
 vm.runInNewContext(html.match(/<script>([\s\S]*?)<\/script>/)[1],{window,document,localStorage:{getItem:k=>storage.get(k)||null,setItem:(k,v)=>storage.set(k,v),removeItem:k=>storage.delete(k)},URL,Promise,Date,console,setTimeout:()=>1,clearTimeout(){},setInterval:()=>1,clearInterval(){},confirm:()=>false});
 return {element,calls,resolve:peer=>resolveDiscovery(peer)};
}
test('启动直接显示工作台，不连接电脑',()=>{
 const h=setup();assert.equal(h.calls.length,0);assert.equal(h.element('home').classList.contains('hide'),false);assert.equal(h.element('gate').classList.contains('hide'),true);
});
test('未连接也能从统一入口打开清单与设置并返回',()=>{
 const h=setup();h.element('module-warehouse').listeners.click();assert.equal(h.element('view').src,'./shipping/index.html');
 h.element('home-back').listeners.click();assert.equal(h.element('home').classList.contains('hide'),false);
 h.element('module-settings').listeners.click();assert.equal(h.element('settings').classList.contains('hide'),false);assert.equal(h.calls.length,0);
});
test('进入业务模块始终加载手机本地页面，不等待电脑发现',()=>{
 const h=setup();h.element('module-warehouse').listeners.click();
 assert.equal(h.calls.length,0);
 assert.equal(h.element('view').src,'./shipping/index.html');
 assert.equal(h.element('gate').classList.contains('hide'),true);
 h.element('home-back').listeners.click();
 assert.equal(h.element('home').classList.contains('hide'),false);
 assert.equal(h.element('view').src,'');
});
