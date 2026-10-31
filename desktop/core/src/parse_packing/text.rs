// 文本 / csv → 二维表（`rows_from_text` 及其辅助函数）

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableMode {
    Tab,
    Csv,
    Spaces,
}

pub(crate) fn rows_from_text(text: &str, mode: TableMode) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let cells: Vec<String> = match mode {
            TableMode::Tab => line.split('\t').map(clean).collect(),
            TableMode::Csv => split_csv(line),
            TableMode::Spaces => split_wide_spaces(line),
        };
        if cells.iter().all(|c| c.is_empty()) {
            continue;
        }
        out.push(cells);
    }
    out
}

pub(crate) fn clean(s: &str) -> String {
    s.trim().replace('\u{3000}', " ").trim().to_string()
}

/// 两个及以上连续空格算分隔符，单个空格保留（商品名里本来就有空格）。
pub(crate) fn split_wide_spaces(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut run = 0usize;
    for ch in line.chars() {
        if ch == ' ' || ch == '\u{3000}' {
            run += 1;
            if run == 2 {
                push_cell(&mut out, &mut cur);
            } else if run == 1 {
                cur.push(' ');
            }
        } else {
            run = 0;
            cur.push(ch);
        }
    }
    push_cell(&mut out, &mut cur);
    out
}

pub(crate) fn push_cell(out: &mut Vec<String>, cur: &mut String) {
    let v = cur.trim().to_string();
    if !v.is_empty() {
        out.push(v);
    }
    cur.clear();
}

/// 带引号的 CSV 拆分（Excel 导出的 csv 会给含逗号的单元格加引号）。
pub(crate) fn split_csv(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if quoted && chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    quoted = !quoted;
                }
            }
            ',' if !quoted => {
                out.push(clean(&cur));
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    out.push(clean(&cur));
    out
}
