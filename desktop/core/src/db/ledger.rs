//! v4：出货台账正规化存储。台账、合并基线、冲突、存档各一张表，
//! 唯一约束放在数据库层，重复行插不进去，不再依赖应用逻辑去重。
use super::Db;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS ledger_meta (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    mode        TEXT NOT NULL DEFAULT '',
    computer_id TEXT NOT NULL DEFAULT '',
    revision    INTEGER NOT NULL DEFAULT 0,
    updated_at  TEXT NOT NULL DEFAULT '',
    operations  TEXT NOT NULL DEFAULT '[]'
);
CREATE TABLE IF NOT EXISTS ledger_row (
    sync_id           TEXT PRIMARY KEY,
    waybill           TEXT NOT NULL DEFAULT '',
    inbound           TEXT NOT NULL DEFAULT '',
    name              TEXT NOT NULL DEFAULT '',
    customer          TEXT NOT NULL DEFAULT '',
    destination       TEXT NOT NULL DEFAULT '',
    qty               INTEGER NOT NULL DEFAULT 0,
    seq               INTEGER NOT NULL DEFAULT 0,
    source_row        INTEGER,
    checked           INTEGER NOT NULL DEFAULT 0,
    archived          INTEGER NOT NULL DEFAULT 0,
    blocked           INTEGER NOT NULL DEFAULT 0,
    task              INTEGER NOT NULL DEFAULT 0,
    task_id           TEXT NOT NULL DEFAULT '',
    claimed_by        TEXT NOT NULL DEFAULT '',
    cloud_authoritative INTEGER NOT NULL DEFAULT 0,
    quantity_group    TEXT,
    qty_raw           TEXT NOT NULL DEFAULT '',
    arrival           TEXT NOT NULL DEFAULT '',
    remarks           TEXT NOT NULL DEFAULT '',
    source_id         TEXT NOT NULL DEFAULT '',
    source_identity   TEXT NOT NULL DEFAULT '',
    record_id         TEXT NOT NULL DEFAULT '',
    dataset_id        TEXT NOT NULL DEFAULT '',
    version           INTEGER NOT NULL DEFAULT 0,
    master_revision   INTEGER NOT NULL DEFAULT 0,
    issues            TEXT NOT NULL DEFAULT '[]',
    cargo             TEXT NOT NULL DEFAULT '{}',
    manual_fields     TEXT NOT NULL DEFAULT '[]',
    known_fields      TEXT NOT NULL DEFAULT '[]',
    updated_at        TEXT NOT NULL DEFAULT (datetime('now','localtime'))
);
CREATE UNIQUE INDEX IF NOT EXISTS ledger_row_source ON ledger_row(waybill, source_row) WHERE source_row IS NOT NULL AND archived = 0;
CREATE UNIQUE INDEX IF NOT EXISTS ledger_row_key ON ledger_row(waybill, inbound, name, qty, seq) WHERE source_row IS NULL AND archived = 0;
CREATE UNIQUE INDEX IF NOT EXISTS ledger_row_archive ON ledger_row(waybill, inbound, name, qty) WHERE archived = 1;
CREATE INDEX IF NOT EXISTS idx_ledger_row_waybill ON ledger_row(waybill);
CREATE TABLE IF NOT EXISTS ledger_conflict (
    device_id  TEXT NOT NULL,
    record_id  TEXT NOT NULL,
    field      TEXT NOT NULL,
    phone      TEXT NOT NULL DEFAULT '',
    computer   TEXT NOT NULL DEFAULT '',
    name       TEXT NOT NULL DEFAULT '',
    inbound    TEXT NOT NULL DEFAULT '',
    source_row INTEGER,
    PRIMARY KEY (device_id, record_id, field)
);
CREATE TABLE IF NOT EXISTS ledger_peer (
    device_id  TEXT PRIMARY KEY,
    request_id TEXT NOT NULL DEFAULT '',
    response   TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS ledger_peer_base (
    device_id   TEXT NOT NULL,
    sync_id     TEXT NOT NULL,
    inbound     TEXT NOT NULL DEFAULT '',
    name        TEXT NOT NULL DEFAULT '',
    destination TEXT NOT NULL DEFAULT '',
    qty         INTEGER NOT NULL DEFAULT 0,
    seq         INTEGER NOT NULL DEFAULT 0,
    source_row  INTEGER,
    waybill     TEXT NOT NULL DEFAULT '',
    checked     INTEGER NOT NULL DEFAULT 0,
    customer    TEXT NOT NULL DEFAULT '',
    arrival     TEXT NOT NULL DEFAULT '',
    remarks     TEXT NOT NULL DEFAULT '',
    cargo       TEXT NOT NULL DEFAULT '{}',
    issues      TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (device_id, sync_id)
);
CREATE TABLE IF NOT EXISTS archive_ticket (
    waybill    TEXT PRIMARY KEY,
    updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime'))
);
CREATE TABLE IF NOT EXISTS archive_item (
    waybill     TEXT NOT NULL,
    inbound     TEXT NOT NULL DEFAULT '',
    name        TEXT NOT NULL DEFAULT '',
    customer    TEXT NOT NULL DEFAULT '',
    destination TEXT NOT NULL DEFAULT '',
    dup         INTEGER NOT NULL DEFAULT 0,
    seq         INTEGER NOT NULL DEFAULT 0,
    source_row  INTEGER,
    quantity_group TEXT,
    sync_id     TEXT NOT NULL DEFAULT '',
    qty         INTEGER NOT NULL DEFAULT 0,
    qty_raw     TEXT NOT NULL DEFAULT '',
    arrival     TEXT NOT NULL DEFAULT '',
    checked     INTEGER NOT NULL DEFAULT 0,
    issues      TEXT NOT NULL DEFAULT '[]',
    cargo       TEXT NOT NULL DEFAULT '{}',
    manual_fields TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (waybill, inbound, name, customer, destination, dup)
);
"#;

fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").trim().to_owned()
}
fn raw_text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").to_owned()
}
fn int(v: &Value, key: &str) -> i64 {
    v[key].as_i64().or_else(|| v[key].as_u64().map(|n| n as i64)).unwrap_or(0)
}
fn flag(v: &Value, key: &str) -> i64 {
    if v[key].as_bool().unwrap_or(false) { 1 } else { 0 }
}
fn opt_int(v: &Value, key: &str) -> Option<i64> {
    v[key].as_i64().or_else(|| v[key].as_u64().map(|n| n as i64))
}
fn opt_text(v: &Value, key: &str) -> Option<String> {
    v[key].as_str().map(str::to_owned)
}
fn list(v: &Value, key: &str) -> String {
    v[key].as_array().map(|a| Value::Array(a.clone())).unwrap_or_else(|| json!([])).to_string()
}
fn object(v: &Value, key: &str) -> String {
    match &v[key] {
        Value::Object(_) => v[key].to_string(),
        _ => "{}".into(),
    }
}
fn is_history(v: &Value) -> bool {
    v["issues"].as_array().is_some_and(|a| a.iter().any(|i| i == "history_archive"))
}

/// 写库前按唯一索引的同一套键合并重复行。
///
/// 唯一索引是最后一道防线，但真撞上时整次同步会失败——所以更稳妥的做法是
/// 在写之前就合并：同票同原表行只留一条，历史副本同货同量只留一条。
/// 合并时把有用的信息补到保留行上（已出状态取并、缺的入库号/客户/目的地补齐、issues 合并）。
fn dedupe_rows(rows: Vec<Value>) -> Vec<Value> {
    let mut kept = dedupe_same_key(rows);
    drop_history_twins(&mut kept);
    kept
}

/// 历史副本已经有同货同量的现行行时直接丢掉，把「已出」并上去。
/// 没有现行行的（整票出货完成、云表已移除单号）必须保留，否则台账会被截短。
fn drop_history_twins(rows: &mut Vec<Value>) {
    let mut drop = vec![false; rows.len()];
    for i in 0..rows.len() {
        if !is_history(&rows[i]) {
            continue;
        }
        let copy_waybill = text(&rows[i], "waybill");
        let keeper = (0..rows.len()).find(|&j| {
            j != i
                && !drop[j]
                && !is_history(&rows[j])
                && int(&rows[j], "qty") == int(&rows[i], "qty")
                && same_goods(&rows[j], &rows[i])
                // 未分配的快照只要现行行同货同量就算同一件货（那行后来被分到某票上了）；
                // 已分票的副本必须同票才算，避免把另一个已出货的票消掉。
                && (copy_waybill.is_empty() || text(&rows[j], "waybill") == copy_waybill)
        });
        let Some(j) = keeper else { continue };
        if flag(&rows[i], "checked") == 1 {
            rows[j]["checked"] = json!(true);
        }
        if text(&rows[j], "waybill").is_empty() && !copy_waybill.is_empty() {
            rows[j]["waybill"] = json!(copy_waybill);
        }
        drop[i] = true;
    }
    let mut flag_iter = drop.into_iter();
    rows.retain(|_| !flag_iter.next().unwrap_or(false));
}

/// 同一件货的判定，与手机合并用的 equivalent 一致：
/// 入库号 + 商品名相同（至少一个非空），目的地相同或一侧为空。
fn same_goods(a: &Value, b: &Value) -> bool {
    let (ai, an) = (text(a, "inbound"), text(a, "name"));
    let (bi, bn) = (text(b, "inbound"), text(b, "name"));
    if ai != bi || an != bn || (ai.is_empty() && an.is_empty()) {
        return false;
    }
    let (ad, bd) = (text(a, "destination"), text(b, "destination"));
    ad == bd || ad.is_empty() || bd.is_empty()
}

fn dedupe_same_key(rows: Vec<Value>) -> Vec<Value> {
    let mut kept: Vec<Value> = Vec::with_capacity(rows.len());
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for row in rows {
        let history = is_history(&row);
        let waybill = text(&row, "waybill");
        let key = if history {
            format!("h|{waybill}|{}|{}|{}", text(&row, "inbound"), text(&row, "name"), int(&row, "qty"))
        } else if let Some(source) = opt_int(&row, "sourceRow") {
            format!("s|{waybill}|{source}")
        } else {
            format!(
                "k|{waybill}|{}|{}|{}|{}",
                text(&row, "inbound"),
                text(&row, "name"),
                int(&row, "qty"),
                int(&row, "seq")
            )
        };
        match index.get(&key) {
            Some(&at) => merge_row(&mut kept[at], &row),
            None => {
                index.insert(key, kept.len());
                kept.push(row);
            }
        }
    }
    kept
}

/// 把副本里更完整的信息并到保留行，不覆盖已有值。
fn merge_row(into: &mut Value, from: &Value) {
    let done = |v: &Value| v["checked"].as_bool().unwrap_or(false) || v["checked"].as_i64().is_some_and(|n| n != 0);
    if done(from) {
        into["checked"] = json!(true);
    }
    for field in ["inbound", "customer", "destination", "arrival", "qtyRaw", "taskId", "claimedBy"] {
        if text(into, field).is_empty() && !text(from, field).is_empty() {
            into[field] = from[field].clone();
        }
    }
    if let Some(issues) = from["issues"].as_array() {
        let mut merged: Vec<Value> = into["issues"].as_array().cloned().unwrap_or_default();
        for issue in issues {
            if !merged.contains(issue) {
                merged.push(issue.clone());
            }
        }
        into["issues"] = Value::Array(merged);
    }
}

/// 台账主状态。字段与旧 JSON 结构一致，方便上层平滑迁移。
#[derive(Debug, Clone, Default)]
pub struct LedgerState {
    pub mode: String,
    pub computer_id: String,
    pub revision: i64,
    pub updated_at: String,
    pub operations: String,
    pub rows: Vec<Value>,
    pub peers: BTreeMap<String, Value>,
}

impl Db {
    pub fn ledger_state(&self) -> rusqlite::Result<LedgerState> {
        let mut state = LedgerState::default();
        let meta = self
            .conn
            .query_row(
                "SELECT mode,computer_id,revision,updated_at,operations FROM ledger_meta WHERE id=1",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?;
        if let Some((mode, computer_id, revision, updated_at, operations)) = meta {
            state.mode = mode;
            state.computer_id = computer_id;
            state.revision = revision;
            state.updated_at = updated_at;
            state.operations = operations;
        }
        state.rows = self.ledger_rows()?;
        state.peers = self.ledger_peers()?;
        Ok(state)
    }

    pub fn ledger_rows(&self) -> rusqlite::Result<Vec<Value>> {
        let mut statement = self.conn.prepare(
            "SELECT sync_id,waybill,inbound,name,customer,destination,qty,seq,source_row,checked,archived,
                    blocked,task,task_id,claimed_by,cloud_authoritative,quantity_group,qty_raw,arrival,remarks,
                    source_id,source_identity,record_id,dataset_id,version,master_revision,issues,cargo,manual_fields,known_fields
             FROM ledger_row ORDER BY rowid",
        )?;
        let rows = statement.query_map([], |r| {
            let list = |i: usize| -> rusqlite::Result<Value> {
                let raw: String = r.get(i)?;
                Ok(serde_json::from_str(&raw).unwrap_or_else(|_| json!([])))
            };
            Ok(json!({
                "syncId": r.get::<_, String>(0)?,
                "waybill": r.get::<_, String>(1)?,
                "inbound": r.get::<_, String>(2)?,
                "name": r.get::<_, String>(3)?,
                "customer": r.get::<_, String>(4)?,
                "destination": r.get::<_, String>(5)?,
                "qty": r.get::<_, i64>(6)?,
                "seq": r.get::<_, i64>(7)?,
                "sourceRow": r.get::<_, Option<i64>>(8)?,
                "checked": r.get::<_, i64>(9)? != 0,
                "blocked": r.get::<_, i64>(11)? != 0,
                "task": r.get::<_, i64>(12)? != 0,
                "taskId": r.get::<_, String>(13)?,
                "claimedBy": r.get::<_, String>(14)?,
                "cloudAuthoritative": r.get::<_, i64>(15)? != 0,
                "quantityGroup": r.get::<_, Option<String>>(16)?,
                "qtyRaw": r.get::<_, String>(17)?,
                "arrival": r.get::<_, String>(18)?,
                "remarks": r.get::<_, String>(19)?,
                "sourceId": r.get::<_, String>(20)?,
                "sourceIdentity": r.get::<_, String>(21)?,
                "recordId": r.get::<_, String>(22)?,
                "datasetId": r.get::<_, String>(23)?,
                "version": r.get::<_, i64>(24)?,
                "masterRevision": r.get::<_, i64>(25)?,
                "issues": list(26)?,
                "cargo": serde_json::from_str::<Value>(&r.get::<_, String>(27)?).unwrap_or_else(|_| json!({})),
                "manualFields": list(28)?,
                "knownFields": list(29)?,
            }))
        })?;
        rows.collect()
    }

    fn ledger_peers(&self) -> rusqlite::Result<BTreeMap<String, Value>> {
        let mut peers = BTreeMap::new();
        let mut statement = self.conn.prepare("SELECT device_id,request_id,response FROM ledger_peer")?;
        let rows = statement.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        for row in rows {
            let (device_id, request_id, response) = row?;
            peers.insert(
                device_id,
                json!({
                    "requestId": request_id,
                    "response": serde_json::from_str::<Value>(&response).unwrap_or(Value::Null),
                    "base": [],
                    "conflicts": [],
                }),
            );
        }
        let mut base = self.conn.prepare(
            "SELECT device_id,sync_id,inbound,name,destination,qty,seq,source_row,waybill,checked,
                    customer,arrival,remarks,cargo,issues
             FROM ledger_peer_base ORDER BY rowid",
        )?;
        let rows = base.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                json!({
                    "syncId": r.get::<_, String>(1)?,
                    "inbound": r.get::<_, String>(2)?,
                    "name": r.get::<_, String>(3)?,
                    "destination": r.get::<_, String>(4)?,
                    "qty": r.get::<_, i64>(5)?,
                    "seq": r.get::<_, i64>(6)?,
                    "sourceRow": r.get::<_, Option<i64>>(7)?,
                    "waybill": r.get::<_, String>(8)?,
                    "checked": r.get::<_, i64>(9)? != 0,
                    "customer": r.get::<_, String>(10)?,
                    "arrival": r.get::<_, String>(11)?,
                    "remarks": r.get::<_, String>(12)?,
                    "cargo": serde_json::from_str::<Value>(&r.get::<_, String>(13)?).unwrap_or_else(|_| json!({})),
                    "issues": serde_json::from_str::<Value>(&r.get::<_, String>(14)?).unwrap_or_else(|_| json!([])),
                }),
            ))
        })?;
        for row in rows {
            let (device_id, value) = row?;
            if let Some(peer) = peers.get_mut(&device_id) {
                if let Some(list) = peer["base"].as_array_mut() {
                    list.push(value);
                }
            }
        }
        let mut conflicts = self.conn.prepare(
            "SELECT device_id,record_id,field,phone,computer,name,inbound,source_row FROM ledger_conflict",
        )?;
        let rows = conflicts.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                json!({
                    "recordId": r.get::<_, String>(1)?,
                    "field": r.get::<_, String>(2)?,
                    "phone": serde_json::from_str::<Value>(&r.get::<_, String>(3)?).unwrap_or(Value::Null),
                    "computer": serde_json::from_str::<Value>(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
                    "name": r.get::<_, String>(5)?,
                    "inbound": r.get::<_, String>(6)?,
                    "sourceRow": r.get::<_, Option<i64>>(7)?,
                }),
            ))
        })?;
        for row in rows {
            let (device_id, value) = row?;
            if let Some(peer) = peers.get_mut(&device_id) {
                if let Some(list) = peer["conflicts"].as_array_mut() {
                    list.push(value);
                }
            }
        }
        Ok(peers)
    }

    /// 整份台账 + 基线/冲突在同一事务里落地。
    /// device_id 为空表示迁移：保存全部设备的基线与冲突。
    pub fn save_ledger(
        &self,
        state: &LedgerState,
        device_id: Option<&str>,
        backup: &Value,
    ) -> rusqlite::Result<()> {
        let rows = dedupe_rows(state.rows.clone());
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO ledger_meta(id,mode,computer_id,revision,updated_at,operations) VALUES(1,?1,?2,?3,?4,?5)
             ON CONFLICT(id) DO UPDATE SET mode=excluded.mode,computer_id=excluded.computer_id,
             revision=excluded.revision,updated_at=excluded.updated_at,operations=excluded.operations",
            params![state.mode, state.computer_id, state.revision, state.updated_at, state.operations],
        )?;
        {
            // 这次写入就是整份台账：先清空再落，避免「同一票同一原表行、syncId 换过了」
            // 撞上唯一索引——那会让整次手机同步退回旧数据（手机上就是这么报错的）。
            tx.execute("DELETE FROM ledger_row", [])?;
            let mut statement = tx.prepare(
                "INSERT OR REPLACE INTO ledger_row(sync_id,waybill,inbound,name,customer,destination,qty,seq,source_row,
                    checked,archived,blocked,task,task_id,claimed_by,cloud_authoritative,quantity_group,qty_raw,
                    arrival,remarks,source_id,source_identity,record_id,dataset_id,version,master_revision,
                    issues,cargo,manual_fields,known_fields)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30)",
            )?;
            for row in &rows {
                statement.execute(params![
                    text(row, "syncId"), text(row, "waybill"), text(row, "inbound"), text(row, "name"),
                    text(row, "customer"), text(row, "destination"), int(row, "qty"), int(row, "seq"),
                    opt_int(row, "sourceRow"), flag(row, "checked"),
                    if is_history(row) { 1 } else { 0 }, flag(row, "blocked"), flag(row, "task"),
                    text(row, "taskId"), text(row, "claimedBy"), flag(row, "cloudAuthoritative"),
                    opt_text(row, "quantityGroup"), raw_text(row, "qtyRaw"), text(row, "arrival"),
                    text(row, "remarks"), text(row, "sourceId"), raw_text(row, "sourceIdentity"),
                    raw_text(row, "recordId"), raw_text(row, "datasetId"), int(row, "version"),
                    int(row, "masterRevision"), list(row, "issues"), object(row, "cargo"),
                    list(row, "manualFields"), list(row, "knownFields"),
                ])?;
            }
        }
        let devices: Vec<String> = match device_id {
            Some(device) => vec![device.to_owned()],
            None => state.peers.keys().cloned().collect(),
        };
        for device in &devices {
            tx.execute("DELETE FROM ledger_peer_base WHERE device_id=?1", [device])?;
            tx.execute("DELETE FROM ledger_conflict WHERE device_id=?1", [device])?;
        }
        for device in &devices {
            let Some(peer) = state.peers.get(device) else { continue };
            tx.execute(
                "INSERT INTO ledger_peer(device_id,request_id,response) VALUES(?1,?2,?3)
                 ON CONFLICT(device_id) DO UPDATE SET request_id=excluded.request_id,response=excluded.response",
                params![device, text(peer, "requestId"), peer["response"].to_string()],
            )?;
            let mut base = tx.prepare(
                "INSERT INTO ledger_peer_base(device_id,sync_id,inbound,name,destination,qty,seq,source_row,waybill,checked,
                    customer,arrival,remarks,cargo,issues)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                 ON CONFLICT(device_id,sync_id) DO UPDATE SET inbound=excluded.inbound,name=excluded.name,
                 destination=excluded.destination,qty=excluded.qty,seq=excluded.seq,
                 source_row=excluded.source_row,waybill=excluded.waybill,checked=excluded.checked,
                 customer=excluded.customer,arrival=excluded.arrival,remarks=excluded.remarks,
                 cargo=excluded.cargo,issues=excluded.issues",
            )?;
            for row in peer["base"].as_array().into_iter().flatten() {
                base.execute(params![
                    device, text(row, "syncId"), text(row, "inbound"), text(row, "name"),
                    text(row, "destination"), int(row, "qty"), int(row, "seq"), opt_int(row, "sourceRow"),
                    text(row, "waybill"), flag(row, "checked"), text(row, "customer"), text(row, "arrival"),
                    text(row, "remarks"), object(row, "cargo"), list(row, "issues"),
                ])?;
            }
            let mut conflict = tx.prepare(
                "INSERT INTO ledger_conflict(device_id,record_id,field,phone,computer,name,inbound,source_row)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(device_id,record_id,field) DO UPDATE SET phone=excluded.phone,
                 computer=excluded.computer,name=excluded.name,inbound=excluded.inbound,source_row=excluded.source_row",
            )?;
            for c in peer["conflicts"].as_array().into_iter().flatten() {
                conflict.execute(params![
                    device, text(c, "recordId"), text(c, "field"),
                    c["phone"].to_string(), c["computer"].to_string(),
                    text(c, "name"), text(c, "inbound"), opt_int(c, "sourceRow"),
                ])?;
            }
        }
        if let Some(device_id) = device_id {
            let key = format!("packing_phone_backup:{device_id}");
            let put = |key: &str, value: &Value| {
                tx.execute(
                    "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![key, value.to_string()],
                )
            };
            let previous: Option<String> = tx
                .query_row("SELECT value FROM settings WHERE key=?1", [&key], |r| r.get(0))
                .optional()?;
            if let Some(previous) = previous {
                put(&format!("{key}:previous"), &serde_json::from_str(&previous).unwrap_or(Value::Null))?;
            }
            put(&key, backup)?;
        }
        tx.commit()
    }

    /// 出货档案（前端「含存档」视图）：按票返回，键统一，不再有原文/编码两份。
    pub fn archive_tickets(&self) -> rusqlite::Result<Vec<Value>> {
        let mut tickets = self.conn.prepare(
            "SELECT waybill,updated_at FROM archive_ticket ORDER BY updated_at DESC, waybill",
        )?;
        let list: Vec<(String, String)> = tickets
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut items = self.conn.prepare(
            "SELECT waybill,inbound,name,customer,destination,seq,source_row,quantity_group,sync_id,qty,qty_raw,
                    arrival,checked,issues,cargo,manual_fields FROM archive_item ORDER BY rowid",
        )?;
        let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let rows = items.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                json!({
                    "inbound": r.get::<_, String>(1)?,
                    "name": r.get::<_, String>(2)?,
                    "customer": r.get::<_, String>(3)?,
                    "destination": r.get::<_, String>(4)?,
                    "seq": r.get::<_, i64>(5)?,
                    "sourceRow": r.get::<_, Option<i64>>(6)?,
                    "quantityGroup": r.get::<_, Option<String>>(7)?,
                    "syncId": r.get::<_, String>(8)?,
                    "qty": r.get::<_, i64>(9)?,
                    "qtyRaw": r.get::<_, String>(10)?,
                    "arrival": r.get::<_, String>(11)?,
                    "checked": r.get::<_, i64>(12)? != 0,
                    "issues": serde_json::from_str::<Value>(&r.get::<_, String>(13)?).unwrap_or_else(|_| json!([])),
                    "cargo": serde_json::from_str::<Value>(&r.get::<_, String>(14)?).unwrap_or_else(|_| json!({})),
                    "manualFields": serde_json::from_str::<Value>(&r.get::<_, String>(15)?).unwrap_or_else(|_| json!([])),
                }),
            ))
        })?;
        for row in rows {
            let (waybill, value) = row?;
            grouped.entry(waybill).or_default().push(value);
        }
        Ok(list
            .into_iter()
            .map(|(waybill, updated_at)| {
                json!({
                    "waybill": waybill,
                    "updatedAt": updated_at,
                    "items": grouped.remove(&waybill).unwrap_or_default(),
                })
            })
            .collect())
    }

    /// 前端落档：按 (票,入库号,商品,客户,目的地,同货第几次出现) 唯一。
    ///
    /// 键里刻意不含原表行号 seq：原表重新解析后行号会整体偏移，
    /// 用 seq 当键会让同一件货每解析一次就多存一份（旧版 57 行 377 件就是这么来的）。
    /// 同一件货在同一票内出现多次（拆批 20+15）时按箱数从大到小排定 dup，位置稳定。
    pub fn archive_rows(&self, rows: &[Value]) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            // 先按业务键分组，组内按「箱数」定 dup：箱数不同才是拆批（20+15），
            // 货/客户/目的地/箱数全同的多行只可能是一份，归到同一个 dup 互相覆盖。
            let mut groups: BTreeMap<(String, String, String, String, String), Vec<usize>> = BTreeMap::new();
            for (index, row) in rows.iter().enumerate() {
                if text(row, "waybill").is_empty() {
                    continue;
                }
                groups
                    .entry((
                        text(row, "waybill"),
                        text(row, "inbound"),
                        text(row, "name"),
                        text(row, "customer"),
                        text(row, "destination"),
                    ))
                    .or_default()
                    .push(index);
            }
            let mut dups = vec![0i64; rows.len()];
            let mut slots: BTreeMap<(String, String, String, String, String), usize> = BTreeMap::new();
            for (key, member) in &groups {
                let mut quantities: Vec<i64> = member.iter().map(|&i| int(&rows[i], "qty")).collect();
                quantities.sort_unstable_by(|a, b| b.cmp(a));
                quantities.dedup();
                slots.insert(key.clone(), quantities.len());
                for &index in member {
                    let qty = int(&rows[index], "qty");
                    dups[index] = quantities.iter().position(|&q| q == qty).unwrap_or(0) as i64;
                }
            }
            let mut upsert = tx.prepare(
                "INSERT INTO archive_item(waybill,inbound,name,customer,destination,dup,seq,source_row,quantity_group,
                    sync_id,qty,qty_raw,arrival,checked,issues,cargo,manual_fields)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
                 ON CONFLICT(waybill,inbound,name,customer,destination,dup) DO UPDATE SET
                 seq=excluded.seq,source_row=excluded.source_row,quantity_group=excluded.quantity_group,
                 sync_id=excluded.sync_id,qty=excluded.qty,qty_raw=excluded.qty_raw,arrival=excluded.arrival,
                 checked=excluded.checked,issues=excluded.issues,cargo=excluded.cargo,
                 manual_fields=excluded.manual_fields",
            )?;
            let mut touched = std::collections::BTreeSet::new();
            for (index, row) in rows.iter().enumerate() {
                let waybill = text(row, "waybill");
                if waybill.is_empty() {
                    continue;
                }
                touched.insert(waybill.clone());
                let inbound = text(row, "inbound");
                if !inbound.is_empty() {
                    // 后补入库号：同一商品/客户/目的地的缺号旧记录让位。
                    tx.execute(
                        "DELETE FROM archive_item WHERE waybill=?1 AND inbound='' AND name=?2 AND customer=?3 AND destination=?4",
                        params![waybill, text(row, "name"), text(row, "customer"), text(row, "destination")],
                    )?;
                }
                upsert.execute(params![
                    waybill, inbound, text(row, "name"), text(row, "customer"), text(row, "destination"),
                    dups[index], int(row, "seq"), opt_int(row, "sourceRow"), opt_text(row, "quantityGroup"),
                    text(row, "syncId"), int(row, "qty"), raw_text(row, "qtyRaw"), text(row, "arrival"),
                    flag(row, "checked"), list(row, "issues"), object(row, "cargo"), list(row, "manualFields"),
                ])?;
            }
            // 本次写入里同一件货只剩 n 份，多出来的旧副本（拆批合并后）清掉。
            for (key, keep) in &slots {
                tx.execute(
                    "DELETE FROM archive_item WHERE waybill=?1 AND inbound=?2 AND name=?3 AND customer=?4 AND destination=?5 AND dup>=?6",
                    params![key.0, key.1, key.2, key.3, key.4, *keep as i64],
                )?;
            }
            for waybill in touched {
                tx.execute(
                    "INSERT INTO archive_ticket(waybill,updated_at) VALUES(?1,datetime('now','localtime'))
                     ON CONFLICT(waybill) DO UPDATE SET updated_at=excluded.updated_at",
                    [&waybill],
                )?;
            }
        }
        tx.commit()
    }

    /// 从 v3 的 settings JSON 导入台账与存档；导入后旧键改名保留，可回退。
    pub(super) fn migrate_ledger_json(&self) -> rusqlite::Result<()> {
        let settings: Vec<(String, String)> = {
            let mut statement = self.conn.prepare("SELECT key,value FROM settings")?;
            let rows = statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            let collected: Vec<(String, String)> = rows.collect::<rusqlite::Result<_>>()?;
            collected
        };
        let mut state: Option<Value> = None;
        let mut archives = Vec::new();
        let mut consumed = Vec::new();
        for (key, value) in &settings {
            if key == "packing_master_state_v1" {
                state = serde_json::from_str(value).ok();
                consumed.push(key.clone());
            } else if key.starts_with("packing_archive_v1:") {
                archives.push(serde_json::from_str::<Value>(value).ok());
                consumed.push(key.clone());
            }
        }
        if let Some(state) = &state {
            let meta = json!({
                "mode": state["mode"].clone(),
                "computer_id": state["computerId"].clone(),
                "revision": state["revision"].clone(),
                "updated_at": state["updatedAt"].clone(),
                "operations": state["operations"].clone(),
            });
            self.conn.execute(
                "INSERT INTO ledger_meta(id,mode,computer_id,revision,updated_at,operations) VALUES(1,?1,?2,?3,?4,?5)
                 ON CONFLICT(id) DO UPDATE SET mode=excluded.mode,computer_id=excluded.computer_id,
                 revision=excluded.revision,updated_at=excluded.updated_at,operations=excluded.operations",
                params![
                    text(&meta, "mode"),
                    text(&meta, "computer_id"),
                    int(&meta, "revision"),
                    text(&meta, "updated_at"),
                    meta["operations"].to_string()
                ],
            )?;
            let rows: Vec<Value> = state["records"].as_array().cloned().unwrap_or_default();
            let mut imported = LedgerState {
                mode: text(&meta, "mode"),
                computer_id: text(&meta, "computer_id"),
                revision: int(&meta, "revision"),
                updated_at: text(&meta, "updated_at"),
                operations: meta["operations"].to_string(),
                rows: Vec::new(),
                peers: BTreeMap::new(),
            };
            // 全量读入，重复由 save_ledger 的 dedupe_rows 按同一套键合并；
            // 历史副本的「已出」状态先并到对应的现行行上，副本自己随后被折叠掉。
            imported.rows = rows;
            let checked_of: Vec<(String, String, String, i64, bool)> = imported
                .rows
                .iter()
                .filter(|r| is_history(r) && flag(r, "checked") == 1)
                .map(|r| (text(r, "waybill"), text(r, "inbound"), text(r, "name"), int(r, "qty"), true))
                .collect();
            for (waybill, inbound, name, qty, _) in checked_of {
                for live in imported.rows.iter_mut().filter(|r| !is_history(r)) {
                    if text(live, "waybill") == waybill
                        && text(live, "inbound") == inbound
                        && text(live, "name") == name
                        && int(live, "qty") == qty
                    {
                        live["checked"] = json!(true);
                    }
                }
            }
            if let Some(peers) = state["peers"].as_object() {
                for (device, peer) in peers {
                    imported.peers.insert(device.clone(), peer.clone());
                }
            }
            self.save_ledger(&imported, None, &json!({"migrated":true}))?;
        }
        for archive in archives.into_iter().flatten() {
            let waybill = text(&archive, "waybill");
            // 没有单号的伪票（旧版的 __phone_master_unassigned__）归不了位，
            // 留着只会把一堆无主明细塞进存档，直接跳过。
            if waybill.is_empty() || waybill == "__phone_master_unassigned__" {
                continue;
            }
            let items: Vec<Value> = archive["items"].as_array().cloned().unwrap_or_default();
            self.archive_rows(&items.iter().map(|item| {
                let mut value = item.clone();
                value["waybill"] = json!(waybill);
                value
            }).collect::<Vec<_>>())?;
        }
        // 旧键改名保留，回退时可查；不再参与任何读写。
        for key in consumed {
            self.conn.execute(
                "INSERT OR REPLACE INTO settings(key,value) VALUES(?1,(SELECT value FROM settings WHERE key=?2))",
                params![format!("{key}.v3bak"), key],
            )?;
            self.conn.execute("DELETE FROM settings WHERE key=?1", [&key])?;
        }
        Ok(())
    }

    pub fn ledger_row_count(&self) -> rusqlite::Result<i64> {
        self.conn.query_row("SELECT COUNT(*) FROM ledger_row", [], |r| r.get(0))
    }

    /// 当前台账协议模式（""=未启用，waybill-notebook=旧单号记忆，shipment-ledger=完整台账）。
    pub fn ledger_mode(&self) -> rusqlite::Result<String> {
        Ok(self
            .conn
            .query_row("SELECT mode FROM ledger_meta WHERE id=1", [], |r| r.get::<_, String>(0))
            .optional()?
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(waybill: &str, inbound: &str, name: &str, qty: i64, seq: i64) -> Value {
        json!({
            "syncId": format!("{waybill}-{inbound}-{name}-{qty}-{seq}"),
            "waybill": waybill, "inbound": inbound, "name": name, "qty": qty, "seq": seq,
            "checked": false, "customer": "", "destination": "", "arrival": "",
            "issues": [], "cargo": {}, "manualFields": [], "knownFields": [],
        })
    }

    fn save(db: &Db, rows: Vec<Value>) {
        let state = LedgerState { rows, ..Default::default() };
        db.save_ledger(&state, None, &json!({})).unwrap();
    }

    /// 同票同原表行重复上报：只留一条，已出状态并上去。
    #[test]
    fn duplicate_source_rows_collapse_to_one() {
        let db = Db::open_in_memory().unwrap();
        let mut phone = row("W1", "IN1", "螺丝", 10, 1);
        phone["sourceRow"] = json!(7);
        phone["syncId"] = json!("phone");
        let mut pc = row("W1", "IN1", "螺丝", 10, 1);
        pc["sourceRow"] = json!(7);
        pc["syncId"] = json!("pc");
        pc["checked"] = json!(true);

        save(&db, vec![phone.clone(), pc]);
        assert_eq!(db.ledger_row_count().unwrap(), 1);
        let rows = db.ledger_rows().unwrap();
        assert_eq!(rows[0]["checked"], json!(true), "副本的已出状态必须并到保留行");

        // 再灌一次不会长出来。
        save(&db, vec![phone]);
        assert_eq!(db.ledger_row_count().unwrap(), 1);
    }

    /// 历史副本同货同量只留一条，重复灌不会回涨。
    #[test]
    fn history_copies_collapse_and_do_not_regrow() {
        let db = Db::open_in_memory().unwrap();
        let mut a = row("W2", "IN2", "垫片", 6, 1);
        a["issues"] = json!(["history_archive"]);
        a["syncId"] = json!("h1");
        let mut b = a.clone();
        b["syncId"] = json!("h2");
        let mut c = a.clone();
        c["syncId"] = json!("h3");
        c["checked"] = json!(true);

        save(&db, vec![a, b, c]);
        assert_eq!(db.ledger_row_count().unwrap(), 1);
        let rows = db.ledger_rows().unwrap();
        assert_eq!(rows[0]["checked"], json!(true));
    }

    /// 云表只给半张票（无原表行号）时，同货同量同序号也不会变成两行。
    #[test]
    fn keyed_rows_without_source_row_are_unique() {
        let db = Db::open_in_memory().unwrap();
        save(&db, vec![row("W3", "", "胶带", 2, 1), row("W3", "", "胶带", 2, 1)]);
        assert_eq!(db.ledger_row_count().unwrap(), 1);
    }

    /// 同一个单号在旧版里被写成「原文键 + URL 编码键」两份，迁移后必须是一票一份。
    #[test]
    fn archive_double_keys_merge_into_one_ticket() {
        let db = Db::open_in_memory().unwrap();
        let ticket = json!({
            "waybill": "发深圳",
            "updatedAt": "2026-11-01T10:00:00Z",
            "items": [{
                "inbound": "IN9", "name": "纸箱", "customer": "甲", "destination": "深圳",
                "seq": 1, "qty": 4, "qtyRaw": "4", "arrival": "", "checked": true, "issues": []
            }],
        });
        db.set_setting("packing_archive_v1:发深圳", &ticket.to_string()).unwrap();
        db.set_setting("packing_archive_v1:%E5%8F%91%E6%B7%B1%E5%9C%B3", &ticket.to_string()).unwrap();

        db.migrate_ledger_json().unwrap();

        let tickets = db.archive_tickets().unwrap();
        assert_eq!(tickets.len(), 1, "编码键与原文键必须并成一票");
        assert_eq!(tickets[0]["items"].as_array().unwrap().len(), 1);

        // 旧键改名留底，不再参与读写。
        assert!(db.get_setting("packing_archive_v1:发深圳").unwrap().is_none());
        assert!(db.get_setting("packing_archive_v1:发深圳.v3bak").unwrap().is_some());
    }

    /// 后补入库号时要顶掉同货同客户同目的地的缺号旧记录，不能新旧并存。
    #[test]
    fn later_inbound_replaces_blank_inbound_item() {
        let db = Db::open_in_memory().unwrap();
        let base = json!({
            "waybill": "W4", "inbound": "", "name": "彩盒", "customer": "乙",
            "destination": "深圳", "seq": 1, "qty": 3, "qtyRaw": "3", "arrival": "",
            "checked": false, "issues": [],
        });
        db.archive_rows(&[base.clone()]).unwrap();
        let mut filled = base.clone();
        filled["inbound"] = json!("IN88");
        db.archive_rows(&[filled]).unwrap();

        let tickets = db.archive_tickets().unwrap();
        let items = tickets[0]["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["inbound"], json!("IN88"));
    }

    /// 台账迁移：同票同原表行的旧重复行只留一条，且不回涨。
    #[test]
    fn migration_collapses_legacy_duplicate_records() {
        let db = Db::open_in_memory().unwrap();
        let legacy_row = json!({
            "syncId": "a", "waybill": "W5", "inbound": "IN5", "name": "泡棉", "qty": 5, "seq": 1,
            "sourceRow": 4, "checked": false, "customer": "", "destination": "深圳",
            "arrival": "", "issues": [], "cargo": {},
        });
        let mut copy = legacy_row.clone();
        copy["syncId"] = json!("b");
        copy["checked"] = json!(true);
        let state = json!({
            "mode": "ledger", "computerId": "pc", "revision": 1, "updatedAt": "2026-11-01",
            "operations": [], "records": [legacy_row, copy], "peers": {},
        });
        db.set_setting("packing_master_state_v1", &state.to_string()).unwrap();

        db.migrate_ledger_json().unwrap();

        assert_eq!(db.ledger_row_count().unwrap(), 1);
        let rows = db.ledger_rows().unwrap();
        assert_eq!(rows[0]["checked"], json!(true));
        assert_eq!(db.ledger_state().unwrap().mode, "ledger");
        assert!(db.get_setting("packing_master_state_v1.v3bak").unwrap().is_some());
    }
    /// 现行行已经有同货同量时，历史副本必须消失，否则同一批货会被算两遍。
    #[test]
    fn history_twin_of_live_row_is_dropped() {
        let db = Db::open_in_memory().unwrap();
        let mut live = row("W6", "IN6", "热风梳", 35, 121);
        live["destination"] = json!("休斯顿");
        let mut copy = live.clone();
        copy["syncId"] = json!("old");
        copy["issues"] = json!(["history_archive"]);
        copy["checked"] = json!(true);
        save(&db, vec![copy, live]);
        assert_eq!(db.ledger_row_count().unwrap(), 1);
        assert_eq!(db.ledger_rows().unwrap()[0]["checked"], json!(true));
    }

    /// 整票出货完成、云表已移除单号：历史副本没有现行行，必须留着，不能被截短。
    #[test]
    fn completed_ticket_history_is_kept() {
        let db = Db::open_in_memory().unwrap();
        let mut a = row("W7", "IN7", "护腰带", 2, 128);
        a["issues"] = json!(["history_archive"]);
        let mut b = row("W7", "IN7", "暖宫腰带", 3, 129);
        b["issues"] = json!(["history_archive"]);
        save(&db, vec![a, b]);
        assert_eq!(db.ledger_row_count().unwrap(), 2);
    }

    /// 原表重新解析后行号整体偏移，同一件货不能因此多存几份。
    #[test]
    fn archive_ignores_shifting_source_row() {
        let db = Db::open_in_memory().unwrap();
        let base = json!({
            "waybill": "6812705863", "inbound": "IB191261", "name": "热风梳-三合一",
            "customer": "杨总", "destination": "休斯顿", "qty": 35, "qtyRaw": "35",
            "arrival": "", "checked": true, "issues": [],
        });
        for seq in [121, 122, 124, 134, 174] {
            let mut row = base.clone();
            row["seq"] = json!(seq);
            db.archive_rows(&[row]).unwrap();
        }
        let items = db.archive_tickets().unwrap()[0]["items"].as_array().unwrap().clone();
        assert_eq!(items.len(), 1, "行号变了不算新的一件货");
        assert_eq!(items[0]["qty"], json!(35));
    }

    /// 拆批（20+15）是两行，不能合成一行；合并回 35 后旧的两行让位。
    #[test]
    fn archive_keeps_split_batches_and_collapses_when_merged() {
        let db = Db::open_in_memory().unwrap();
        let make = |qty: i64, seq: i64| {
            json!({
                "waybill": "W8", "inbound": "IN8", "name": "泡棉", "customer": "丙",
                "destination": "美西", "qty": qty, "qtyRaw": qty.to_string(), "seq": seq,
                "arrival": "", "checked": false, "issues": [],
            })
        };
        db.archive_rows(&[make(20, 3), make(15, 4)]).unwrap();
        assert_eq!(db.archive_tickets().unwrap()[0]["items"].as_array().unwrap().len(), 2);

        db.archive_rows(&[make(35, 3)]).unwrap();
        let items = db.archive_tickets().unwrap()[0]["items"].as_array().unwrap().clone();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["qty"], json!(35));
    }

    /// 数量被客户改动时就地覆盖，不留旧数量那一行。
    #[test]
    fn archive_quantity_edit_overwrites_in_place() {
        let db = Db::open_in_memory().unwrap();
        let make = |qty: i64, seq: i64| {
            json!({
                "waybill": "W9", "inbound": "IN9", "name": "杯刷", "customer": "丁",
                "destination": "美西", "qty": qty, "qtyRaw": qty.to_string(), "seq": seq,
                "arrival": "", "checked": false, "issues": [],
            })
        };
        db.archive_rows(&[make(5, 2)]).unwrap();
        db.archive_rows(&[make(6, 2)]).unwrap();
        let items = db.archive_tickets().unwrap()[0]["items"].as_array().unwrap().clone();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["qty"], json!(6));
    }
    /// 未分配时的历史快照，在现行行被分到某票之后必须让位，不能两边各算一遍。
    #[test]
    fn unassigned_history_snapshot_yields_to_assigned_live_row() {
        let db = Db::open_in_memory().unwrap();
        let mut snapshot = row("", "IB06726110", "电动刮痧仪", 40, 113);
        snapshot["issues"] = json!(["history_archive"]);
        let live = row("6812705863", "IB06726110", "电动刮痧仪", 40, 113);
        save(&db, vec![snapshot, live]);
        assert_eq!(db.ledger_row_count().unwrap(), 1);
        assert_eq!(db.ledger_rows().unwrap()[0]["waybill"], json!("6812705863"));
    }

    /// 已分票的副本只在同票时让步：另一票的历史不能被抹掉。
    #[test]
    fn history_of_another_ticket_is_never_dropped() {
        let db = Db::open_in_memory().unwrap();
        let mut other = row("6800000001", "IB0001", "洁面仪", 2, 5);
        other["issues"] = json!(["history_archive"]);
        let live = row("6812705863", "IB0001", "洁面仪", 2, 5);
        save(&db, vec![other, live]);
        assert_eq!(db.ledger_row_count().unwrap(), 2);
    }
    /// 手机重新分配 syncId 后，同一票同一原表行以新 ID 进来，不能撞唯一索引报错。
    /// （真实现场：手机同步整次退回旧数据，提示 UNIQUE constraint failed: ledger_row.waybill, ledger_row.source_row）
    #[test]
    fn reassigned_sync_id_does_not_hit_unique_index() {
        let db = Db::open_in_memory().unwrap();
        let mut old = row("W1", "IN1", "螺丝", 10, 1);
        old["sourceRow"] = json!(7);
        old["syncId"] = json!("old");
        save(&db, vec![old]);

        let mut fresh = row("W1", "IN1", "螺丝", 10, 1);
        fresh["sourceRow"] = json!(7);
        fresh["syncId"] = json!("new");
        fresh["checked"] = json!(true);
        save(&db, vec![fresh]);

        assert_eq!(db.ledger_row_count().unwrap(), 1);
        let rows = db.ledger_rows().unwrap();
        assert_eq!(rows[0]["syncId"], json!("new"));
        assert_eq!(rows[0]["checked"], json!(true));
    }

    /// 整份台账写入：这次没带的行不会留在库里。
    #[test]
    fn rows_missing_from_state_are_removed() {
        let db = Db::open_in_memory().unwrap();
        save(&db, vec![row("W1", "IN1", "螺丝", 10, 1), row("W1", "IN1", "垫片", 6, 2)]);
        assert_eq!(db.ledger_row_count().unwrap(), 2);
        save(&db, vec![row("W1", "IN1", "螺丝", 10, 1)]);
        assert_eq!(db.ledger_row_count().unwrap(), 1);
    }
}
