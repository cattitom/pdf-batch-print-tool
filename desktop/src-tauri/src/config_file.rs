//! 用户偏好独立于业务数据库。JSON 只包含可配置项，不导出凭证、缓存或存档。
use std::{collections::BTreeMap,path::Path};
use serde::{Deserialize,Serialize};
use serde_json::Value;
pub const KEYS:&[(&str,bool)]=&[("packing_auto_open",true),("kdocs_auto_sync",false),("show_label_tab",false),("show_boxplan_tab",false),("show_product_label_tab",true)];
pub fn is_preference(key:&str)->bool{KEYS.iter().any(|(k,_)|*k==key)}
#[derive(Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Config{schema_version:u32,preferences:BTreeMap<String,Value>}
fn read(path:&Path)->Result<Config,String>{
 let bytes=std::fs::read(path).map_err(|e|format!("读取配置文件失败（{}）：{e}",path.display()))?;
 let mut data:Config=serde_json::from_slice(&bytes).map_err(|e|format!("配置文件 JSON 格式错误（{}）：{e}；原文件已保留，请修正后重试",path.display()))?;
 if data.schema_version!=1{return Err("不支持的配置文件版本，原文件已保留".into());}
 for(key,value)in &data.preferences{if !is_preference(key)||!value.is_boolean(){return Err(format!("配置项 {key} 无效：仅支持已知设置，值须为 true 或 false；原文件已保留"));}}
 for(key,default)in KEYS{data.preferences.entry((*key).into()).or_insert(Value::Bool(*default));}
 Ok(data)
}
fn write(path:&Path,data:&Config)->Result<(),String>{
 let temp=path.with_extension(format!("json.{}.tmp",std::process::id()));
 let bytes=serde_json::to_vec_pretty(data).map_err(|e|e.to_string())?;
 let result=(||{use std::io::Write;let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;file.write_all(&bytes)?;file.sync_all()?;drop(file);std::fs::rename(&temp,path)})();
 if let Err(e)=result{let _=std::fs::remove_file(&temp);return Err(format!("配置保存失败，原配置未改动：{e}"));}Ok(())
}
pub fn ensure(path:&Path,legacy:&[(String,String)])->Result<(),String>{
 if path.exists(){return Ok(());}
 let preferences=KEYS.iter().map(|(key,default)|{
  let value=legacy.iter().find(|(k,_)|k==key).and_then(|(_,v)|match v.as_str(){"1"=>Some(true),"0"=>Some(false),_=>None}).unwrap_or(*default);
  ((*key).into(),Value::Bool(value))
 }).collect();
 write(path,&Config{schema_version:1,preferences})
}
pub fn values(path:&Path)->Result<BTreeMap<String,String>,String>{Ok(read(path)?.preferences.into_iter().map(|(k,v)|(k,if v.as_bool()==Some(true){"1".into()}else{"0".into()})).collect())}
pub fn set(path:&Path,key:&str,value:&str)->Result<(),String>{
 if !is_preference(key)||!matches!(value,"0"|"1"){return Err("配置开关只能保存 0 或 1".into());}
 let mut data=read(path)?;data.preferences.insert(key.into(),Value::Bool(value=="1"));write(path,&data)
}
#[cfg(test)]
mod tests{
 use super::*;
 fn path()->std::path::PathBuf{let dir=std::env::temp_dir().join(format!("shipping-config-{}-{}",std::process::id(),crate::mobile::new_token().unwrap()));std::fs::create_dir_all(&dir).unwrap();dir.join("config.json")}
 #[test]fn migrates_only_preferences_and_file_remains_authoritative(){let p=path();ensure(&p,&[("kdocs_auto_sync".into(),"1".into()),("mobile_token".into(),"secret".into()),("packing_archive_v1:W".into(),"archive".into())]).unwrap();assert_eq!(values(&p).unwrap()["kdocs_auto_sync"],"1");let text=std::fs::read_to_string(&p).unwrap();assert!(!text.contains("secret"));assert!(!text.contains("archive"));set(&p,"kdocs_auto_sync","0").unwrap();ensure(&p,&[("kdocs_auto_sync".into(),"1".into())]).unwrap();assert_eq!(values(&p).unwrap()["kdocs_auto_sync"],"0");}
 #[test]fn malformed_file_cannot_be_overwritten(){let p=path();std::fs::write(&p,"{broken").unwrap();ensure(&p,&[]).unwrap();assert!(set(&p,"packing_auto_open","1").is_err());assert_eq!(std::fs::read_to_string(&p).unwrap(),"{broken");}
 #[test]fn unknown_keys_and_non_booleans_rejected_and_partial_defaults_work(){let p=path();std::fs::write(&p,r#"{"schemaVersion":1,"preferences":{"packing_auto_open":false}}"#).unwrap();assert_eq!(values(&p).unwrap()["packing_auto_open"],"0");assert_eq!(values(&p).unwrap()["show_product_label_tab"],"1");let raw=r#"{"schemaVersion":1,"preferences":{"mobile_token":true}}"#;std::fs::write(&p,raw).unwrap();assert!(set(&p,"packing_auto_open","1").is_err());assert_eq!(std::fs::read_to_string(&p).unwrap(),raw);}
}
