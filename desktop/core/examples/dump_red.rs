//! 排查工具：列出 .xls 里字体是红色的单元格（原表红色 = 已出货）。
//!
//! 用法：cargo run --example dump_red -- <path.xls> [关键词]
//!
//! 排查「某行没被认成已出」时先跑它：确认红字确实在文件里、落在哪几列。
//! 输出里的 `red` 是 0-based 列号。

use pdfimpose_core::xls::Book;

fn short(s: &str) -> String {
    let t = s.trim();
    if t.chars().count() > 12 {
        format!("{}…", t.chars().take(12).collect::<String>())
    } else {
        t.to_string()
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("需要 .xls 路径");
    let kw = args.next().unwrap_or_default();
    let book = Book::open(&path).expect("打开 .xls");
    let pick = book.pick_packing().expect("挑一张来货登记表");
    let s = book.sheet(&pick).expect("读表");
    let total_red: usize = s.red.iter().map(|r| r.iter().filter(|v| **v).count()).sum();
    println!("{path}");
    println!("  表={pick} 行={} 红格={total_red}", s.rows.len());
    let mut shown = 0;
    for r in 0..s.rows.len() {
        let reds: Vec<String> = s.red[r]
            .iter()
            .enumerate()
            .filter(|(_, v)| **v)
            .map(|(c, _)| c.to_string())
            .collect();
        let hit_kw = !kw.is_empty() && s.rows[r].join("|").contains(&kw);
        if reds.is_empty() && !hit_kw {
            continue;
        }
        shown += 1;
        let cells: Vec<String> = s.rows[r].iter().take(13).map(|c| format!("{:?}", short(c))).collect();
        println!("  r{r:>4} red=[{:<12}] {}", reds.join(","), cells.join(","));
        if shown > 40 {
            println!("  …（还有更多）");
            break;
        }
    }
}
