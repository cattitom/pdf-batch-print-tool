/* 手机单号记忆：云表明细只读；单号独立持久保存，换电脑和分批拆行不丢失。 */
(function () {
  'use strict';
  var CONNECTION='shipping-shell.current', CACHE='shipping-shell.cache', MASTER='shipping-shell.master.v1';
  var busy=Promise.resolve(), offline=true, lastError='', state=null;
  function clone(v){return JSON.parse(JSON.stringify(v));}
  function read(k,f){try{return JSON.parse(localStorage.getItem(k))||f;}catch(_){return f;}}
  function uid(){var bytes=new Uint8Array(16);if(window.crypto&&window.crypto.getRandomValues){window.crypto.getRandomValues(bytes);return Array.from(bytes).map(function(b){return b.toString(16).padStart(2,'0');}).join('');}return Date.now().toString(36)+'-'+Math.random().toString(36).slice(2)+'-'+Math.random().toString(36).slice(2);}
  function key(r){return JSON.stringify([r.waybill||'',r.inbound||'',r.name||'']);}
  function normalize(r){var row=Object.assign({waybill:'',inbound:'',name:'',customer:'',destination:'',arrival:'',remarks:'',qty:0,qtyRaw:String(r.qty||0),cargo:{},issues:[],seq:0,checked:false,manualFields:[]},r);row.syncId=row.syncId||uid();row.knownFields=Array.isArray(row.knownFields)?row.knownFields:Object.keys(r);row.manualFields=Array.isArray(row.manualFields)?row.manualFields:[];row.issues=Array.isArray(row.issues)?row.issues:[];return row;}
  function migrate(){
    var cache=read(CACHE,{rows:[]}), pending=read('shipping-shell.pending',[]);
    var rows=(cache.rows||[]).map(normalize), operations=[];
    pending.forEach(function(op){var matches=rows.filter(function(r){return key(r)===key(op);});if(matches.length===1){matches[0].checked=op.checked;if(matches[0].manualFields.indexOf('checked')<0)matches[0].manualFields.push('checked');operations.push({id:uid(),deviceId:'legacy',recordId:matches[0].syncId,field:'checked',value:op.checked,at:Date.now()});}});
    return {schema:1,storeVersion:0,deviceId:uid(),rows:rows,operations:operations,localRevision:rows.length?1:0,peers:{},conflicts:[],resolutions:{},pendingRequest:null,outboxes:{},lastSaved:cache.at||Date.now(),legacyBackup:{cache:cache,pending:pending}};
  }
  function noteKey(r){return JSON.stringify([String(r.inbound||'').trim(),String(r.name||'').trim(),String(r.destination||'').trim(),r.sourceRow||0]);}
  function remember(next,row,waybill,preserve){
    if(waybill==='亚马逊'||waybill==='发深圳')return;
    next.waybillNotes=next.waybillNotes||[];
    var existing=next.waybillNotes.find(function(n){return n.key===noteKey(row)&&(!preserve||n.waybill===waybill);});
    var note=Object.assign({},existing||{id:uid(),key:noteKey(row)},{waybill:waybill,inbound:row.inbound||'',name:row.name||'',destination:row.destination||'',sourceRow:row.sourceRow||null,updatedAt:Date.now()});
    if(existing)Object.assign(existing,note);else next.waybillNotes.push(note);
  }
  function migrateNotebook(value){
    if(value.authorityMode==='waybill-notebook')return false;
    value.notebookMigrationBackup=clone(value);
    value.waybillNotes=value.waybillNotes||[];
    (value.rows||[]).forEach(function(row){if(row.waybill)remember(value,row,row.waybill);});
    value.operations=(value.operations||[]).filter(function(op){return op.field==='waybill';});
    value.authorityMode='waybill-notebook';value.pendingRequest=null;value.outboxes={};value.conflicts=[];value.resolutions={};value.peers={};
    return true;
  }
  function migrateLedger(value){
    var changed=migrateNotebook(value);
    if(value.syncMode==='shipment-ledger')return changed;
    value.ledgerMigrationBackup=clone(value);
    value.syncMode='shipment-ledger';value.pendingRequest=null;value.outboxes={};value.conflicts=[];value.resolutions={};value.peers={};
    return true;
  }
  state=migrate();
  function database(){return new Promise(function(resolve,reject){if(!window.indexedDB){reject(new Error('手机持久数据库不可用，暂不能保存，请保留现有应用数据'));return;}var open=window.indexedDB.open('shipping-master',1);open.onupgradeneeded=function(){open.result.createObjectStore('state');};open.onerror=function(){reject(open.error);};open.onsuccess=function(){resolve(open.result);};});}
  var adapter=window.shippingMasterStorage||{
    load:async function(){var db=await database();try{return await new Promise(function(resolve,reject){var tx=db.transaction('state','readonly'),req=tx.objectStore('state').get(MASTER);req.onsuccess=function(){resolve(req.result||null);};req.onerror=function(){reject(req.error);};});}finally{db.close();}},
    save:async function(value,expected){var db=await database();try{await new Promise(function(resolve,reject){var tx=db.transaction('state','readwrite');var store=tx.objectStore('state'),existing=store.get(MASTER);var conflict=null;existing.onsuccess=function(){if((existing.result&&existing.result.storeVersion||0)!==(expected||0)){conflict=new Error('主库已在另一个页面更新，请重新打开清单');tx.abort();return;}store.put(value,MASTER);};tx.oncomplete=function(){resolve();};tx.onerror=function(){reject(tx.error);};tx.onabort=function(){reject(conflict||tx.error||new Error('手机保存事务被取消'));};});}finally{db.close();}}
  };
  function mirror(){try{localStorage.setItem(CACHE,JSON.stringify({at:state.lastSaved,rows:state.rows,master:true}));}catch(_){} }
  var ready=adapter.load().then(async function(saved){if(saved){if(saved.schema!==1||!Array.isArray(saved.rows)||!Array.isArray(saved.operations)||!saved.deviceId)throw new Error('手机主库版本不兼容，请保留数据并升级应用');state=saved;state.outboxes=state.outboxes||{};var expected=state.storeVersion||0;if(migrateLedger(state)){state.storeVersion=expected+1;await adapter.save(clone(state),expected);}}else{migrateLedger(state);state.storeVersion=1;await adapter.save(clone(state),0);}mirror();}).catch(function(e){lastError=e.message||'手机主库读取失败';throw e;});
  ready.catch(function(){});
  async function save(next){next.lastSaved=Date.now();next.storeVersion=(state.storeVersion||0)+1;try{await adapter.save(clone(next),state.storeVersion||0);}catch(error){if(String(error.message).indexOf('另一个页面')>=0){var latest=await adapter.load();if(latest){state=latest;mirror();}}throw error;}state=next;mirror();}
  function serialize(fn){var next=busy.catch(function(){}).then(async function(){await ready;return fn();});busy=next;return next;}
  function endpoint(){var entry=read(CONNECTION,null);if(!entry||!entry.url)throw new Error('未配对电脑，主库可离线使用；请在设置中扫码配对');var u=new URL(entry.url);if(!/^https?:$/.test(u.protocol)||!/^[a-f0-9]{64}$/i.test(u.searchParams.get('t')||''))throw new Error('配对信息无效，请重新扫码');return u;}
  function origin(){try{return endpoint().origin;}catch(_){return '';}}
  async function request(path,options,pairedUrl){if(['/api/sheet','/api/check','/api/claim','/api/refresh','/api/master-sync'].indexOf(path)<0)throw new Error('不支持的接口');var u=pairedUrl?new URL(pairedUrl):endpoint(),controller=new AbortController(),timer=setTimeout(function(){controller.abort();},10000);try{return await fetch(u.origin+path+'?t='+encodeURIComponent(u.searchParams.get('t')),Object.assign({},options,{signal:controller.signal,cache:'no-store',credentials:'omit'}));}finally{clearTimeout(timer);}}
  async function exchange(){
    var pairedUrl=endpoint().href,computer=new URL(pairedUrl).origin;if(!computer)throw new Error('尚未配对电脑，手机主库已保留');
    if(state.pendingRequest&&state.pendingRequest.computer!==computer){var switched=clone(state);switched.outboxes[switched.pendingRequest.computer]=switched.pendingRequest;switched.pendingRequest=switched.outboxes[computer]||null;delete switched.outboxes[computer];await save(switched);}
    if(!state.pendingRequest){var draft=clone(state),peer=draft.peers[computer]||{};draft.pendingRequest={computer:computer,revision:draft.localRevision,opEnd:Math.min(draft.operations.length,(peer.opCursor||0)+1000),body:{mode:'shipment-ledger',waybillNotes:draft.waybillNotes||[],deviceId:draft.deviceId,requestId:uid(),rows:draft.rows,operations:draft.operations.slice(peer.opCursor||0,(peer.opCursor||0)+1000),resolutions:draft.resolutions}};await save(draft);}
    var sent=clone(state.pendingRequest), res=await request('/api/master-sync',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(sent.body)},pairedUrl);
    var result=await res.json().catch(function(){return {};});
    if(!res.ok){if(res.status===404)throw new Error('当前电脑未支持手机主库，请先更新电脑程序');throw new Error(result.error||(res.status===403?'配对已失效，请重新扫码':'同步失败'));}
    if(result.protocol!==1||result.mode!=='shipment-ledger'||!Array.isArray(result.rows)||!result.computerId)throw new Error('电脑返回的同步数据不完整，手机原数据已保留');
    var next=clone(state),before=JSON.stringify(next.rows.map(business));
    next.rows=result.rows.map(normalize);
    next.rows.forEach(function(row){if(row.waybill)remember(next,row,row.waybill,true);});
    // 台账中的已出票据独立保留；云表清空单号不删除本地历史。
    // 与电脑端 equivalent 对齐：目的地一侧为空仍算同一行，否则云表补填目的地会把同一票重复存档。
    state.rows.forEach(function(old){
      if(!old.checked||!old.waybill||next.rows.some(function(r){var ri=String(r.inbound||'').trim(),rn=String(r.name||'').trim(),rd=String(r.destination||'').trim(),oi=String(old.inbound||'').trim(),on=String(old.name||'').trim(),od=String(old.destination||'').trim();return r.syncId===old.syncId||ri===oi&&rn===on&&(ri||rn)&&(rd===od||!rd||!od);}))return;
      var archived=clone(old);if(archived.issues.indexOf('history_archive')<0)archived.issues.push('history_archive');next.rows.push(archived);
    });
    state.operations.filter(function(op){return (op.revision||0)>sent.revision;}).forEach(function(op){var row=next.rows.find(function(r){return r.syncId===op.recordId;});if(row&&(op.field==='waybill'||op.field==='checked')){row[op.field]=clone(op.value);if(row.manualFields.indexOf(op.field)<0)row.manualFields.push(op.field);}});
    next.conflicts=result.conflicts||[];next.resolutions={};
    if(before!==JSON.stringify(next.rows.map(business)))next.localRevision+=1;
    var known={};next.operations.forEach(function(op){known[op.id]=true;});
    (result.operations||[]).forEach(function(op){if(!known[op.id]){next.localRevision+=1;next.operations.push(Object.assign({},op,{revision:next.localRevision,originComputer:result.computerId}));known[op.id]=true;}});
    var previous=next.peers[computer];
    next.peers[computer]={computerId:result.computerId,lastSync:Date.now(),ackRevision:state.localRevision>sent.revision?sent.revision:next.localRevision,opCursor:previous&&previous.computerId!==result.computerId?0:sent.opEnd};
    next.pendingRequest=null;await save(next);offline=false;lastError='';
    return {rows:clone(state.rows),offline:false,conflicts:clone(state.conflicts)};
  }
  function business(r){return [r.syncId,r.waybill,r.inbound,r.name,r.customer,r.destination,r.qty,r.checked,r.arrival,r.remarks,r.issues,r.cargo];}
  function pendingCount(){var peer=state.peers[origin()];if(!peer)return state.rows.length;var n=state.operations.slice(peer.opCursor||0).filter(function(op){return op.originComputer!==peer.computerId;}).length;return n||((peer.ackRevision||0)<state.localRevision?1:0);}
  function status(){if(lastError)return '出货台账已保留 · '+lastError;if(state.conflicts.length)return '有 '+state.conflicts.length+' 项单号或出货状态冲突待核对';var n=pendingCount();if(n)return '单号已保存在手机 · 待同步';var p=state.peers[origin()];return offline?'云表离线快照 · 可记单号':p?'云表已更新 · '+new Date(p.lastSync).toLocaleTimeString():'手机单号记忆';}
  function connectionError(e){return e.name==='AbortError'?'电脑连接超时，请确认电脑程序已打开并连接同一网络':e.name==='TypeError'?'无法连接电脑，请确认程序已打开、网络可达，必要时重新扫码':e.message||'电脑连接失败';}
  function sheet(){return serialize(async function(){try{return await exchange();}catch(e){offline=true;lastError=connectionError(e);if(!state.rows.length)throw new Error(lastError);return {rows:clone(state.rows),offline:true,cachedAt:state.lastSaved};}});}
  function find(row){var matches=state.rows.filter(function(r){return row.syncId?r.syncId===row.syncId:key(r)===key(row);});if(matches.length!==1)throw new Error('记录不唯一或已改变，请重新打开明细');return matches[0];}
  function canShip(row){return row.issues.some(function(i){return i==='special_amazon'||i==='special_shenzhen';})||String(row.inbound).trim()&&String(row.waybill).trim();}
  function edit(row,fields){return serialize(async function(){
    var original=find(row),next=clone(state),target=next.rows.find(function(r){return r.syncId===original.syncId;});
    var allowed=['waybill','checked'];Object.keys(fields).forEach(function(field){if(allowed.indexOf(field)<0)throw new Error('手机仅修改单号和出货状态，货物资料以云表为准');target[field]=fields[field];});
    if(target.checked&&!canShip(target))throw new Error('入库号和单号必须都填写，才能出货；特殊路线除外');
    if(Object.prototype.hasOwnProperty.call(fields,'waybill'))remember(next,original,String(target.waybill||'').trim());
    Object.keys(fields).forEach(function(field){if(JSON.stringify(original[field])!==JSON.stringify(target[field])){if(target.manualFields.indexOf(field)<0)target.manualFields.push(field);next.localRevision+=1;next.operations.push({id:uid(),deviceId:next.deviceId,recordId:target.syncId,field:field,previous:original[field],value:target[field],revision:next.localRevision,at:Date.now()});}});
    await save(next);
    try{await exchange();}catch(e){offline=true;lastError=e.message||'连接失败';}
    return {offline:offline,local:true};
  });}
  function check(row,checked){return edit(row,{checked:checked});}
  function resolveConflict(recordId,field,choice){return serialize(async function(){if(state.pendingRequest)await exchange();var conflict=state.conflicts.find(function(c){return c.recordId===recordId&&c.field===field;});if(!conflict)return;var next=clone(state);next.resolutions[recordId+':'+field]={choice:choice,phone:conflict.phone,computer:conflict.computer};await save(next);return exchange();});}
  function noteCandidates(note,allowFallback){
    var found=state.rows.filter(function(r){return String(r.inbound||'').trim()===note.inbound.trim()&&String(r.name||'').trim()===note.name.trim()&&(!note.destination||r.destination===note.destination);});
    if(note.targetIdentity&&!allowFallback)return found.filter(function(r){return JSON.stringify([r.inbound.trim(),r.name.trim(),String(r.destination||'').trim(),r.sourceRow||r.seq||0])===note.targetIdentity;});
    if(note.sourceRow){var exact=found.filter(function(r){return r.sourceRow===note.sourceRow;});if(exact.length)found=exact;}
    return found;
  }
  function syncInfo(){var current=origin(),peer=state.peers[current];return {otherPending:Object.keys(state.outboxes||{}).length,computer:current||'未连接',lastSync:peer&&peer.lastSync,pending:pendingCount(),conflicts:clone(state.conflicts),deviceId:state.deviceId,
    waybillNotes:clone((state.waybillNotes||[]).filter(function(n){return !!n.waybill;})).map(function(note){var candidates=noteCandidates(note);return Object.assign(note,{candidates:clone(candidates.length?candidates:noteCandidates(note,true)),needsMatch:candidates.length!==1,cloudWaybill:candidates.length===1?candidates[0].waybill:''});})};}
  function linkWaybill(noteId,rowId){return serialize(async function(){var next=clone(state),note=next.waybillNotes.find(function(n){return n.id===noteId;}),row=next.rows.find(function(r){return r.syncId===rowId;});if(!note||!row)throw new Error('明细已变化，请刷新后核对');
    if(row.inbound.trim()!==note.inbound.trim()||row.name.trim()!==note.name.trim())throw new Error('请选择该单号对应的同一货物');
    note.targetIdentity=JSON.stringify([row.inbound.trim(),row.name.trim(),String(row.destination||'').trim(),row.sourceRow||row.seq||0]);note.sourceRow=row.sourceRow||null;
    next.localRevision++;next.pendingRequest=null;await save(next);return exchange();});}
  window.shippingTransport={waybillOnly:false,ledgerOnly:true,linkWaybill:linkWaybill,request:request,sheet:sheet,check:check,edit:edit,cached:function(){return clone(state.rows);},status:status,ready:ready,syncInfo:syncInfo,resolveConflict:resolveConflict,prepareRefresh:function(){return serialize(exchange);},exportInfo:function(){return {source:'本地出货台账',cachedAt:state.lastSaved,offline:offline,pending:pendingCount()};}};
})();
