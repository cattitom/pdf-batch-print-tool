use super::Db;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS packing_cargo (
 record_key TEXT PRIMARY KEY, customer TEXT NOT NULL, inbound TEXT NOT NULL,
 name TEXT NOT NULL, destination TEXT NOT NULL, waybill TEXT NOT NULL,
 seq INTEGER NOT NULL, qty INTEGER NOT NULL, cargo_json TEXT NOT NULL,
 source TEXT NOT NULL, updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime'))
);
CREATE INDEX IF NOT EXISTS idx_packing_cargo_inbound ON packing_cargo(inbound);
";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackingCargoRecord {
    pub customer: String, pub inbound: String, pub name: String,
    pub destination: String, pub waybill: String, pub seq: u32, pub qty: i64,
    pub cargo: BTreeMap<String, String>, pub source: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> PackingCargoRecord {
        PackingCargoRecord { customer:"陈东龙".into(),inbound:"IB015261107RS".into(),name:"面部刮痧".into(),destination:"休斯顿".into(),
            waybill:"W".into(),seq:1,qty:10,source:"WPS · 11月2周".into(),cargo:[("length".into(),"53".into()),("actualWeight".into(),"17.6".into())].into() }
    }
    #[test]
    fn migrates_existing_checks_and_manual_notes_without_loss() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(super::super::SCHEMA_V1).unwrap();
        conn.execute_batch(super::super::SCHEMA_V2).unwrap();
        conn.pragma_update(None,"user_version",2).unwrap();
        conn.execute("INSERT INTO settings(key,value) VALUES('manual-customer-message','keep')",[]).unwrap();
        conn.execute("INSERT INTO packing_checks(waybill,inbound,name,checked) VALUES('W','I','N',1)",[]).unwrap();
        let db = Db::init(conn).unwrap();
        assert_eq!(db.get_setting("manual-customer-message").unwrap().as_deref(),Some("keep"));
        assert!(db.all_packing_checks().unwrap()[0].3);
        db.cache_packing_cargo(&[sample()]).unwrap();
        assert_eq!(db.packing_cargo(&sample()).unwrap().unwrap().cargo["length"],"53");
    }
    #[test]
    fn keeps_different_source_rows_and_manual_revisions_separate() {
        let db = Db::open_in_memory().unwrap();
        let a = sample();
        let mut b = a.clone(); b.seq=2; b.cargo.insert("length".into(),"60".into());
        db.cache_packing_cargo(&[a.clone(),b.clone()]).unwrap();
        assert_eq!(db.packing_cargo(&a).unwrap().unwrap().cargo["length"],"53");
        assert_eq!(db.packing_cargo(&b).unwrap().unwrap().cargo["length"],"60");
        db.set_setting("packing_customer_message_v1:manual","user override").unwrap();
        let mut updated = a.clone(); updated.cargo.insert("actualWeight".into(),"18".into());
        db.cache_packing_cargo(&[updated]).unwrap();
        assert_eq!(db.packing_cargo(&a).unwrap().unwrap().cargo["actualWeight"],"18");
        assert_eq!(db.get_setting("packing_customer_message_v1:manual").unwrap().as_deref(),Some("user override"));
    }
}
impl PackingCargoRecord {
    fn key(&self) -> String {
        serde_json::to_string(&(self.customer.trim(), self.inbound.trim(), self.name.trim(),
            self.destination.trim(), self.waybill.trim(), self.seq)).expect("货物键序列化")
    }
}
impl Db {
    /// 同步保存原表快照；用户修订仍独立保存在 settings，不会被云表覆盖。
    pub fn cache_packing_cargo(&self, records: &[PackingCargoRecord]) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut statement = tx.prepare("INSERT INTO packing_cargo
              (record_key,customer,inbound,name,destination,waybill,seq,qty,cargo_json,source)
              VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
              ON CONFLICT(record_key) DO UPDATE SET qty=excluded.qty,cargo_json=excluded.cargo_json,
              source=excluded.source,updated_at=datetime('now','localtime')")?;
            for row in records {
                // 原表没有这个列时保留已有资料；明确存在但为空的列才覆盖为空。
                let previous: Option<String> = tx.query_row("SELECT cargo_json FROM packing_cargo WHERE record_key=?1",
                    [row.key()], |r| r.get(0)).optional()?;
                let mut merged: BTreeMap<String,String> = match previous {
                    Some(raw) => serde_json::from_str(&raw).map_err(|e| rusqlite::Error::FromSqlConversionFailure(0,rusqlite::types::Type::Text,Box::new(e)))?,
                    None => BTreeMap::new(),
                };
                merged.extend(row.cargo.clone());
                let cargo = serde_json::to_string(&merged).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                statement.execute(params![row.key(),row.customer,row.inbound,row.name,row.destination,row.waybill,row.seq,row.qty,cargo,row.source])?;
            }
        }
        tx.commit()
    }
    pub fn packing_cargo(&self, identity: &PackingCargoRecord) -> rusqlite::Result<Option<PackingCargoRecord>> {
        self.conn.query_row("SELECT customer,inbound,name,destination,waybill,seq,qty,cargo_json,source FROM packing_cargo WHERE record_key=?1",
            [identity.key()], |r| {
                let raw: String = r.get(7)?;
                let cargo = serde_json::from_str(&raw).map_err(|e| rusqlite::Error::FromSqlConversionFailure(7,rusqlite::types::Type::Text,Box::new(e)))?;
                Ok(PackingCargoRecord { customer:r.get(0)?,inbound:r.get(1)?,name:r.get(2)?,destination:r.get(3)?,
                    waybill:r.get(4)?,seq:r.get(5)?,qty:r.get(6)?,cargo,source:r.get(8)? })
            }).optional()
    }
}
