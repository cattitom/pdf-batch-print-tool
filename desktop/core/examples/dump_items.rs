//! 排查工具：解析 .xls 后每行的「已出」判定来自哪里。
//!
//! 用法：cargo run --example dump_items -- <path.xls> [关键词]
//!
//! 「已出」= 原表「产品名称 → 入库号」这几列字体红色，或老表的「标记」列写了 1。

use pdfimpose_core::parse_xls_file;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("需要 .xls 路径");
    let kw = args.next().unwrap_or_default();
    let (sheet, name, _) = parse_xls_file(&path, None).expect("解析");
    let done = sheet.items.iter().filter(|i| i.checked).count();
    println!(
        "表={name} 行={} 已出={done} 合计={} 映射={}",
        sheet.items.len(),
        sheet.total_qty,
        sheet.mapping
    );
    for it in &sheet.items {
        if !kw.is_empty() && !it.name.contains(&kw) {
            continue;
        }
        println!(
            "  {} {:>18} {:>4} 件 inbound={:?} arrival={:?} issues={:?}",
            if it.checked { "已出" } else { "未出" },
            it.name,
            it.qty,
            it.inbound,
            it.arrival,
            it.issues
        );
    }
}
