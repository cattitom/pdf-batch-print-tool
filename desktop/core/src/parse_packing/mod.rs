//! 装箱单解析：把「粘贴的表格文本 / xlsx / csv / xls」归一化成二维表，再按列映射取出出货待办。
//!
//! 三种输入都先落到 `Vec<Vec<String>>`，之后共用同一套规则：
//! 表头识别 → 列映射（没有表头时按截图模板的列序）→ 合并单元格展开 →
//! 单号 / 目的地识别 → 逐行校验；分批出货的同名同号记录分别保留。
//!
//! 与 UI 无关，可独立 `cargo test`。
//!
//! 模块划分：`text`（文本 / csv → 二维表）、`table`（表头识别与列映射，核心规则）、
//! `xlsx`（Excel 库适配与合并值展开）、`tests`（回归用例）。

use std::fmt;

mod table;
mod text;
#[cfg(test)]
mod tests;
mod xlsx;

pub(crate) use self::table::*;
pub(crate) use self::text::*;
pub(crate) use self::xlsx::*;

/// 一条待办，对应表里一行货物。
#[derive(Debug, Clone, Default)]
pub struct PackingItem {
    pub seq: u32,
    /// Excel 原表行号（一基）；文本/OCR 无 Excel 行号。
    pub source_row: Option<u32>,
    /// 数量合并范围；同一组只在第一条货物上计入箱数。
    pub quantity_group: Option<String>,
    pub name: String,
    /// 解析出的整数数量；0 表示这一行没读到数量
    pub qty: i64,
    /// 数量原文，解析失败时给界面显示
    pub qty_raw: String,
    /// 原表货物规格、尺寸重量及备注，保留单元格原文。
    pub cargo: std::collections::BTreeMap<String, String>,
    pub inbound: String,
    pub destination: String,
    /// 原表的「到货情况」原样带过来（到货 / 未到货 / 部分到货），界面只读展示
    pub arrival: String,
    pub waybill: String,
    /// 客户 / 供应商名称（表里通常只在段首写一次，下面沿用）
    pub customer: String,
    pub checked: bool,
    /// 校验提示码：name / qty / inbound / inbound_sci / mark_text / dup_mark
    pub issues: Vec<String>,
}

impl PackingItem {
    pub fn has_issue(&self, code: &str) -> bool {
        self.issues.iter().any(|i| i == code)
    }
}

/// 一次解析的结果。
#[derive(Debug, Clone, Default)]
pub struct PackingSheet {
    pub items: Vec<PackingItem>,
    pub destination: String,
    pub waybill: String,
    pub total_qty: i64,
    /// 图上自带的合计（「现 168 件」），用来对账
    pub expected_qty: Option<i64>,
    /// 列映射说明，给界面展示
    pub mapping: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PackingError {
    code: &'static str,
    message: String,
}

impl PackingError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for PackingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PackingError {}

// ---------- 入口 ----------

/// 解析粘贴进来的表格文本（Excel 复制 = 制表符分隔）。

pub fn parse_text(text: &str) -> PackingSheet {
    let mode = if text.contains('\t') { TableMode::Tab } else { TableMode::Spaces };
    parse_table(rows_from_text(text, mode), false)
}

/// 解析文件：xlsx / xls / csv / txt。
pub fn parse_file(path: &str) -> Result<PackingSheet, PackingError> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        // xlsx 里的合并单元格已经被 apply_merges 按真实 mergeCells 铺开
        "xlsx" | "xlsm" => {
            let sheet = read_xlsx(path)?;
            Ok(parse_excel_sheet(sheet))
        },
        // .xls 由 rxls 读取，自动挑最新一张来货登记表
        "xls" => parse_xls_file(path, None).map(|(sheet, _, _)| sheet),
        "csv" => Ok(parse_table(rows_from_text(&read_text_file(path)?, TableMode::Csv), false)),
        "txt" | "tsv" | "md" => {
            let text = read_text_file(path)?;
            let mode = if text.contains('\t') { TableMode::Tab } else { TableMode::Spaces };
            Ok(parse_table(rows_from_text(&text, mode), false))
        }
        "" => Err(PackingError::new("unsupported", "这个文件没有扩展名，认不出格式")),
        other => Err(PackingError::new(
            "unsupported",
            format!("暂不支持 .{other} ：拖 xlsx / xls / csv / txt，或直接在 Excel 里全选复制粘贴"),
        )),
    }
}

/// 解析 .xls（BIFF8，WPS 云表格导出的就是这种）。
///
/// `sheet` 传 None 表示自动挑最新一张「来货登记」表（表按月排，最后一张最新）；
/// 返回（解析结果, 选中的表名, 全部表名）——表名列表给界面的换表下拉用。
pub fn parse_xls_file(
    path: &str,
    sheet: Option<&str>,
) -> Result<(PackingSheet, String, Vec<String>), PackingError> {
    let book = crate::xls::Book::open(path).map_err(|e| PackingError::new("xls", e))?;
    let names = book.names();
    if names.is_empty() {
        return Err(PackingError::new("xls", "这个 .xls 里没有工作表"));
    }
    let picked = match sheet.map(str::trim).filter(|s| !s.is_empty()) {
        Some(want) => names.iter().find(|n| n.trim() == want).cloned().ok_or_else(|| {
            PackingError::new(
                "xls",
                format!("云表格里没有「{want}」这张表，可选：{}", names.join("、")),
            )
        })?,
        None => book.pick_packing().ok_or_else(|| {
            PackingError::new(
                "xls",
                "没有一张表像来货登记（前几行里找不到 收货仓 / 来货日期 / 入库号 表头）",
            )
        })?,
    };
    let data = book
        .sheet(&picked)
        .map_err(|e| PackingError::new("xls", e))?;
    // 合并单元格在 xls::Book 里已经铺开了（等价于 xlsx 走的 apply_merges）；
    // 红字矩阵一起传下去：原表里红色字体 = 已出货
    Ok((parse_excel_sheet(data), picked, names))
}

/// 直接解析已经切好的二维表（OCR、外部导入用）。
pub fn parse_grid(rows: Vec<Vec<String>>) -> PackingSheet {
    parse_table(rows, false)
}

