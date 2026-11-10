//! 双向三方合并：每台电脑保留与手机上次共同确认的基线，字段冲突交给用户。
use crate::mobile::{MobileRow, MobileShared};
use pdfimpose_core::db::{Db, LedgerState};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::collections::{BTreeMap, BTreeSet};
#[path="shipment_ledger.rs"]
mod shipment_ledger;
const FIELDS:&[&str]=&["checked","waybill","customer","name","inbound","destination","qty","arrival","remarks","cargo","issues"];

#[derive(Clone,Default,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct Conflict {pub record_id:String,pub field:String,pub phone:Value,pub computer:Value,pub name:String,
 #[serde(default)] pub inbound:String,
 #[serde(default,skip_serializing_if="Option::is_none")] pub source_row:Option<u32>}

#[derive(Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct Resolution {pub choice:String,pub phone:Value,pub computer:Value}
#[derive(Clone,Default,Serialize,Deserialize)]
#[serde(rename_all="camelCase",default)]
pub struct Request {pub mode:String,pub waybill_notes:Vec<WaybillNote>,pub device_id:String,pub request_id:String,pub rows:Vec<MobileRow>,pub operations:Vec<Value>,pub resolutions:BTreeMap<String,Resolution>}
#[derive(Clone,Default,Serialize,Deserialize)]
#[serde(rename_all="camelCase",default)]
pub struct Response {pub mode:String,pub protocol:u32,pub computer_id:String,pub revision:u64,pub rows:Vec<MobileRow>,pub conflicts:Vec<Conflict>,pub operations:Vec<Value>}
#[derive(Clone,Default,Serialize,Deserialize)]
#[serde(rename_all="camelCase",default)]
struct Peer {base:Vec<MobileRow>,conflicts:Vec<Conflict>,request_id:String,response:Response}
#[derive(Default,Serialize,Deserialize)]
#[serde(rename_all="camelCase",default)]
struct State {mode:String,computer_id:String,revision:u64,records:Vec<MobileRow>,peers:BTreeMap<String,Peer>,operations:Vec<Value>,updated_at:String}

fn uid()->String {crate::mobile::new_token().expect("系统随机数不可用")}
pub fn identity(r:&MobileRow)->String {
    json!([r.inbound.trim(),r.name.trim(),r.destination.trim(),r.seq]).to_string()
}
fn equivalent(a:&MobileRow,b:&MobileRow)->bool {
    a.inbound.trim()==b.inbound.trim()&&a.name.trim()==b.name.trim()&&(!a.name.trim().is_empty()||!a.inbound.trim().is_empty())&&
    (a.destination.trim()==b.destination.trim()||a.destination.trim().is_empty()||b.destination.trim().is_empty())
}
// 空入库号不是货物标识。同名行需要再核对数量、客户和原表行号；只接受双向唯一候选。
fn candidates<'a>(rows:&'a [MobileRow],row:&MobileRow)->Vec<&'a MobileRow>{
    let mut matches:Vec<_>=rows.iter().filter(|r|equivalent(r,row)).collect();
    if matches.len()>1 {
        if let Some(source_row)=row.source_row {
            let by_source:Vec<_>=matches.iter().copied().filter(|r|r.source_row==Some(source_row)).collect();
            if !by_source.is_empty(){matches=by_source;}
        }
        let by_qty:Vec<_>=matches.iter().copied().filter(|r|r.qty==row.qty).collect();
        if !by_qty.is_empty(){matches=by_qty;}
        if matches.len()>1&&!row.customer.trim().is_empty(){let by_customer:Vec<_>=matches.iter().copied().filter(|r|r.customer.trim()==row.customer.trim()).collect();if !by_customer.is_empty(){matches=by_customer;}}
        if matches.len()>1&&row.seq>0 {let by_seq:Vec<_>=matches.iter().copied().filter(|r|r.seq==row.seq).collect();if !by_seq.is_empty(){matches=by_seq;}}
    }
    matches
}
fn business(row:&MobileRow)->Value {
    let mut value=serde_json::to_value(row).unwrap();
    value["issues"]=json!(row.issues.iter().filter(|s|s.as_str()=="special_amazon"||s.as_str()=="special_shenzhen").collect::<Vec<_>>());
    value
}
fn add_manual(row:&mut MobileRow,field:&str) {if !row.manual_fields.iter().any(|s|s==field) {row.manual_fields.push(field.into());}}
/// 从正规化表读出台账状态：台账行、每台设备的合并基线和冲突分开存。
fn load_state(db:&Db)->Result<State,String>{
    let ledger=db.ledger_state().map_err(|e|e.to_string())?;
    let mut state=State{mode:ledger.mode,computer_id:ledger.computer_id,revision:ledger.revision.max(0) as u64,
        updated_at:ledger.updated_at,operations:serde_json::from_str(&ledger.operations).unwrap_or_default(),
        records:Vec::new(),peers:BTreeMap::new()};
    for row in ledger.rows {if let Ok(row)=serde_json::from_value::<MobileRow>(row){state.records.push(row);}}
    for (device,peer) in ledger.peers {if let Ok(peer)=serde_json::from_value::<Peer>(peer){state.peers.insert(device,peer);}}
    Ok(state)
}
/// 整份台账和本次设备的基线/冲突在一个事务里落库；唯一约束在数据库层兜底。
fn persist(db:&Db,state:&State,device:&str,backup:&Value)->Result<(),String>{
    let ledger=LedgerState{mode:state.mode.clone(),computer_id:state.computer_id.clone(),
        revision:state.revision as i64,updated_at:state.updated_at.clone(),
        operations:serde_json::to_string(&state.operations).unwrap_or_else(|_|"[]".into()),
        rows:state.records.iter().map(|r|serde_json::to_value(r).unwrap()).collect(),
        peers:state.peers.iter().map(|(k,v)|(k.clone(),serde_json::to_value(v).unwrap())).collect()};
    db.save_ledger(&ledger,Some(device),backup).map_err(|e|e.to_string())
}

/// 手机只携带单号记忆；数量、商品、出货状态等始终取电脑当前数据集。
#[derive(Clone,Default,Serialize,Deserialize)]
#[serde(rename_all="camelCase",default)]
pub struct WaybillNote {
 pub id:String,pub waybill:String,pub inbound:String,pub name:String,pub destination:String,
 pub source_row:Option<u32>,pub target_identity:String,
}
fn notebook_identity(r:&MobileRow)->String {
 json!([r.inbound.trim(),r.name.trim(),r.destination.trim(),r.source_row.unwrap_or(r.seq)]).to_string()
}
fn note_candidates(rows:&[MobileRow],note:&WaybillNote)->Vec<usize> {
 let mut found:Vec<_>=rows.iter().enumerate().filter(|(_,r)|
  !note.name.trim().is_empty()&&r.name.trim()==note.name.trim()&&r.inbound.trim()==note.inbound.trim()&&
  (note.destination.trim().is_empty()||r.destination.trim()==note.destination.trim())).map(|(i,_)|i).collect();
 if !note.target_identity.is_empty(){found.retain(|&i|notebook_identity(&rows[i])==note.target_identity);}
 else if let Some(source)=note.source_row {
  let exact:Vec<_>=found.iter().copied().filter(|&i|rows[i].source_row==Some(source)).collect();
  if !exact.is_empty(){found=exact;}
 }
 found
}
fn exchange_notebook(shared:&MobileShared,request:Request)->Result<Response,(u16,String)> {
 if request.device_id.is_empty()||request.device_id.len()>100||request.request_id.is_empty()||request.request_id.len()>150||request.waybill_notes.len()>20000 {
  return Err((400,"单号同步数据不合法或超出范围".into()));
 }
 let mut inner=shared.inner.lock().map_err(|_|(500,"手机状态锁异常".into()))?;
 if inner.rows.is_empty(){return Err((409,"电脑尚未载入云表格，请先在电脑打开出货清单；手机单号已保留".into()));}
 let db=shared.db.lock().map_err(|_|(500,"数据库锁异常".into()))?;
 let mut state=load_state(&db).map_err(|e|(500,e))?;
 if state.mode=="shipment-ledger"{return Err((409,"请加载新版出货台账页面；旧版单号快照不能覆盖完整台账".into()));}
 if let Some(peer)=state.peers.get(&request.device_id){if peer.request_id==request.request_id&&peer.response.mode=="waybill-notebook"{return Ok(peer.response.clone());}}
 if state.computer_id.is_empty(){state.computer_id=uid();}
 let mut rows=inner.rows.clone();
 let mut numbers:BTreeMap<usize,BTreeSet<String>>=BTreeMap::new();
 for note in &request.waybill_notes {
  if note.id.len()>150||note.waybill.len()>100||note.target_identity.len()>2000{return Err((400,"单号记忆内容超出范围".into()));}
  if note.waybill.trim().is_empty(){continue;}
  let found=note_candidates(&rows,note);
  if let [index]=found.as_slice(){numbers.entry(*index).or_default().insert(note.waybill.trim().into());}
 }
 for (i,row) in rows.iter_mut().enumerate(){
  if row.sync_id.is_empty(){row.sync_id=uid();}
  row.source_identity=identity(row);
  // 不将手机旧数量、客户或出货勾选写回电脑，只允许补充唯一对应的缺失单号。
  row.manual_fields.retain(|f|f=="waybill");
  if row.waybill.trim().is_empty(){if let Some(values)=numbers.get(&i){if values.len()==1{
   row.waybill=values.first().unwrap().clone();add_manual(row,"waybill");
  }}}
  row.known_fields=FIELDS.iter().map(|s|(*s).into()).collect();
  row.blocked=!row.issues.iter().any(|i|i=="special_amazon"||i=="special_shenzhen")&&(row.inbound.trim().is_empty()||row.waybill.trim().is_empty());
 }
 let changed=rows.len()!=inner.rows.len()||rows.iter().zip(&inner.rows).any(|(a,b)|a.sync_id!=b.sync_id||FIELDS.iter().any(|f|business(a)[*f]!=business(b)[*f]));
 if changed{state.revision+=1;}
 for row in &mut rows {row.master_revision=state.revision;row.record_id=json!([row.waybill.trim(),row.inbound.trim(),row.name.trim()]).to_string();}
 let response=Response{mode:"waybill-notebook".into(),protocol:1,computer_id:state.computer_id.clone(),revision:state.revision,rows:rows.clone(),conflicts:vec![],operations:vec![]};
 state.mode="waybill-notebook".into();state.records=rows.clone();state.updated_at=now_ms().to_string();
 state.peers.insert(request.device_id.clone(),Peer{base:rows.clone(),conflicts:vec![],request_id:request.request_id.clone(),response:response.clone()});
 persist(&db,&state,&request.device_id,&json!({"mode":"waybill-notebook","request":request,"at":now_ms()})).map_err(|e|(500,format!("单号同步保存失败：{e}")))?;
 inner.rows=rows;
 Ok(response)
}

pub fn exchange(shared:&MobileShared,request:Request)->Result<Response,(u16,String)> {
    if request.mode=="shipment-ledger" {return shipment_ledger::exchange_ledger(shared,request);}
    if request.mode=="waybill-notebook" {return exchange_notebook(shared,request);}
    if request.device_id.is_empty()||request.device_id.len()>100||request.request_id.is_empty()||request.request_id.len()>150||request.rows.len()>20000||request.operations.len()>50000 {return Err((400,"手机同步数据不合法或超出范围".into()));}
    let mut inner=shared.inner.lock().map_err(|_|(500,"手机状态锁异常".into()))?;
    let db=shared.db.lock().map_err(|_|(500,"数据库锁异常".into()))?;
    let mut state=load_state(&db).map_err(|e|(500,e))?;
    if matches!(state.mode.as_str(),"waybill-notebook"|"shipment-ledger") {return Err((409,"手机同步规则已改为单号记忆，请重新加载手机页面后再连接；旧主库不会覆盖云表".into()));}
    if let Some(peer)=state.peers.get(&request.device_id) {
        if peer.request_id==request.request_id {return Ok(peer.response.clone());}
    }
    if state.computer_id.is_empty() {state.computer_id=uid();}
    let mut incoming=request.rows.clone();
    let mut seen=BTreeSet::new();
    for row in &mut incoming {
        if row.sync_id.is_empty(){row.sync_id=uid();}
        if row.sync_id.len()>100||!seen.insert(row.sync_id.clone())||row.qty<0||row.qty>1_000_000_000 {return Err((400,"记录ID重复或数量不合法".into()));}
    }
    let mut current=inner.rows.clone();
    // 电脑重新打开原表后，从上次持久快照恢复记录ID；不以可变单号或客户名认行。
    for (row_index,row) in current.iter_mut().enumerate() {
        if row.sync_id.is_empty() {
            let candidates:Vec<_>=state.records.iter().filter(|r|identity(r)==identity(row)).collect();
            if candidates.len()==1 {row.sync_id=candidates[0].sync_id.clone();}
        }
        if !incoming.iter().any(|p|p.sync_id==row.sync_id) {
            let matches=candidates(&incoming,row);
            let reverse=if matches.len()==1{candidates(&inner.rows,matches[0])}else{vec![]};
            if matches.len()>1 || matches.len()==1&&reverse.len()!=1 {
                let describe=|r:&MobileRow|format!("{}{} 箱（{}）",
                    r.source_row.map(|n|format!("原表第 {n} 行 · ")).unwrap_or_else(||if r.seq>0{format!("明细第 {} 条 · ",r.seq)}else{String::new()}),
                    r.qty,if r.checked{"已出"}else{"未出"});
                let phone=incoming.iter().filter(|r|equivalent(r,row)).map(describe).collect::<Vec<_>>().join("；");
                let computer=inner.rows.iter().filter(|r|equivalent(r,row)).map(describe).collect::<Vec<_>>().join("；");
                return Err((409,format!("{} / {}：{}。手机：{}。电脑：{}。可能是原表分批拆行，不能自动对应；本次未覆盖任一端数据",row.name,
                    if row.inbound.trim().is_empty(){"未填入库号"}else{row.inbound.trim()},
                    if row.inbound.trim().is_empty(){"缺少入库号且有同名记录"}else{"有多条相似记录"},phone,computer)));
            }
            // 手机旧版少了一行时，只绑定真正对应的原行，新增同款行另建ID。
            if matches.len()==1 && std::ptr::eq(reverse[0],&inner.rows[row_index]) {row.sync_id=matches[0].sync_id.clone();}
        }
        if row.sync_id.is_empty(){row.sync_id=uid();}
    }
    // 原表删行不等于删除历史记录。
    for row in &state.records {
        if !current.iter().any(|r|r.sync_id==row.sync_id||identity(r)==identity(row)) {current.push(row.clone());}
    }
    let old_peer=state.peers.get(&request.device_id).cloned().unwrap_or_default();
    let base:BTreeMap<_,_>=old_peer.base.iter().map(|r|(r.sync_id.clone(),business(r))).collect();
    let mut desktop:BTreeMap<_,_>=current.into_iter().map(|r|(r.sync_id.clone(),r)).collect();
    let mut phone:BTreeMap<_,_>=incoming.into_iter().map(|r|(r.sync_id.clone(),r)).collect();
    let ids:BTreeSet<_>=desktop.keys().chain(phone.keys()).cloned().collect();
    let mut merged=Vec::new();let mut phone_view=Vec::new();let mut conflicts=Vec::new();let mut events=Vec::new();
    for id in ids {
        let Some(p)=phone.remove(&id) else {let row=desktop.remove(&id).unwrap();phone_view.push(row.clone());merged.push(row);continue;};
        let Some(c)=desktop.remove(&id) else {phone_view.push(p.clone());let mut imported=p;if !inner.rows.is_empty()&&!imported.issues.iter().any(|s|s=="history_archive"){imported.issues.push("history_archive".into());}merged.push(imported);continue;};
        let pv=business(&p);let cv=business(&c);let bv=base.get(&id);
        let mut output=serde_json::to_value(&c).unwrap();
        if c.source_identity.is_empty() {output["sourceIdentity"]=json!(identity(&c));}
        let mut phone_output=output.clone();
        let mut manual=c.manual_fields.clone();for f in &p.manual_fields {if !manual.contains(f){manual.push(f.clone());}}
        for field in FIELDS {
            let key=format!("{id}:{field}");let a=&pv[*field];let b=&cv[*field];
            let pending=old_peer.conflicts.iter().any(|x|x.record_id==id&&x.field==*field);
            let choice=request.resolutions.get(&key).filter(|r|r.phone==*a&&r.computer==*b);
            let selected=if !p.known_fields.iter().any(|f|f==field)&&!p.manual_fields.iter().any(|f|f==field) {Some(b)}
                else if a==b {Some(a)}
                else if let Some(r)=choice {match r.choice.as_str(){"phone"=>Some(a),"computer"=>Some(b),_=>None}}
                else if pending {None}
                else if let Some(base)=bv {if a==&base[*field] {Some(b)} else if b==&base[*field] {Some(a)} else {None}}
                else {None};
            if let Some(value)=selected {
                output[*field]=value.clone();phone_output[*field]=value.clone();
                if *field=="issues" {
                    let mut all:Vec<Value>=c.issues.iter().filter(|s|s.as_str()!="special_amazon"&&s.as_str()!="special_shenzhen").map(|s|json!(s)).collect();
                    all.extend(value.as_array().into_iter().flatten().cloned());output[*field]=json!(all);phone_output[*field]=output[*field].clone();
                }
                if a!=b || choice.is_some() {
                    if choice.is_some()&&!manual.iter().any(|f|f==field){manual.push((*field).into());}
                    events.push(json!({"id":uid(),"deviceId":state.computer_id,"recordId":id,"field":field,"value":value,"at":now_ms()}));
                }
            } else {
                conflicts.push(Conflict{record_id:id.clone(),field:(*field).into(),phone:a.clone(),computer:b.clone(),name:p.name.clone(),inbound:c.inbound.clone(),source_row:c.source_row});
                phone_output[*field]=a.clone(); // 冲突未解决时，手机和电脑各自保留原值。
            }
        }
        output["manualFields"]=json!(manual);phone_output["manualFields"]=output["manualFields"].clone();
        let mut result:MobileRow=serde_json::from_value(output).map_err(|e|(400,e.to_string()))?;
        let phone_result:MobileRow=serde_json::from_value(phone_output).map_err(|e|(400,e.to_string()))?;
        // 仅新的出货操作受缺号限制，原有历史已出记录仍允许保留和撤回。
        if result.checked&&!c.checked&&(result.inbound.trim().is_empty()||result.waybill.trim().is_empty())&& !result.issues.iter().any(|i|i=="special_amazon"||i=="special_shenzhen") {return Err((400,format!("{} 缺少入库号或单号，不能标已出",result.name)));}
        if result.checked!=c.checked {add_manual(&mut result,"checked");}
        merged.push(result);phone_view.push(phone_result);
    }
    let changed=merged.len()!=inner.rows.len()||merged.iter().any(|r|!inner.rows.iter().any(|old|old.sync_id==r.sync_id&&FIELDS.iter().all(|f|business(old)[*f]==business(r)[*f])&&old.manual_fields==r.manual_fields));
    if changed {state.revision+=1;}
    let source=inner.rows.first().map(|r|r.source_id.clone()).unwrap_or_default();
    for row in &mut merged {
        row.known_fields=FIELDS.iter().map(|s|(*s).into()).collect();
        row.master_revision=state.revision;row.source_id=source.clone();row.record_id=json!([row.waybill.trim(),row.inbound.trim(),row.name.trim()]).to_string();
        row.dataset_id=format!("master-{}",state.revision);row.version=0;
        row.blocked=!row.issues.iter().any(|i|i=="special_amazon"||i=="special_shenzhen")&&(row.waybill.trim().is_empty()||row.inbound.trim().is_empty());
    }
    for row in &mut phone_view {row.known_fields=FIELDS.iter().map(|s|(*s).into()).collect();row.master_revision=state.revision;row.blocked=!row.issues.iter().any(|i|i=="special_amazon"||i=="special_shenzhen")&&(row.waybill.trim().is_empty()||row.inbound.trim().is_empty());}
    let order=|r:&MobileRow|if r.seq==0{u32::MAX}else{r.seq};merged.sort_by_key(order);phone_view.sort_by_key(order);
    let response=Response{mode:String::new(),protocol:1,computer_id:state.computer_id.clone(),revision:state.revision,rows:phone_view,conflicts:conflicts.clone(),operations:events.clone()};
    state.records=merged.clone();state.updated_at=now_ms().to_string();
    let mut known:BTreeSet<_>=state.operations.iter().filter_map(|x|x["id"].as_str().map(str::to_string)).collect();
    for event in request.operations.iter().chain(events.iter()) {if let Some(id)=event["id"].as_str(){if known.insert(id.into()){state.operations.push(event.clone());}}}
    state.peers.insert(request.device_id.clone(),Peer{base:merged.clone(),conflicts,request_id:request.request_id.clone(),response:response.clone()});
    persist(&db,&state,&request.device_id,&json!({"deviceId":request.device_id,"requestId":request.request_id,"rows":request.rows,"operations":request.operations,"at":now_ms()})).map_err(|e|(500,format!("同步保存失败，未覆盖原数据：{e}")))?;
    inner.rows=merged;
    Ok(response)
}
fn now_ms()->u128 {std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()}

pub fn edit_local(shared:&MobileShared,before:MobileRow,fields:BTreeMap<String,Value>)->Result<MobileRow,(u16,String)> {
    let allowed=["waybill","customer","name","inbound","destination","qty","qtyRaw","arrival","remarks","issues","cargo"];
    if fields.keys().any(|k|!allowed.contains(&k.as_str())) {return Err((400,"此字段不能通过明细编辑修改".into()));}
    let mut inner=shared.inner.lock().map_err(|_|(500,"手机状态锁异常".into()))?;
    let candidates:Vec<_>=inner.rows.iter().enumerate().filter(|(_,r)|if before.sync_id.is_empty(){equivalent(r,&before)}else{r.sync_id==before.sync_id}).map(|(i,_)|i).collect();
    if candidates.len()!=1{return Err((409,"记录不唯一或表格已变化，请刷新核对".into()));}
    let index=candidates[0];let current=inner.rows[index].clone();let old=serde_json::to_value(&before).unwrap();let mut value=serde_json::to_value(&current).unwrap();
    for (field,next) in &fields {if value[field]!=old[field]&&value[field]!=*next{return Err((409,format!("{field} 已被手机或其他操作修改，请刷新核对")));}value[field]=next.clone();}
    let mut row:MobileRow=serde_json::from_value(value).map_err(|e|(400,e.to_string()))?;
    if row.sync_id.is_empty(){row.sync_id=uid();}if row.source_identity.is_empty(){row.source_identity=identity(&before);}
    for field in fields.keys(){add_manual(&mut row,field);}
    let db=shared.db.lock().map_err(|_|(500,"数据库锁异常".into()))?;
    let mut state=load_state(&db).map_err(|e|(500,e))?;
    if state.computer_id.is_empty(){state.computer_id=uid();}
    state.records.retain(|r|r.sync_id!=row.sync_id&&identity(r)!=identity(&before));state.records.push(row.clone());
    for(field,next)in fields{state.operations.push(json!({"id":uid(),"deviceId":state.computer_id,"recordId":row.sync_id,"field":field,"value":next,"at":now_ms()}));}
    let raw=serde_json::to_value(&row).unwrap();
    persist(&db,&state,"desktop-edit",&raw).map_err(|e|(500,e))?;
    inner.rows[index]=row.clone();Ok(row)
}

#[cfg(test)]
mod tests {
 use super::*;
 fn fixture()->MobileShared {let s=crate::mobile::new_shared();s.inner.lock().unwrap().rows=vec![MobileRow{waybill:"W".into(),inbound:"I".into(),name:"商品".into(),customer:"客户".into(),destination:"仓库".into(),qty:7,known_fields:FIELDS.iter().map(|s|(*s).into()).collect(),..Default::default()}];s}
 fn req(id:&str,rows:Vec<MobileRow>)->Request {Request{device_id:"phone".into(),request_id:id.into(),rows,..Default::default()}}
 #[test]
 fn carries_phone_changes_between_two_computers_and_detects_initial_conflict(){
  let company=fixture();let home=fixture();let initial=exchange(&company,req("a",vec![])).unwrap();
  let mut phone=initial.rows;phone[0].checked=true;phone[0].manual_fields.push("checked".into());
  let sent=exchange(&company,req("b",phone)).unwrap();assert!(sent.rows[0].checked);assert!(sent.conflicts.is_empty());
  let conflict=exchange(&home,req("c",sent.rows)).unwrap();assert_eq!(conflict.conflicts.len(),1);assert_eq!(conflict.conflicts[0].field,"checked");
  assert!(!home.inner.lock().unwrap().rows[0].checked,"未解决冲突不能覆盖电脑");
  let f=&conflict.conflicts[0];let mut resolution=req("d",conflict.rows.clone());resolution.resolutions.insert(format!("{}:{}",f.record_id,f.field),Resolution{choice:"phone".into(),phone:f.phone.clone(),computer:f.computer.clone()});
  let synced=exchange(&home,resolution).unwrap();assert!(synced.conflicts.is_empty());assert!(home.inner.lock().unwrap().rows[0].checked);
  home.inner.lock().unwrap().rows[0].customer="家里修订".into();
  let back=exchange(&home,req("e",synced.rows)).unwrap();assert_eq!(back.rows[0].customer,"家里修订");
  let final_rows=exchange(&company,req("f",back.rows)).unwrap();assert_eq!(final_rows.rows[0].customer,"家里修订");assert!(final_rows.rows[0].checked);
 }
 #[test]
 fn retry_is_idempotent_and_no_change_does_not_advance_data_revision(){
  let s=fixture();let a=exchange(&s,req("one",vec![])).unwrap();let b=exchange(&s,req("two",a.rows.clone())).unwrap();assert_eq!(a.revision,b.revision);
  let retry=exchange(&s,req("two",vec![])).unwrap();assert_eq!(retry.rows.len(),1);assert_eq!(retry.revision,b.revision);
 }
 #[test]
 fn both_sides_changed_same_field_preserves_each_side_until_resolution(){
  let s=fixture();let initial=exchange(&s,req("a",vec![])).unwrap();let mut phone=initial.rows;phone[0].customer="手机客户".into();
  s.inner.lock().unwrap().rows[0].customer="电脑客户".into();let a=exchange(&s,req("b",phone)).unwrap();
  assert_eq!(a.rows[0].customer,"手机客户");assert_eq!(s.inner.lock().unwrap().rows[0].customer,"电脑客户");
  let b=exchange(&s,req("c",a.rows)).unwrap();assert_eq!(b.conflicts.len(),1,"刷新不能绕过未解决的冲突");
 }
 #[test]
 fn missing_inbound_same_name_uses_quantity_and_preserves_both_rows(){
  let s=fixture();let mut a=s.inner.lock().unwrap().rows[0].clone();a.inbound.clear();a.name="美规焊枪200钉".into();a.qty=33;a.seq=11;
  let mut b=a.clone();b.qty=1;b.seq=12;s.inner.lock().unwrap().rows=vec![a.clone(),b.clone()];
  let mut p=a;p.seq=0;p.sync_id="phone-a".into();let mut q=b;q.seq=0;q.sync_id="phone-b".into();
  let synced=exchange(&s,req("first",vec![q,p])).unwrap();assert_eq!(synced.rows.len(),2);assert!(synced.conflicts.is_empty());
  assert_eq!(synced.rows.iter().find(|r|r.qty==33).unwrap().sync_id,"phone-a");
  assert_eq!(synced.rows.iter().find(|r|r.qty==1).unwrap().sync_id,"phone-b");
  assert_eq!(s.db.lock().unwrap().ledger_row_count().unwrap(),2);
  let again=exchange(&s,req("second",synced.rows)).unwrap();assert_eq!(again.rows.len(),2);
 }
 #[test]
 fn identical_missing_inbound_rows_still_require_unique_original_row(){
  let s=fixture();let mut a=s.inner.lock().unwrap().rows[0].clone();a.inbound.clear();a.seq=11;let mut b=a.clone();b.seq=12;
  s.inner.lock().unwrap().rows=vec![a.clone(),b.clone()];a.sync_id="a".into();b.sync_id="b".into();
  assert_eq!(exchange(&s,req("known",vec![a.clone(),b.clone()])).unwrap().rows.len(),2);
  let fresh=fixture();fresh.inner.lock().unwrap().rows=s.inner.lock().unwrap().rows.clone();for r in &mut fresh.inner.lock().unwrap().rows{r.sync_id.clear();}
  a.seq=0;b.seq=0;let error=exchange(&fresh,req("unknown",vec![a,b])).err().unwrap();assert_eq!(error.0,409);assert!(error.1.contains("缺少入库号"));assert!(!error.1.contains("同入库号"));
 }
 #[test]
 fn newly_retained_identical_row_never_reuses_phone_record_id(){
  let s=fixture();s.inner.lock().unwrap().rows[0].seq=10;
  let initial=exchange(&s,req("old",vec![])).unwrap();
  let mut restored=s.inner.lock().unwrap().rows[0].clone();restored.seq=16;restored.sync_id.clear();restored.source_identity.clear();
  s.inner.lock().unwrap().rows.push(restored);
  let response=exchange(&s,req("restored",initial.rows)).unwrap();assert_eq!(response.rows.len(),2);assert_ne!(response.rows[0].sync_id,response.rows[1].sync_id);assert_eq!(response.rows.iter().map(|r|r.qty).sum::<i64>(),14);
  assert_eq!(s.db.lock().unwrap().ledger_row_count().unwrap(),2);
 }
 #[test]
 fn desktop_edit_is_persisted_and_stale_edit_is_rejected(){
  let s=fixture();let before=s.inner.lock().unwrap().rows[0].clone();
  let changed=edit_local(&s,before.clone(),BTreeMap::from([("customer".into(),json!("杨总"))])).unwrap();
  assert_eq!(changed.customer,"杨总");assert!(changed.manual_fields.contains(&"customer".into()));
  let db=s.db.lock().unwrap();let state=load_state(&db).unwrap();
  assert_eq!(state.records[0].customer,"杨总");assert_eq!(state.operations.len(),1);
  assert!(matches!(edit_local(&s,before,BTreeMap::from([("customer".into(),json!("旧页面修改"))])),Err((409,_))));
  assert_eq!(s.inner.lock().unwrap().rows[0].customer,"杨总");
 }
 fn notebook_request(id:&str,notes:Vec<WaybillNote>)->Request {Request{mode:"waybill-notebook".into(),waybill_notes:notes,..req(id,vec![])}}
 fn note(row:&MobileRow,number:&str)->WaybillNote {WaybillNote{id:"note".into(),waybill:number.into(),inbound:row.inbound.clone(),name:row.name.clone(),destination:row.destination.clone(),..Default::default()}}
 #[test]
 fn notebook_split_35_becomes_cloud_20_and_15_without_importing_old_state(){
  let s=fixture();let mut a=s.inner.lock().unwrap().rows[0].clone();a.qty=20;a.checked=true;a.source_row=Some(273);a.seq=97;a.waybill.clear();
  let mut b=a.clone();b.qty=15;b.checked=false;b.source_row=Some(291);b.seq=115;
  s.inner.lock().unwrap().rows=vec![a.clone(),b];
  let response=exchange(&s,notebook_request("split",vec![note(&a,"6812700001")])).unwrap();
  assert_eq!(response.rows.len(),2);assert_eq!(response.rows.iter().map(|r|r.qty).sum::<i64>(),35);
  assert!(response.rows[0].checked);assert!(!response.rows[1].checked);
  assert!(response.rows.iter().all(|r|r.waybill.is_empty()),"不能把旧总行单号猜给两个分批");
  assert!(response.conflicts.is_empty());assert_eq!(response.mode,"waybill-notebook");
 }
 #[test]
 fn notebook_phone_quantities_customers_and_status_are_never_imported(){
  let s=fixture();let cloud=s.inner.lock().unwrap().rows[0].clone();
  let mut old=cloud.clone();old.qty=999;old.customer="手机旧客户".into();old.checked=true;
  let mut request=notebook_request("authoritative",vec![]);request.rows=vec![old];
  let response=exchange(&s,request).unwrap();let row=&response.rows[0];
  assert_eq!(row.qty,7);assert_eq!(row.customer,"客户");assert!(!row.checked);assert!(response.conflicts.is_empty());
 }
 #[test]
 fn notebook_fills_only_unique_empty_waybill_and_does_not_replace_cloud_number(){
  let s=fixture();let cloud=s.inner.lock().unwrap().rows[0].clone();
  let response=exchange(&s,notebook_request("keep",vec![note(&cloud,"PHONE")])).unwrap();assert_eq!(response.rows[0].waybill,"W");
  let empty=fixture();empty.inner.lock().unwrap().rows[0].waybill.clear();
  let first=exchange(&empty,notebook_request("fill",vec![note(&cloud,"PHONE")])).unwrap();assert_eq!(first.rows[0].waybill,"PHONE");
  assert_eq!(first.rows[0].qty,7);assert!(!first.rows[0].checked);
  let second=exchange(&empty,notebook_request("again",vec![note(&cloud,"PHONE")])).unwrap();assert_eq!(first.revision,second.revision);
  assert!(matches!(exchange(&empty,req("legacy",vec![])),Err((409,_))));
 }
 #[test]
 fn notebook_explicit_row_link_applies_number_only_to_chosen_split(){
  let s=fixture();let mut a=s.inner.lock().unwrap().rows[0].clone();a.qty=20;a.waybill.clear();a.source_row=Some(273);let mut b=a.clone();b.qty=15;b.source_row=Some(291);
  s.inner.lock().unwrap().rows=vec![a.clone(),b.clone()];let mut n=note(&a,"PHONE");n.target_identity=notebook_identity(&b);
  let response=exchange(&s,notebook_request("link",vec![n])).unwrap();assert!(response.rows[0].waybill.is_empty());assert_eq!(response.rows[1].waybill,"PHONE");
 }
 #[test]
 fn notebook_no_loaded_cloud_does_not_erase_phone_and_retries_are_idempotent(){
  let empty=fixture();empty.inner.lock().unwrap().rows.clear();assert!(matches!(exchange(&empty,notebook_request("empty",vec![])),Err((409,_))));
  let s=fixture();let first=exchange(&s,notebook_request("once",vec![])).unwrap();let again=exchange(&s,notebook_request("once",vec![])).unwrap();assert_eq!(first.revision,again.revision);assert_eq!(first.rows.len(),again.rows.len());
 }
 #[test]
 #[ignore]
 fn browser_roundtrip_with_real_indexeddb_and_two_isolated_computers(){
  let company=fixture();let home=fixture();
  let (a,stop_a)=crate::mobile::isolated_test_server(&company);let(b,stop_b)=crate::mobile::isolated_test_server(&home);
  let repo=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
  let status=std::process::Command::new("node").arg(".tmp/verify-phone-master-roundtrip.cjs").env("MASTER_COMPANY_PORT",a.to_string()).env("MASTER_HOME_PORT",b.to_string()).current_dir(repo).status();
  stop_a.store(true,std::sync::atomic::Ordering::Relaxed);stop_b.store(true,std::sync::atomic::Ordering::Relaxed);
  assert!(status.unwrap().success());
  assert!(company.inner.lock().unwrap().rows[0].checked);assert!(home.inner.lock().unwrap().rows[0].checked);
  assert_eq!(company.inner.lock().unwrap().rows[0].customer,"手机修订客户");
 }
}
