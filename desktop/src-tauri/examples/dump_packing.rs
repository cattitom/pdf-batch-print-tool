//! 生成前端预览用的 PackingSheetDto JSON（把真实 .xls 解析结果导出来当 mock 数据）。
//!
//! cargo run --example dump_packing -- <file.xls> [sheet] > packing-dto.json

use desktop_lib::commands::PackingSheetDto;

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_packing <file.xls> [sheet]");
    let sheet = std::env::args().nth(2);
    let (parsed, picked, names) =
        pdfimpose_core::parse_xls_file(&path, sheet.as_deref()).expect("parse");
    let dto: PackingSheetDto = parsed.into();
    let wrapper = serde_json::json!({
        "sheet": picked,
        "sheets": names,
        "bytes": std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        "changed": true,
        "fver": "preview",
        "path": path,
        "packing": dto,
    });
    println!("{}", serde_json::to_string(&wrapper).unwrap());
    eprintln!("items={}", wrapper["packing"]["items"].as_array().map(|a| a.len()).unwrap_or(0));
}
