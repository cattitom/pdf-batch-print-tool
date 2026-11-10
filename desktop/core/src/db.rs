//! SQLite 数据层：生成历史 / 箱唛单据 / 用户偏好。
//!
//! 用 `PRAGMA user_version` 做最简迁移：后续加表/列时新增 `SCHEMA_Vn`
//! 并把版本号往上推即可，旧库升级自动执行增量脚本。

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

mod cargo;
mod ledger;
mod mobile_master;
pub use cargo::PackingCargoRecord;
pub use ledger::LedgerState;

pub const SCHEMA_VERSION: i64 = 4;

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS print_history (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at   TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
    output_name  TEXT NOT NULL,
    output_path  TEXT,
    output_pages INTEGER NOT NULL DEFAULT 0,
    paper        TEXT,
    sep_pages    INTEGER NOT NULL DEFAULT 0,
    params_json  TEXT NOT NULL DEFAULT '{}',
    files_json   TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX IF NOT EXISTS idx_print_history_created ON print_history(created_at DESC);

CREATE TABLE IF NOT EXISTS label_docs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
    inbound_no  TEXT NOT NULL,
    warehouse   TEXT NOT NULL DEFAULT '',
    client      TEXT NOT NULL DEFAULT '',
    shelf       TEXT NOT NULL DEFAULT '1',
    skus_json   TEXT NOT NULL DEFAULT '[]',
    total_boxes INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_label_docs_created ON label_docs(created_at DESC);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// v2：出货清单的「已录」勾选。按「单号 + 入库号 + 商品名」认行，
/// 数量改了不影响，重新解析同一份表时还能认回来。
const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS packing_checks (
    waybill    TEXT NOT NULL,
    inbound    TEXT NOT NULL,
    name       TEXT NOT NULL,
    checked    INTEGER NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
    PRIMARY KEY (waybill, inbound, name)
);
"#;

/// 一次拼版生成要写入历史的参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintHistoryInput {
    pub output_name: String,
    pub output_path: Option<String>,
    pub output_pages: i64,
    pub paper: Option<String>,
    pub sep_pages: i64,
    /// 其余参数（旋转 / 目标页数 / 裁剪等）的快照，前端自定义结构即可
    pub params: serde_json::Value,
    /// 输入文件清单快照 [{name, pages, target, rotate, crop}, ...]
    pub files: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct PrintHistoryEntry {
    #[ts(type = "number")]
    pub id: i64,
    pub created_at: String,
    pub output_name: String,
    pub output_path: Option<String>,
    #[ts(type = "number")]
    pub output_pages: i64,
    pub paper: Option<String>,
    #[ts(type = "number")]
    pub sep_pages: i64,
    #[ts(type = "unknown")]
    pub params: serde_json::Value,
    #[ts(type = "unknown")]
    pub files: serde_json::Value,
}

/// 一份箱唛单据（可整体复用上次录入）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelDocInput {
    pub inbound_no: String,
    pub warehouse: String,
    pub client: String,
    pub shelf: String,
    pub skus: serde_json::Value,
    pub total_boxes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct LabelDocEntry {
    #[ts(type = "number")]
    pub id: i64,
    pub created_at: String,
    pub inbound_no: String,
    pub warehouse: String,
    pub client: String,
    pub shelf: String,
    #[ts(type = "unknown")]
    pub skus: serde_json::Value,
    #[ts(type = "number")]
    pub total_boxes: i64,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    /// 打开（或创建）数据库文件；父目录不存在时自动创建。
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Db> {
        if let Some(parent) = path.as_ref().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let path = path.as_ref();
        log::info!("数据库初始化：path={}", path.display());
        let conn = Connection::open(path).map_err(|e| {
            log::error!("数据库初始化失败：step=open path={} error={e:?}", path.display());
            e
        })?;
        // 结构升级前先把旧库整体备份一份，迁移出错可原样回退。
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap_or(0);
        if version > 0 && version < SCHEMA_VERSION {
            let backup = path.with_extension(format!("v{version}.bak"));
            if let Err(e) = conn.execute("VACUUM INTO ?1", [backup.to_string_lossy().to_string()]) {
                log::warn!("数据库升级前备份失败（继续升级）：{e:?}");
            } else {
                log::info!("数据库升级前已备份：{}", backup.display());
            }
        }
        log::info!("数据库连接：readonly={:?}", conn.is_readonly("main"));
        Self::init(conn)
    }

    /// 内存库（测试 / 临时用）。
    pub fn open_in_memory() -> rusqlite::Result<Db> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Db> {
        conn.pragma_update(None, "journal_mode", "WAL").map_err(|e| {
            log::error!("数据库初始化失败：step=journal_mode_WAL error={e:?}");
            match e {
                rusqlite::Error::SqliteFailure(code, message) => rusqlite::Error::SqliteFailure(code, Some(format!("step=journal_mode_WAL extended_code={} {}", code.extended_code, message.unwrap_or_default()))),
                other => other,
            }
        })?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Db { conn };
        db.migrate().map_err(|e| {
            log::error!("数据库初始化失败：step=migrate error={e:?}");
            match e {
                rusqlite::Error::SqliteFailure(code, message) => rusqlite::Error::SqliteFailure(code, Some(format!("step=migrate extended_code={} {}", code.extended_code, message.unwrap_or_default()))),
                other => other,
            }
        })?;
        Ok(db)
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            self.conn.execute_batch(SCHEMA_V1)?;
            self.conn.pragma_update(None, "user_version", 1)?;
        }
        if version < 2 {
            self.conn.execute_batch(SCHEMA_V2)?;
            self.conn.pragma_update(None, "user_version", 2)?;
        }
        if version < 3 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(cargo::SCHEMA)?;
            tx.pragma_update(None, "user_version", 3)?;
            tx.commit()?;
        }
        if version < 4 {
            self.conn.execute_batch(ledger::SCHEMA)?;
            // 先导入再抬版本号：导入中途失败时版本仍是 3，下次启动会整体重试；
            // 旧 settings 键只在导入成功后才改名，重试不会丢数据。
            self.migrate_ledger_json()?;
            self.conn.pragma_update(None, "user_version", 4)?;
        }
        Ok(())
    }

    // ---------- 出货清单的「已录」勾选 ----------

    pub fn set_packing_check(
        &self,
        waybill: &str,
        inbound: &str,
        name: &str,
        checked: bool,
    ) -> rusqlite::Result<()> {
        self.set_packing_checks(&[(waybill.to_owned(), inbound.to_owned(), name.to_owned(), checked)])
    }

    /// 同一批勾选全部提交，任意一行失败则全部回滚。
    pub fn set_packing_checks(&self, changes: &[(String, String, String, bool)]) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO packing_checks(waybill, inbound, name, checked, updated_at)
                 VALUES (?1, ?2, ?3, ?4, datetime('now', 'localtime'))
                 ON CONFLICT(waybill, inbound, name) DO UPDATE SET
                   checked = excluded.checked, updated_at = excluded.updated_at")?;
            for (waybill, inbound, name, checked) in changes {
                stmt.execute(params![waybill, inbound, name, *checked as i64])?;
            }
        }
        tx.commit()
    }

    /// 全部勾选记录（waybill, inbound, name, checked）。行数不多，一次读完。
    pub fn all_packing_checks(&self) -> rusqlite::Result<Vec<(String, String, String, bool)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT waybill, inbound, name, checked FROM packing_checks")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)? != 0,
            ))
        })?;
        rows.collect()
    }

    pub fn clear_packing_checks(&self) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM packing_checks", [])?;
        Ok(())
    }

    // ---------- 生成历史 ----------

    pub fn add_print_history(&self, input: &PrintHistoryInput) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO print_history
             (output_name, output_path, output_pages, paper, sep_pages, params_json, files_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                input.output_name,
                input.output_path,
                input.output_pages,
                input.paper,
                input.sep_pages,
                input.params.to_string(),
                input.files.to_string(),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_print_history(
        &self,
        limit: u32,
        offset: u32,
    ) -> rusqlite::Result<Vec<PrintHistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, output_name, output_path, output_pages,
                    paper, sep_pages, params_json, files_json
             FROM print_history ORDER BY id DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit, offset], map_history)?;
        rows.collect()
    }

    pub fn delete_print_history(&self, id: i64) -> rusqlite::Result<usize> {
        self.conn
            .execute("DELETE FROM print_history WHERE id = ?1", params![id])
    }

    pub fn clear_print_history(&self) -> rusqlite::Result<usize> {
        self.conn.execute("DELETE FROM print_history", [])
    }

    // ---------- 箱唛单据 ----------

    pub fn save_label_doc(&self, input: &LabelDocInput) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO label_docs
             (inbound_no, warehouse, client, shelf, skus_json, total_boxes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                input.inbound_no,
                input.warehouse,
                input.client,
                input.shelf,
                input.skus.to_string(),
                input.total_boxes,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_label_docs(&self, limit: u32) -> rusqlite::Result<Vec<LabelDocEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, inbound_no, warehouse, client, shelf,
                    skus_json, total_boxes
             FROM label_docs ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], map_label_doc)?;
        rows.collect()
    }

    /// 最近一次单据（「复用上次录入」用）。
    pub fn latest_label_doc(&self) -> rusqlite::Result<Option<LabelDocEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, inbound_no, warehouse, client, shelf,
                    skus_json, total_boxes
             FROM label_docs ORDER BY id DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map([], map_label_doc)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    // ---------- 用户偏好 ----------

    pub fn get_setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM settings WHERE key = ?1")?;
        let mut rows = stmt.query_map(params![key], |r| r.get::<_, String>(0))?;
        match rows.next() {
            Some(v) => Ok(Some(v?)),
            None => Ok(None),
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// 批量读写偏好：直接拿整个 KV 表（条目很少，一次读完）。
    pub fn all_settings(&self) -> rusqlite::Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare("SELECT key, value FROM settings")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }
}

fn map_history(
    r: &rusqlite::Row<'_>,
) -> rusqlite::Result<PrintHistoryEntry> {
    let params_json: String = r.get(7)?;
    let files_json: String = r.get(8)?;
    Ok(PrintHistoryEntry {
        id: r.get(0)?,
        created_at: r.get(1)?,
        output_name: r.get(2)?,
        output_path: r.get(3)?,
        output_pages: r.get(4)?,
        paper: r.get(5)?,
        sep_pages: r.get(6)?,
        params: serde_json::from_str(&params_json)
            .unwrap_or(serde_json::Value::Null),
        files: serde_json::from_str(&files_json).unwrap_or(serde_json::Value::Null),
    })
}

fn map_label_doc(r: &rusqlite::Row<'_>) -> rusqlite::Result<LabelDocEntry> {
    let skus_json: String = r.get(6)?;
    Ok(LabelDocEntry {
        id: r.get(0)?,
        created_at: r.get(1)?,
        inbound_no: r.get(2)?,
        warehouse: r.get(3)?,
        client: r.get(4)?,
        shelf: r.get(5)?,
        skus: serde_json::from_str(&skus_json).unwrap_or(serde_json::Value::Null),
        total_boxes: r.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.get_setting("paper").unwrap(), None);
        db.set_setting("paper", "A4").unwrap();
        db.set_setting("copies", "3").unwrap();
        db.set_setting("paper", "A3").unwrap(); // 覆盖
        assert_eq!(db.get_setting("paper").unwrap().as_deref(), Some("A3"));
        let all = db.all_settings().unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn print_history_crud() {
        let db = Db::open_in_memory().unwrap();
        let input = PrintHistoryInput {
            output_name: "A_6_B_4.pdf".into(),
            output_path: Some(r"D:\out\A_6_B_4.pdf".into()),
            output_pages: 12,
            paper: Some("A4".into()),
            sep_pages: 1,
            params: serde_json::json!({"rotate": [0, 90]}),
            files: serde_json::json!([
                {"name": "A.pdf", "pages": 6, "target": 6, "rotate": 0},
                {"name": "B.pdf", "pages": 4, "target": 6, "rotate": 90}
            ]),
        };
        let id = db.add_print_history(&input).unwrap();
        assert!(id > 0);
        let list = db.list_print_history(10, 0).unwrap();
        assert_eq!(list.len(), 1);
        let e = &list[0];
        assert_eq!(e.output_name, "A_6_B_4.pdf");
        assert_eq!(e.output_pages, 12);
        assert_eq!(e.params["rotate"][1], 90);
        assert_eq!(e.files.as_array().unwrap().len(), 2);
        assert_eq!(db.delete_print_history(id).unwrap(), 1);
        assert!(db.list_print_history(10, 0).unwrap().is_empty());
    }

    #[test]
    fn label_docs_latest() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.latest_label_doc().unwrap().is_none());
        for no in ["IB001", "IB002"] {
            db.save_label_doc(&LabelDocInput {
                inbound_no: no.into(),
                warehouse: "NYC".into(),
                client: "江苏翰林".into(),
                shelf: "1".into(),
                skus: serde_json::json!([{"sku": "110-ZXD", "boxes": 80}]),
                total_boxes: 80,
            })
            .unwrap();
        }
        let latest = db.latest_label_doc().unwrap().unwrap();
        assert_eq!(latest.inbound_no, "IB002");
        assert_eq!(latest.skus[0]["sku"], "110-ZXD");
        assert_eq!(db.list_label_docs(10).unwrap().len(), 2);
    }

    #[test]
    fn migration_is_idempotent() {
        let path = std::env::temp_dir().join(format!(
            "pdfimpose_test_{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let db = Db::open(&path).unwrap();
            db.set_setting("k", "v").unwrap();
        }
        // 重新打开：迁移不应重复执行或报错
        {
            let db = Db::open(&path).unwrap();
            assert_eq!(db.get_setting("k").unwrap().as_deref(), Some("v"));
        }
        let _ = std::fs::remove_file(&path);
    }
    #[test]
    fn packing_batch_rolls_back_when_one_row_fails() {
        let db = Db::open_in_memory().unwrap();
        db.set_packing_check("W", "I", "existing", false).unwrap();
        db.conn.execute_batch("CREATE TRIGGER reject_bad BEFORE INSERT ON packing_checks
            WHEN NEW.name = 'bad' BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
        assert!(db.set_packing_checks(&[
            ("W".into(), "I".into(), "existing".into(), true),
            ("W".into(), "I".into(), "bad".into(), true),
        ]).is_err());
        let rows = db.all_packing_checks().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].3);
        db.set_packing_checks(&[
            ("W".into(), "I".into(), "existing".into(), true),
            ("W".into(), "I".into(), "good".into(), true),
        ]).unwrap();
        assert!(db.all_packing_checks().unwrap().iter().all(|r| r.3));
    }

    #[test]
    fn packing_batch_survives_database_reopen() {
        let path = std::env::temp_dir().join(format!("packing-batch-{}-{}.db", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        {
            let db = Db::open(&path).unwrap();
            db.set_packing_checks(&[
                ("W1".into(), "I1".into(), "商品一".into(), true),
                ("W2".into(), "I2".into(), "商品二".into(), false),
            ]).unwrap();
        }
        {
            let db = Db::open(&path).unwrap();
            let rows = db.all_packing_checks().unwrap();
            assert_eq!(rows.len(), 2);
            assert!(rows.iter().any(|r| r.0 == "W1" && r.3));
            assert!(rows.iter().any(|r| r.0 == "W2" && !r.3));
        }
        std::fs::remove_file(path).unwrap();
    }

}
