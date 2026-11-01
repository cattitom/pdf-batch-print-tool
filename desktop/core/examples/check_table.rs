//! 通用核对：读任意受支持的表格（xlsx / xls / csv / txt），逐行打印解析结果。
//!
//! 用法：cargo run --example check_table -- <path> [关键词]
//! 造测试数据后先跑一遍，确认表头映射、单号、入库号、到货情况都认对了。

use pdfimpose_core::parse_file;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("需要表格路径");
    let kw = args.next().unwrap_or_default();
    let sheet = parse_file(&path).expect("解析");
    println!("映射：{}", sheet.mapping);
    println!("行={} 合计={} 单号={:?}", sheet.items.len(), sheet.total_qty, sheet.waybill);
    println!("警告：{:?}", sheet.warnings);
    for it in &sheet.items {
        if !kw.is_empty() && !it.name.contains(&kw) && !it.inbound.contains(&kw) {
            continue;
        }
        println!(
            "  {} | {} 箱 | 客户 {} | 单号 {:?} | 到货 {:?}",
            it.inbound, it.qty, it.customer, it.waybill, it.arrival
        );
    }
}
