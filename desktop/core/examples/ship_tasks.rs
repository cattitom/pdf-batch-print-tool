//! 核对一批「要出货」的入库号：在原表里逐条找出来，打印客户/品名/箱数并合计。
//!
//! 用法：cargo run --example ship_tasks -- <path.xls> <入库号> [入库号 ...]
//! 这是「截图 → 出货任务」功能的命令行原型：截图里认出的入库号先过这一遍，
//! 对不上的当场暴露（不会静默漏货）。

use pdfimpose_core::parse_xls_file;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("需要 .xls 路径");
    let codes: Vec<String> = args.collect();
    let (sheet, name, _) = parse_xls_file(&path, None).expect("解析");
    println!("表={name} 共 {} 行", sheet.items.len());
    // 只看单号分布：用于造测试数据（一张单号一个任务票）
    // 列出某一张票的所有行（造测试数据用）
    if let Some(pos) = codes.iter().position(|c| c == "--rows") {
        let want = codes.get(pos + 1).cloned().unwrap_or_default();
        for it in sheet.items.iter().filter(|i| i.waybill.trim() == want) {
            println!(
                "  {} | {} | {} 箱 | {} | {}",
                it.inbound, it.name, it.qty, it.customer, it.arrival
            );
        }
        return;
    }
    if codes.iter().any(|c| c == "--waybills") {
        let mut seen: Vec<String> = Vec::new();
        for it in &sheet.items {
            let wb = it.waybill.trim();
            if wb.is_empty() || seen.contains(&wb.to_string()) {
                continue;
            }
            seen.push(wb.to_string());
            let n = sheet.items.iter().filter(|x| x.waybill.trim() == wb).count();
            println!("  {} → {} 行", wb, n);
            if seen.len() >= 12 {
                break;
            }
        }
        return;
    }
    let mut total = 0;
    let mut missing = Vec::new();
    for code in &codes {
        let hit = sheet
            .items
            .iter()
            .find(|i| i.inbound.trim().eq_ignore_ascii_case(code.trim()));
        match hit {
            Some(it) => {
                total += it.qty;
                println!(
                    "  {} | {} | {} 箱 | 客户 {} | 单号 {}",
                    it.inbound, it.name, it.qty, it.customer,
                    if it.waybill.is_empty() { "-" } else { &it.waybill }
                );
            }
            None => missing.push(code.clone()),
        }
    }
    if !missing.is_empty() {
        println!("  对不上：{}", missing.join("、"));
    }
    println!("合计 {} 箱 / {} 款", total, codes.len() - missing.len());
}
