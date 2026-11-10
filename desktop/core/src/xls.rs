//! Excel 读取适配层：文件格式、单元格值、合并范围和字体颜色全部交给 rxls。
//!
//! 保留 Book / Sheet 接口供云表格同步和排查工具使用；这里只做二维表转换、
//! 合并值展开，以及“明显红色字体 = 已出货”的业务判断。

use rxls::{Color, Workbook};

/// 已展开合并单元格的工作表。
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
    /// 与 rows 同形状；红色字体表示已出货。
    pub red: Vec<Vec<bool>>,
    /// 原表合并范围（零基、含终点），供数量归属判断使用。
    pub merges: Vec<(usize, usize, usize, usize)>,
}

impl Sheet {
    pub fn nonempty_rows(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.iter().any(|c| !c.is_empty()))
            .count()
    }

    pub fn rich_rows(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.iter().filter(|c| !c.is_empty()).count() >= 4)
            .count()
    }

    pub fn look_like_packing(&self) -> bool {
        const KEYS: [&str; 3] = ["收货仓", "来货日期", "入库号"];
        self.rows
            .iter()
            .take(6)
            .any(|row| row.iter().any(|c| KEYS.iter().any(|k| c.contains(k))))
    }
}

pub struct Book {
    workbook: Workbook,
}

impl Book {
    /// rxls 根据文件内容读取 xls / xlsx，不依赖 Excel、WPS 或 Python。
    pub fn open(path: &str) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("读取 {path} 失败：{e}"))?;
        let workbook = Workbook::open(&bytes).map_err(|e| format!("Excel 读取失败：{e}"))?;
        if workbook.is_partial() {
            return Err("Excel 数据超过读取库限制，无法完整读取，请拆分工作簿后重试".into());
        }
        Ok(Self { workbook })
    }

    pub fn names(&self) -> Vec<String> {
        self.workbook
            .sheets
            .iter()
            .filter(|s| s.is_worksheet)
            .map(|s| s.name.clone())
            .collect()
    }

    pub fn sheet(&self, name: &str) -> Result<Sheet, String> {
        let source = self
            .workbook
            .sheets
            .iter()
            .find(|s| s.is_worksheet && s.name.trim() == name.trim())
            .ok_or_else(|| {
                format!(
                    "Excel 里没有表「{name}」，可选：{}",
                    self.names().join("、")
                )
            })?;
        sheet_from_library(source)
    }

    /// 按工作簿顺序，从后往前优先挑有数据的来货登记表，再考虑空模板。
    pub fn pick_packing(&self) -> Option<String> {
        let mut template = None;
        for name in self.names().iter().rev() {
            if let Ok(sheet) = self.sheet(name) {
                if sheet.look_like_packing() {
                    if sheet.rich_rows() >= 3 {
                        return Some(name.clone());
                    }
                    if template.is_none() {
                        template = Some(name.clone());
                    }
                }
            }
        }
        template
    }

    pub fn sheet_names(&self) -> Vec<String> {
        self.names()
    }

    pub fn all_sheets(&self) -> Vec<Sheet> {
        self.names()
            .iter()
            .filter_map(|n| self.sheet(n).ok())
            .collect()
    }
}

/// 防止稀疏坐标或超大合并范围展开成不可用的二维表。
const MAX_GRID_CELLS: usize = 5_000_000;

fn sheet_from_library(source: &rxls::Sheet) -> Result<Sheet, String> {
    let mut height = 0usize;
    let mut width = 0usize;
    for cell in source.display_cells() {
        height = height.max(cell.row as usize + 1);
        width = width.max(cell.col as usize + 1);
    }
    for &(r1, c1, r2, c2) in source.merged_ranges() {
        if r1 > r2 || c1 > c2 {
            return Err("Excel 合并单元格范围无效".into());
        }
        height = height.max(r2 as usize + 1);
        width = width.max(c2 as usize + 1);
    }
    if height
        .checked_mul(width)
        .map_or(true, |n| n > MAX_GRID_CELLS)
    {
        return Err("Excel 工作表范围过大，请移除远离数据区域的单元格或拆分工作表".into());
    }
    let mut rows = vec![vec![String::new(); width]; height];
    let mut red = vec![vec![false; width]; height];
    for cell in source.display_cells() {
        let (r, c) = (cell.row as usize, cell.col as usize);
        // 文本保留原文，数值和公式使用库提供的显示值（公式不重算）。
        rows[r][c] = cell.formatted.to_owned();
        red[r][c] = source
            .resolved_cell_style(cell.row, cell.col)
            .and_then(|style| style.font)
            .and_then(|font| font.color)
            .is_some_and(is_red);
    }
    // 格式为空值的单元格仍可能带红字样式，不能丢失其业务含义。
    for &(r, c) in source.blank_cell_styles().keys() {
        if let Some(row) = red.get_mut(r as usize) {
            if let Some(value) = row.get_mut(c as usize) {
                *value = source
                    .resolved_cell_style(r, c)
                    .and_then(|style| style.font)
                    .and_then(|font| font.color)
                    .is_some_and(is_red);
            }
        }
    }
    let merges: Vec<_> = source
        .merged_ranges()
        .iter()
        .map(|&(r1, c1, r2, c2)| (r1 as usize, c1 as usize, r2 as usize, c2 as usize))
        .collect();
    crate::parse_packing::apply_merges(&mut rows, &merges);
    for &(r1, c1, r2, c2) in &merges {
        if !rows[r1][c1].is_empty() && red[r1][c1] {
            for row in &mut red[r1..=r2] {
                row[c1..=c2].fill(true);
            }
        }
    }
    Ok(Sheet {
        name: source.name.clone(),
        rows,
        red,
        merges,
    })
}

fn is_red(color: Color) -> bool {
    let [r, g, b] = color.as_rgb();
    r >= 150 && g <= 90 && b <= 90
}

/// 文件是不是传统 xls（仅检测文件标识，格式解析交给库）。
pub fn is_xls(path: &str) -> bool {
    use std::io::Read;
    std::fs::File::open(path).ok().is_some_and(|mut file| {
        let mut head = [0u8; 8];
        file.read_exact(&mut head).is_ok()
            && head == [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_unbounded_sparse_sheet_expansion() {
        let mut sheet = rxls::Sheet::new("稀疏表");
        sheet.write(1_048_575, 16_383, "末尾单元格");
        assert!(sheet_from_library(&sheet).is_err());
    }
}
