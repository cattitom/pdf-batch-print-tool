//! 迁移演练：读迁移前的 settings JSON 统计，跑 Db::open 触���迁移，再统计新表。
//! 用法：cargo run --example ledger_migrate_probe -- <db 副本路径> [单号]
use pdfimpose_core::{rusqlite, Db};
use serde_json::Value;

fn qty_of(rows: &[Value]) -> i64 {
    rows.iter().map(|r| r["qty"].as_i64().unwrap_or(0)).sum()
}

fn main() {
    let path = std::env::args().nth(1).expect("需要数据库副本路径");
    let focus = std::env::args().nth(2).unwrap_or_default();

    let conn = rusqlite::Connection::open(&path).expect("open");
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap_or(0);
    let archives: Vec<(String, String)> = {
        let mut st = conn
            .prepare("SELECT key,value FROM settings WHERE key LIKE 'packing_archive_v1:%'")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    let master: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key='packing_master_state_v1'", [], |r| r.get(0))
        .ok();
    let records: Vec<Value> = master
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v["records"].as_array().cloned())
        .unwrap_or_default();
    println!("迁移前 schema_version={version}");
    println!("迁移前 台账记录={} 件数={}", records.len(), qty_of(&records));
    println!("迁移前 存档键={} 个", archives.len());
    for (key, _) in &archives {
        println!("  存档键 {key}");
    }
    if !focus.is_empty() {
        let hit: Vec<&Value> = records.iter().filter(|r| r["waybill"] == focus.as_str()).collect();
        println!("迁移前 单号 {focus}: {} 行 / {} 件", hit.len(), qty_of(&hit.iter().map(|v| (*v).clone()).collect::<Vec<_>>()));
    }
    drop(conn);

    let db = Db::open(&path).expect("迁移失败");
    println!("迁移后 mode={:?}", db.ledger_mode().unwrap());
    println!("迁移后 台账行数={}", db.ledger_row_count().unwrap());
    let rows = db.ledger_rows().unwrap();
    println!("迁移后 台账件数={}", qty_of(&rows));
    if !focus.is_empty() {
        let hit: Vec<&Value> = rows.iter().filter(|r| r["waybill"] == focus.as_str()).collect();
        println!("迁移后 单号 {focus}: {} 行 / {} 件", hit.len(), qty_of(&hit.iter().map(|v| (*v).clone()).collect::<Vec<_>>()));
    }
    let tickets = db.archive_tickets().unwrap();
    println!("迁移后 存档票数={}", tickets.len());
    for t in &tickets {
        let items = t["items"].as_array().cloned().unwrap_or_default();
        println!("  票 {} 明细 {} 行 / {} 件", t["waybill"].as_str().unwrap_or(""), items.len(), qty_of(&items));
    }
    let kept: i64 = db
        .all_settings()
        .unwrap()
        .iter()
        .filter(|(k, _)| k.ends_with(".v3bak"))
        .count() as i64;
    println!("迁移后 旧键留底={kept} 个");
}
