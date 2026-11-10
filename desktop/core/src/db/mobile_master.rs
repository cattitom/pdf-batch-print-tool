use super::Db;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::collections::BTreeMap;

impl Db {
    /// 手机同步结果、业务覆盖、出货状态和备份在同一事务提交。
    pub fn commit_mobile_master(&self, rows: &[Value], state: &Value, phone: &str, backup: &Value) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let read = |key: &str| -> rusqlite::Result<Value> {
            let raw: Option<String> = tx.query_row("SELECT value FROM settings WHERE key=?1", [key], |r|r.get(0)).optional()?;
            Ok(raw.and_then(|s|serde_json::from_str::<Value>(&s).ok()).filter(Value::is_object).unwrap_or(json!({})))
        };
        let mut patches = read("packing_waybill_patch")?;
        let mut customers = read("packing_customer_corrections_v1")?;
        let mut specials = read("packing_special_shipments")?;
        let mut overrides = read("packing_master_overrides_v1")?;
        let mut archives: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for row in rows {
            let text = |k: &str| row[k].as_str().unwrap_or("").trim().to_owned();
            let (w, i, n, c, d) = (text("waybill"),text("inbound"),text("name"),text("customer"),text("destination"));
            let legacy_identity=json!([i,n,d]).to_string();
            let identity = json!([i,n,d,row["seq"].as_u64().unwrap_or(0)]).to_string();
            let check_key = json!([w,i,n]).to_string();
            let fields: Vec<&str> = row["manualFields"].as_array().map(|a|a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            if fields.contains(&"checked") {
                tx.execute("INSERT INTO packing_checks(waybill,inbound,name,checked) VALUES(?1,?2,?3,?4) ON CONFLICT(waybill,inbound,name) DO UPDATE SET checked=excluded.checked,updated_at=datetime('now','localtime')",params![w,i,n,row["checked"].as_bool().unwrap_or(false)])?;
            }
            let issues = row["issues"].as_array().cloned().unwrap_or_default();
            let special = if issues.iter().any(|x|x=="special_shenzhen") {Some("发深圳")} else if issues.iter().any(|x|x=="special_amazon") {Some("亚马逊")} else {None};
            if let Some(label) = special { specials[&check_key] = json!(label); }
            else if fields.contains(&"issues") { specials.as_object_mut().unwrap().remove(&check_key); }
            if fields.contains(&"waybill") && !i.is_empty() {
                let key=format!("{i}|{n}");
                if w.is_empty() { patches.as_object_mut().unwrap().remove(&key); } else { patches[&key]=json!(w); }
            }
            if fields.contains(&"customer") && !i.is_empty() {customers[&legacy_identity]=json!(c);}
            if !fields.is_empty() {
                let mut value=json!({"syncId":row["syncId"],"manualFields":fields});
                for field in &fields {value[*field]=row[*field].clone();}
                if let Some(source)=row["sourceIdentity"].as_str().filter(|s|!s.is_empty()) {overrides[source]=value.clone();}
                overrides[&legacy_identity]=value.clone();overrides[&identity]=value;
            }
            archives.entry(if w.is_empty() {"__phone_master_unassigned__".into()} else {w.clone()}).or_default().push(row.clone());
            let cargo=row.get("cargo").cloned().unwrap_or(json!({}));
            let seq=row["seq"].as_i64().unwrap_or(0);
            let key=json!([c,i,n,d,w,seq]).to_string();
            tx.execute("INSERT INTO packing_cargo(record_key,customer,inbound,name,destination,waybill,seq,qty,cargo_json,source) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'手机主库同步') ON CONFLICT(record_key) DO UPDATE SET qty=excluded.qty,cargo_json=excluded.cargo_json,updated_at=datetime('now','localtime')",params![key,c,i,n,d,w,seq,row["qty"].as_i64().unwrap_or(0),cargo.to_string()])?;
        }
        let put=|key:&str,value:&Value|tx.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value.to_string()]);
        for (w,items) in archives {
            let key=format!("packing_archive_v1:{w}");
            let existing=read(&key)?;
            let mut combined: BTreeMap<String,Value>=BTreeMap::new();
            for item in existing["items"].as_array().into_iter().flatten().chain(items.iter()) {
                let key=json!([item["inbound"],item["name"],item["destination"],item["seq"].as_u64().unwrap_or(0)]).to_string();combined.insert(key,item.clone());
            }
            // 自愈：同货同量的历史副本不再累积——有现行行时丢弃副本，没有现行行时只留一条。
            let all: Vec<Value>=combined.into_values().collect();
            let text=|v:&Value,k:&str|v[k].as_str().unwrap_or("").trim().to_owned();
            let history=|v:&Value|v["issues"].as_array().is_some_and(|a|a.iter().any(|x|x=="history_archive"));
            let live: Vec<(String,String,Option<String>,i64)>=all.iter().filter(|v|!history(v)).map(|v|(text(v,"inbound"),text(v,"name"),Some(text(v,"destination")),v["qty"].as_i64().unwrap_or(0))).collect();
            let mut seen:std::collections::BTreeSet<String>=std::collections::BTreeSet::new();
            let kept: Vec<Value>=all.into_iter().filter(|v|{
                if !history(v) {return true;}
                let (i,n,d,q)=(text(v,"inbound"),text(v,"name"),text(v,"destination"),v["qty"].as_i64().unwrap_or(0));
                let duplicated=live.iter().any(|(li,ln,ld,lq)|*li==i&&*ln==n&&*lq==q&&(ld.as_deref()==Some(d.as_str())||d.is_empty()||ld.as_deref()==Some("")));
                !duplicated&&seen.insert(format!("{i}|{n}|{q}"))
            }).collect();
            put(&key,&json!({"waybill":w,"items":kept,"updatedAt":state["updatedAt"]}))?;
        }
        put("packing_waybill_patch",&patches)?;put("packing_customer_corrections_v1",&customers)?;
        put("packing_special_shipments",&specials)?;put("packing_master_overrides_v1",&overrides)?;
        put("packing_master_state_v1",state)?;
        let key=format!("packing_phone_backup:{phone}");
        let previous=read(&key)?;put(&format!("{key}:previous"),&previous)?;put(&key,backup)?;
        tx.commit()
    }
}
