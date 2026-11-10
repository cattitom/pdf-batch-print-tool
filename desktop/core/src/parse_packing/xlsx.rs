// Excel 读取使用 rxls；此处保留文本编码处理与合并值展开的业务适配。
use super::PackingError;

pub(crate) fn read_text_file(path: &str) -> Result<String, PackingError> {
    let bytes = std::fs::read(path)
        .map_err(|e| PackingError::new("io", format!("读取 {path} 失败：{e}")))?;
    if let Ok(s) = std::str::from_utf8(&bytes) {
        return Ok(s.to_owned());
    }
    let (decoded, _, _) = encoding_rs::GBK.decode(&bytes);
    Ok(decoded.into_owned())
}

pub(crate) fn read_xlsx(path: &str) -> Result<crate::xls::Sheet, PackingError> {
    let book = crate::xls::Book::open(path).map_err(|e| PackingError::new("xlsx", e))?;
    let name = book
        .names()
        .into_iter()
        .next()
        .ok_or_else(|| PackingError::new("xlsx", "xlsx 里没找到工作表"))?;
    book.sheet(&name).map_err(|e| PackingError::new("xlsx", e))
}

/// 合并单元格：把左上角的值铺满整个范围（Excel 里被合并的格子是空的）。
pub(crate) fn apply_merges(grid: &mut Vec<Vec<String>>, merges: &[(usize, usize, usize, usize)]) {
    for &(r1, c1, r2, c2) in merges {
        let value = get_cell(grid, r1, c1);
        if value.is_empty() {
            continue;
        }
        for r in r1..=r2 {
            for c in c1..=c2 {
                set_cell(grid, r, c, value.clone());
            }
        }
    }
}

pub(crate) fn get_cell(grid: &[Vec<String>], r: usize, c: usize) -> String {
    grid.get(r)
        .and_then(|row| row.get(c))
        .cloned()
        .unwrap_or_default()
}

pub(crate) fn set_cell(grid: &mut Vec<Vec<String>>, r: usize, c: usize, v: String) {
    while grid.len() <= r {
        grid.push(Vec::new());
    }
    let row = &mut grid[r];
    while row.len() <= c {
        row.push(String::new());
    }
    row[c] = v;
}
