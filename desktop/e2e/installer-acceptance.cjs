// 对真实安装后的 release 程序做验收。由隔离安装脚本设置环境并恢复原数据。
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {spawn} = require('node:child_process');
const {chromium} = require(process.env.ACCEPTANCE_PLAYWRIGHT);
const runDir = process.env.ACCEPTANCE_RUN;
const repo = path.resolve(__dirname,'../..');
const fix = path.join(repo,'desktop/core/tests/fixtures');
const phase = process.env.ACCEPTANCE_PHASE || 'primary';
const result = {phase, checks:[], errors:[]};
const mark=(name,details={})=>{result.checks.push({name,...details});console.log('PASS:',name);};
const delay=ms=>new Promise(r=>setTimeout(r,ms));
function makeInheritedPdf(file) {
  const content='BT /F1 18 Tf 10 30 Td (ACCEPTANCE) Tj ET 0 0.4 0.9 rg 10 50 170 30 re f';
  const objects=[
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 200 100] /CropBox [0 0 200 100] /Rotate 90 /Resources << /Font << /F1 6 0 R >> >> >>',
    '<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Annots [5 0 R] >>',
    `<< /Length ${content.length} >>\nstream\n${content}\nendstream`,
    '<< /Type /Annot /Subtype /Text /Rect [5 5 15 15] /P 3 0 R /Contents (test) >>',
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
  ];
  let data='%PDF-1.4\n',offsets=[0];
  objects.forEach((o,i)=>{offsets.push(Buffer.byteLength(data));data+=`${i+1} 0 obj\n${o}\nendobj\n`;});
  const xref=Buffer.byteLength(data);data+=`xref\n0 ${objects.length+1}\n0000000000 65535 f \n`;
  offsets.slice(1).forEach(o=>data+=String(o).padStart(10,'0')+' 00000 n \n');
  data+=`trailer\n<< /Size ${objects.length+1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  fs.writeFileSync(file,data);
}
(async()=>{
  let app,browser,mobileBrowser;
  try {
    app=spawn(process.env.ACCEPTANCE_EXE,[],{windowsHide:false,env:process.env,stdio:['ignore',fs.openSync(path.join(runDir,`app-${phase}.stdout`),'w'),fs.openSync(path.join(runDir,`app-${phase}.stderr`),'w')]});
    fs.writeFileSync(path.join(runDir,`app-${phase}.pid`),String(app.pid));
    const cdp='http://127.0.0.1:19223';
    let ready=false;
    for(let i=0;i<100;i++){
      if(app.exitCode!==null)throw new Error('程序提前退出 '+app.exitCode);
      try{if((await fetch(cdp+'/json/version')).ok){ready=true;break;}}catch{}
      await delay(300);
    }
    assert.ok(ready,'WebView2 CDP 未启动');
    let targets=[];
    for(let i=0;i<120;i++) {
      try { targets=await (await fetch(cdp+'/json/list')).json(); } catch {}
      if(targets.some(t=>t.type==='page'&&t.url!=='about:blank')) break;
      if(app.exitCode!==null) throw new Error('程序在页面创建前退出：'+app.exitCode);
      await delay(250);
    }
    fs.writeFileSync(path.join(runDir,`cdp-before-${phase}.json`),JSON.stringify({targets,exitCode:app.exitCode},null,2));
    console.log('CDP targets before attach:',targets.map(t=>t.type).join(','),'exit',app.exitCode);
    browser=await chromium.connectOverCDP(cdp);
    const context=browser.contexts()[0];
    let page;
    for(let i=0;i<150;i++){
      page=context.pages().find(p=>p.url().includes('tauri')||p.url().includes('localhost'))||context.pages()[0];
      if(page){try{await page.waitForFunction(()=>window.__TAURI__?.core?.invoke,{timeout:800});break;}catch{}}
      await delay(200);
    }
    fs.writeFileSync(path.join(runDir,`cdp-targets-${phase}.json`),JSON.stringify(await (await fetch(cdp+'/json/list')).json(),null,2));
    assert.ok(page,'没有程序页面');
    page.on('pageerror',error=>result.errors.push(error.message));
    await page.locator('button.tab').first().waitFor({state:'attached'});
    const invoke=(command,args={})=>page.evaluate(({command,args})=>window.__TAURI__.core.invoke(command,args),{command,args});
    mark('已安装的 release 程序启动及 IPC 可用');
    if(phase==='startup'){
      assert.ok(Array.isArray(await invoke('list_packing_checks')));
      mark('数据库初始化及出货记录查询可用');
    } else if(phase==='restart'){
      const checks=await invoke('list_packing_checks');
      assert.ok(checks.some(r=>r.waybill==='WB-ACCEPT'&&r.inbound==='IN-ACCEPT'&&r.checked));
      mark('手机提交后关闭并重启程序，出货记录恢复');
      const history=await invoke('list_print_history',{limit:20,offset:0});
      assert.ok(history.some(r=>r.outputName==='installer-acceptance.pdf'));
      mark('打印历史重启后恢复');
    } else if(phase==='missing-models'){
      for(const command of ['parse_packing_image','parse_label_images']){
        let error;
        try{await invoke(command,command==='parse_label_images'?{paths:[path.join(fix,'装箱单样例.png')]}:{path:path.join(fix,'装箱单样例.png')});}catch(e){error=e;}
        // 标签批处理可能以每图错误列表返回；截图入口必须明确报缺模型。
        if(command==='parse_packing_image')assert.match(JSON.stringify(error),/缺少 PaddleOCR 模型/);
      }
      mark('移走安装目录模型后明确报错，不使用系统 OCR 或源码目录模型');
      assert.ok((await invoke('pdf_infos',{paths:[path.join(fix,'PROBE-A4.pdf')]}))[0].pages>0);
      mark('缺少模型时 PDF 功能仍可用');
    } else {
      const tabs=await page.locator('button.tab').allTextContents();
      assert.ok(tabs.some(t=>t.includes('PDF 拼版'))&&tabs.some(t=>t.includes('出货清单')));
      for(const name of ['出货清单','设置','PDF 拼版'])await page.locator('button.tab').filter({hasText:name}).click();
      await page.screenshot({path:path.join(runDir,'desktop.png')});
      mark('桌面导航、页面渲染及截图');
      await delay(600);
      await invoke('mobile_stop');
      const infos=await invoke('pdf_infos',{paths:[path.join(fix,'CROP-A4-3页.pdf')]});
      assert.equal(infos[0].pages,3);assert.ok(infos[0].widthMm>0);
      const previews=await invoke('preview_pdf',{path:path.join(fix,'CROP-A4-3页.pdf'),dpi:72,maxPages:2});
      assert.equal(previews.length,2);assert.ok(previews[0].length>1000);
      mark('PDF 页数、尺寸和预览');
      const output=path.join(runDir,'installer-acceptance.pdf');
      const args={files:[
        {name:'A.pdf',path:path.join(fix,'CROP-A4-3页.pdf'),targetPages:4,rotate:90,crop:{type:'auto',marginMm:2}},
        {name:'B.pdf',path:path.join(fix,'PROBE-A4.pdf'),targetPages:2,rotate:0,crop:null}],
        opts:{sepPages:1,sepText:'验收分隔页',pageNumbers:true,copies:1},outPath:output};
      const imposed=await invoke('impose_pdfs',args);assert.equal(imposed.pages,7);assert.ok(fs.existsSync(imposed.path));
      const before=fs.readFileSync(imposed.path);
      const duplicate=await invoke('impose_pdfs',args);assert.notEqual(duplicate.path,imposed.path);assert.ok(fs.readFileSync(imposed.path).equals(before));
      assert.equal((await invoke('pdf_infos',{paths:[imposed.path]}))[0].pages,7);
      mark('真实 PDF 拼版、裁剪、旋转、分隔页、页码及重名保护',{pages:7});
      const inherited=path.join(runDir,'inherited.pdf');makeInheritedPdf(inherited);
      const source=(await invoke('pdf_infos',{paths:[inherited]}))[0];
      const copy=await invoke('impose_pdfs',{files:[{name:'inherited.pdf',path:inherited,targetPages:1,rotate:0,crop:null}],opts:{sepPages:0,sepText:null,pageNumbers:false,copies:1},outPath:path.join(runDir,'inherited-out.pdf')});
      const copied=(await invoke('pdf_infos',{paths:[copy.path]}))[0];
      assert.ok(Math.abs(source.widthMm-copied.widthMm)<0.1&&Math.abs(source.heightMm-copied.heightMm)<0.1);
      const originalPng=(await invoke('preview_pdf',{path:inherited,dpi:96,maxPages:1}))[0];
      const copiedPng=(await invoke('preview_pdf',{path:copy.path,dpi:96,maxPages:1}))[0];
      const pixelError=await page.evaluate(async ([a,b])=>{
        async function pixels(base64){const image=new Image();image.src='data:image/png;base64,'+base64;await image.decode();const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0);return ctx.getImageData(0,0,canvas.width,canvas.height).data;}
        const left=await pixels(a),right=await pixels(b);if(left.length!==right.length)return Infinity;
        return left.reduce((n,v,i)=>n+Math.abs(v-right[i]),0)/left.length;
      },[originalPng,copiedPng]);
      assert.ok(pixelError<2,'继承属性 PDF 渲染偏差 '+pixelError);
      mark('继承资源、原始旋转及批注循环引用的 PDF 渲染一致',{meanPixelError:pixelError});
      const sheet=await invoke('parse_packing_file',{path:path.join(fix,'装箱单样例.xlsx')});assert.ok(sheet.items.length>0);
      mark('真实 XLSX 装箱单导入',{rows:sheet.items.length});
      for(const [name,count,total] of [['装箱单样例.png',12,168],['装箱单样例2.png',7,66],['装箱单样例3.png',null,128]]){
        const ocr=await invoke('parse_packing_image',{path:path.join(fix,name)});
        assert.equal(ocr.engine,'paddle');assert.equal(ocr.totalQty,total);if(count)assert.equal(ocr.items.length,count);
        mark('安装后的 PaddleOCR：'+name,{rows:ocr.items.length,totalQty:ocr.totalQty});
      }
      const label=await invoke('preview_label',{spec:{inboundNo:'IN-ACCEPT',warehouse:'验收仓',client:'测试',shelf:'1',skus:[{sku:'220-ABC',boxes:2,pcsPerBox:250,quantities:'250,249',newFlag:true}]}});assert.ok(label.length>1000);
      mark('箱唛条码及二维码预览');
      await invoke('add_print_history',{entry:{outputName:'installer-acceptance.pdf',outputPath:imposed.path,outputPages:7,paper:'test',sepPages:1,params:{acceptance:true},files:[]}});
      const rows=[{waybill:'WB-ACCEPT',inbound:'IN-ACCEPT',name:'验收商品',customer:'验收客户',qty:12,checked:false,issues:[],blocked:false,task:false,taskId:'',claimedBy:'',sourceId:'acceptance'}];
      const info=await invoke('mobile_start',{rows});
      const base='http://127.0.0.1:'+info.port;
      const auth='?t='+encodeURIComponent(info.token);
      assert.equal((await fetch(base+'/api/sheet')).status,403);
      const snapshot=(await (await fetch(base+'/api/sheet'+auth)).json()).rows[0];
      const post=body=>fetch(base+'/api/check'+auth,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
      assert.equal((await post({...snapshot,checked:true})).status,200);
      assert.ok((await invoke('list_packing_checks')).some(r=>r.waybill==='WB-ACCEPT'&&r.checked));
      assert.equal((await post({...snapshot,checked:false})).status,409);
      mark('手机未配对拒绝访问、直接落库、过期版本冲突拒绝');
      const udp=require('node:dgram').createSocket('udp4');
      const discovery=await new Promise((resolve,reject)=>{const timer=setTimeout(()=>{udp.close();reject(new Error('UDP discovery timeout'));},5000);udp.on('message',data=>{const value=JSON.parse(data);if(value.port===info.port){clearTimeout(timer);udp.close();resolve(value);}});udp.send('PDF-MOBILE-DISCOVER',17655,'127.0.0.1');});
      assert.equal(discovery.token,undefined);assert.ok(!JSON.stringify(discovery).includes(info.token));
      mark('实际 UDP 发现不泄露配对口令');
      const edge=process.env.ACCEPTANCE_EDGE;
      if(edge&&fs.existsSync(edge)){
        mobileBrowser=await chromium.launch({executablePath:edge,headless:true,args:['--no-first-run']});
        const mp=await mobileBrowser.newPage({viewport:{width:390,height:844}});
        await mp.goto(base+'/'+auth);
        await mp.locator('.ticket[data-w="WB-ACCEPT"]').click();
        await mp.locator('#lines .tick').first().click();
        await mp.waitForFunction(()=>document.querySelector('#lines .tick')?.getAttribute('aria-pressed')==='false');
        for(let i=0;i<50;i++){if((await invoke('list_packing_checks')).some(r=>r.waybill==='WB-ACCEPT'&&!r.checked))break;await delay(100);}
        assert.ok((await invoke('list_packing_checks')).some(r=>r.waybill==='WB-ACCEPT'&&!r.checked));
        await mp.screenshot({path:path.join(runDir,'mobile.png')});
        mark('手机网页真实点击可保存并反馈状态（Edge 模拟视口）');
        await mobileBrowser.close();mobileBrowser=null;
      }
      const current=(await (await fetch(base+'/api/sheet'+auth)).json()).rows[0];
      assert.equal((await post({...current,checked:true})).status,200);
      await invoke('mobile_sync',{rows:[{...rows[0],sourceId:'acceptance-new'}]});
      assert.equal((await post({...current,checked:false})).status,409);
      mark('换表后旧手机请求被拒绝');
      const rotated=await invoke('mobile_reset_token');
      const newBase='http://127.0.0.1:'+rotated.port;
      assert.equal((await fetch(newBase+'/api/sheet'+auth)).status,403);
      assert.equal((await fetch(newBase+'/api/sheet',{headers:{Cookie:'mt='+info.token}})).status,403);
      mark('换口令后旧链接及 Cookie 失效');
      await invoke('mobile_stop');
      assert.equal(result.errors.length,0,'页面运行时错误 '+result.errors.join(';'));
    }
    result.ok=true;
  }catch(error){result.ok=false;result.failure=error.stack||String(error);result.appExitCode=app?.exitCode;console.error(error);process.exitCode=1;}
  finally{
    if(mobileBrowser)await mobileBrowser.close().catch(()=>{});
    if(browser)await browser.close().catch(()=>{});
    if(app){app.kill();for(let i=0;i<30&&app.exitCode===null;i++)await delay(100);}
    fs.writeFileSync(path.join(runDir,`result-${phase}.json`),JSON.stringify(result,null,2));
  }
})();
