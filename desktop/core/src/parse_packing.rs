//! 装箱单解析：把「粘贴的表格文本 / xlsx / csv」归一化成二维表，再按列映射取出录货待办。
//!
//! 三种输入都先落到 `Vec<Vec<String>>`，之后共用同一套规则：
//! 表头识别 → 列映射（没有表头时按截图模板的列序）→ 合并单元格展开 →
//! 单号 / 目的地识别 → 逐行校验 → 去重。
//!
//! 与 UI 无关，可独立 `cargo test`。

use std::collections::BTreeMap;
use std::fmt;
use std::io::Read as _;

// ---------- 对外数据结构 ----------

/// 一条待办，对应表里一行货物。
#[derive(Debug, Clone, Default)]
pub struct PackingItem {
    pub seq: u32,
    pub name: String,
    /// 解析出的整数数量；0 表示这一行没读到数量
    pub qty: i64,
    /// 数量原文，解析失败时给界面显示
    pub qty_raw: String,
    pub inbound: String,
    pub destination: String,
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
    parse_table(rows_from_text(text, mode))
}

/// 解析文件：xlsx / csv / txt。
pub fn parse_file(path: &str) -> Result<PackingSheet, PackingError> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "xlsx" | "xlsm" => Ok(parse_table(read_xlsx(path)?)),
        "csv" => Ok(parse_table(rows_from_text(&read_text_file(path)?, TableMode::Csv))),
        "txt" | "tsv" | "md" => {
            let text = read_text_file(path)?;
            let mode = if text.contains('\t') { TableMode::Tab } else { TableMode::Spaces };
            Ok(parse_table(rows_from_text(&text, mode)))
        }
        "" => Err(PackingError::new("unsupported", "这个文件没有扩展名，认不出格式")),
        other => Err(PackingError::new(
            "unsupported",
            format!("暂不支持 .{other} ：拖 xlsx / csv / txt，或直接在 Excel 里全选复制粘贴"),
        )),
    }
}

/// 直接解析已经切好的二维表（OCR、外部导入用）。
pub fn parse_grid(rows: Vec<Vec<String>>) -> PackingSheet {
    parse_table(rows)
}

// ---------- 文本 → 二维表 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableMode {
    Tab,
    Csv,
    Spaces,
}

fn rows_from_text(text: &str, mode: TableMode) -> Vec<Vec<String>> {
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

fn clean(s: &str) -> String {
    s.trim().replace('\u{3000}', " ").trim().to_string()
}

/// 两个及以上连续空格算分隔符，单个空格保留（商品名里本来就有空格）。
fn split_wide_spaces(line: &str) -> Vec<String> {
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

fn push_cell(out: &mut Vec<String>, cur: &mut String) {
    let v = cur.trim().to_string();
    if !v.is_empty() {
        out.push(v);
    }
    cur.clear();
}

/// 带引号的 CSV 拆分（Excel 导出的 csv 会给含逗号的单元格加引号）。
fn split_csv(line: &str) -> Vec<String> {
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

// ---------- 列角色 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Col {
    Name,
    Qty,
    Inbound,
    Dest,
    Customer,
    Mark,
    Waybill,
    Ignore,
    Unknown,
}

fn classify(header: &str) -> Col {
    let h = header.to_lowercase();
    let has = |keys: &[&str]| keys.iter().any(|k| h.contains(k));

    // 「客户名称」是下单方；「客户单号」还是单号
    if has(&["客户"]) && !has(&["单号"]) {
        return Col::Customer;
    }
    // 顺序要紧：「入库单号」是入库号，不是运单号
    if has(&["入库"]) {
        return Col::Inbound;
    }
    if has(&["单号", "运单", "waybill", "tracking"]) {
        return Col::Waybill;
    }
    if has(&[
        "目的地",
        "目的港",
        "到港",
        "收货仓",
        "收货地",
        "仓库",
        "destination",
    ]) {
        return Col::Dest;
    }
    if has(&["数量", "件数", "箱数", "qty", "quantity", "pcs"]) {
        return Col::Qty;
    }
    if has(&["标记", "已录", "勾选", "状态", "mark"]) {
        return Col::Mark;
    }
    if has(&[
        "长", "宽", "高", "尺寸", "毛重", "净重", "重量", "体积", "报关", "付款", "合计", "总",
        "cm", "kg",
    ]) {
        return Col::Ignore;
    }
    if has(&["名称", "品名", "商品", "货品", "产品", "描述", "name", "description", "sku"]) {
        return Col::Name;
    }
    Col::Unknown
}

/// 一行里取出来的原始字段（还没做合并展开和校验）。
#[derive(Debug, Default, Clone)]
struct RawRow {
    name: String,
    qty_raw: String,
    inbound: String,
    destination: String,
    customer: String,
    waybill: Option<String>,
    /// None = 这一格是空的（可能是合并单元格的下半部分）；Some(true/false) = 格子里有内容
    mark: Option<bool>,
    /// 格子内容不止一个「1」（例如 `TK5 1`），标出来让人确认
    mark_text: bool,
}

#[derive(Debug, Clone)]
struct Strategy {
    name: usize,
    qty: usize,
    inbound: usize,
    dest: Option<usize>,
    customer: Option<usize>,
    mark: Option<usize>,
    waybill: Option<usize>,
    label: String,
}

impl Strategy {
    fn from_header(header: &[String]) -> Option<Self> {
        let (mut name, mut qty, mut inbound, mut dest, mut customer, mut mark, mut waybill) =
            (None, None, None, None, None, None, None);
        for (i, cell) in header.iter().enumerate() {
            match classify(cell) {
                Col::Name if name.is_none() => name = Some(i),
                Col::Qty if qty.is_none() => qty = Some(i),
                Col::Inbound if inbound.is_none() => inbound = Some(i),
                Col::Dest if dest.is_none() => dest = Some(i),
                Col::Customer if customer.is_none() => customer = Some(i),
                Col::Mark if mark.is_none() => mark = Some(i),
                Col::Waybill if waybill.is_none() => waybill = Some(i),
                _ => {}
            }
        }
        let (name, qty, inbound) = (name?, qty?, inbound?);
        let mut label = format!(
            "按表头识别：名称=第{}列 数量=第{}列",
            name + 1,
            qty + 1
        );
        if let Some(d) = dest {
            label.push_str(&format!(" 目的地=第{}列", d + 1));
        }
        if let Some(c) = customer {
            label.push_str(&format!(" 客户=第{}列", c + 1));
        }
        label.push_str(&format!(" 入库号=第{}列", inbound + 1));
        Some(Self {
            name,
            qty,
            inbound,
            dest,
            customer,
            mark,
            waybill,
            label,
        })
    }

    /// 截图同款模板：名称 / 长 / 宽 / 高 / 毛重 / 体积 / 数量 / 标记 / 报关 / 目的地 / 入库号。
    fn positional(rows: &[Vec<String>]) -> Option<Self> {
        let maxlen = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if maxlen < 11 {
            return None;
        }
        Some(Self {
            name: 0,
            qty: 6,
            inbound: 10,
            dest: Some(9),
            customer: None,
            mark: Some(7),
            waybill: None,
            label: "按截图模板列序：名称=1 数量=7 标记=8 目的地=10 入库号=11".to_string(),
        })
    }

    fn row(&self, row: &[String]) -> RawRow {
        let get = |i: usize| row.get(i).map(|s| s.trim().to_string()).unwrap_or_default();
        let waybill = self
            .waybill
            .and_then(|i| row.get(i))
            .and_then(|s| extract_waybill(s));

        let mut out = RawRow {
            name: get(self.name),
            qty_raw: get(self.qty),
            inbound: get(self.inbound),
            destination: self.dest.map(get).unwrap_or_default(),
            customer: self.customer.map(get).unwrap_or_default(),
            waybill,
            mark: None,
            mark_text: false,
        };
        if let Some(i) = self.mark {
            if let Some(cell) = row.get(i) {
                let cell = cell.trim();
                if !cell.is_empty() {
                    out.mark = Some(has_mark_one(cell));
                    out.mark_text = out.mark == Some(true) && !is_only_one(cell);
                }
            }
        }
        out
    }
}

/// 单元格里是否有独立的「1」（`1`、`TK5 1` 都算）。
fn has_mark_one(cell: &str) -> bool {
    cell.split_whitespace().any(|t| t == "1") || cell.trim() == "1"
}

fn is_only_one(cell: &str) -> bool {
    let t = cell.trim();
    t == "1" || t == "１"
}

/// 没有表头也没有 11 列时的兜底：从右往左找像入库号的那一格。
fn guess_row(row: &[String]) -> RawRow {
    let inbound_idx = row
        .iter()
        .rposition(|c| is_id_like(c))
        .or_else(|| row.iter().rposition(|c| c.chars().all(|ch| ch.is_ascii_digit()) && c.len() >= 6));

    let inbound = inbound_idx.map(|i| row[i].clone()).unwrap_or_default();

    // 数量：入库号左边最近的纯整数
    let qty_idx = inbound_idx.and_then(|i| {
        (0..i).rev().find(|&j| {
            let t = row[j].trim();
            !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) && t.parse::<i64>().is_ok()
        })
    });

    RawRow {
        name: row.first().cloned().unwrap_or_default(),
        qty_raw: qty_idx.map(|i| row[i].clone()).unwrap_or_default(),
        inbound,
        destination: String::new(),
        customer: String::new(),
        waybill: row.iter().find_map(|c| extract_waybill(c)),
        mark: None,
        mark_text: false,
    }
}

fn is_id_like(cell: &str) -> bool {
    let t = cell.trim();
    if t.len() < 5 || t.len() > 24 || t.contains(char::is_whitespace) {
        return false;
    }
    if !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return false;
    }
    let letters = t.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let digits = t.chars().filter(|c| c.is_ascii_digit()).count();
    letters >= 1 && digits >= 2
}

// ---------- 二维表 → 结果 ----------

fn detect_header(rows: &[Vec<String>]) -> Option<usize> {
    for (i, row) in rows.iter().enumerate().take(3) {
        let hits = row.iter().filter(|c| classify(c) != Col::Unknown).count();
        if hits >= 2 {
            return Some(i);
        }
    }
    None
}

fn is_header_like(row: &[String]) -> bool {
    // 数据行里常常也带着「报关」「收货仓」「单号：xxx」这些词，光数关键词会把货行误杀。
    // 真表头不会有纯数字格子（长宽高、数量、体积都空着），先按这条挡一道。
    if row.iter().any(|c| c.trim().parse::<f64>().is_ok()) {
        return false;
    }
    row.iter().filter(|c| classify(c) != Col::Unknown).count() >= 3
}

fn parse_table(rows: Vec<Vec<String>>) -> PackingSheet {
    let mut sheet = PackingSheet::default();
    if rows.is_empty() {
        sheet.warnings.push("没有读到任何数据行".to_string());
        return sheet;
    }

    let header = detect_header(&rows);
    let strategy = header
        .and_then(|i| Strategy::from_header(&rows[i]))
        .or_else(|| Strategy::positional(&rows));

    sheet.mapping = match &strategy {
        Some(s) => s.label.clone(),
        None => "未找到表头，按内容猜列（建议带上表头或按截图模板粘贴）".to_string(),
    };

    let start = header.map(|i| i + 1).unwrap_or(0);
    // 单号常常写在表格右侧的独立格子里（甚至只有一行），先整表扫一遍兜底
    let table_waybill = rows.iter().flat_map(|r| r.iter()).find_map(|c| extract_waybill(c));
    let mut items: Vec<PackingItem> = Vec::new();
    let mut last_dest = String::new();
    let mut last_inbound = String::new();
    let mut last_customer = String::new();
    let mut waybills: Vec<String> = Vec::new();
    // 周报类表格按段写「单号：xxx」：段内向下沿用，遇到新的就切换
    let mut current_waybill = String::new();
    // 表尾的合计行（整行只有一个「总箱数」）：不是货物，记下来当整表合计数
    let mut table_total: Option<i64> = None;

    for row in rows.iter().skip(start) {
        if is_header_like(row) {
            continue;
        }
        if let Some(w) = row.iter().find_map(|c| extract_waybill(c)) {
            current_waybill = w;
        }
        let raw = match &strategy {
            Some(s) => s.row(row),
            None => guess_row(row),
        };
        // 客户名可能写在一行「只有客户名、没有货」的分隔行上（如「翰林」），
        // 这种行后面会被跳过，所以要在所有 continue 之前先记住它
        if !raw.customer.trim().is_empty() {
            last_customer = raw.customer.trim().to_string();
        }
        if raw.name.is_empty() && raw.qty_raw.is_empty() && raw.inbound.is_empty() {
            continue;
        }
        // 只有数量、没有名称/入库号/目的地 → 是表尾合计，别当成一条货
        if !raw.qty_raw.trim().is_empty()
            && raw.name.trim().is_empty()
            && raw.inbound.trim().is_empty()
            && raw.destination.trim().is_empty()
        {
            let v = parse_qty(&raw.qty_raw);
            if v > 0 {
                table_total = Some(v);
            }
            continue;
        }

        // 「单号: 6812704625」「(现168件)」这类散行：只收单号，不当成货物。
        // 判据要收紧：OCR 常常读丢数量，那种行仍然是货物行，不能丢。
        let own_inbound = !raw.inbound.trim().is_empty();
        let filled = row.iter().filter(|c| !c.trim().is_empty()).count();
        let looks_scatter = raw.name.contains("单号")
            || raw.name.contains("现")
            || (filled <= 1 && raw.destination.trim().is_empty());
        if raw.qty_raw.trim().is_empty() && !own_inbound && looks_scatter {
            if let Some(w) = raw
                .waybill
                .clone()
                .or_else(|| extract_waybill(&raw.name))
            {
                waybills.push(w);
            }
            continue;
        }

        let mut item = PackingItem {
            name: raw.name.trim().to_string(),
            qty_raw: raw.qty_raw.trim().to_string(),
            inbound: raw.inbound.trim().to_string(),
            destination: raw.destination.trim().to_string(),
            customer: raw.customer.trim().to_string(),
            waybill: raw
                .waybill
                .clone()
                .unwrap_or_else(|| current_waybill.clone()),
            checked: raw.mark.unwrap_or(false),
            ..Default::default()
        };

        // 客户名称通常只在段首写一次，下面沿用
        if item.customer.is_empty() {
            item.customer = last_customer.clone();
        } else {
            last_customer = item.customer.clone();
        }

        // 合并单元格的下半部分：目的地 / 入库号按上一行补齐（这两列留空一定不对）。
        // 补出来的值要标出来：OCR 把合并格里的字认在中间那行时，最上面那行会被
        // 补成**上一组**的入库号，这种只能靠人眼确认。
        if item.destination.is_empty() {
            item.destination = last_dest.clone();
            if !item.destination.is_empty() {
                item.issues.push("dest_inherit".to_string());
            }
        } else {
            last_dest = item.destination.clone();
        }
        if item.inbound.is_empty() {
            item.inbound = last_inbound.clone();
            if !item.inbound.is_empty() {
                item.issues.push("inbound_inherit".to_string());
            }
        } else {
            last_inbound = item.inbound.clone();
        }
        // 标记列**不做**向下填充：普通空单元格和合并单元格在文本里长得一样，
        // 硬填会把没录的行误标成已录。xlsx 走的是真正的 mergeCells，见 `apply_merges`。
        if raw.mark_text {
            item.issues.push("mark_text".to_string());
        }

        item.qty = parse_qty(&item.qty_raw);
        if item.qty <= 0 {
            item.issues.push("qty".to_string());
        }
        if item.name.is_empty() {
            item.issues.push("name".to_string());
        }
        if item.inbound.is_empty() {
            item.issues.push("inbound".to_string());
        } else if looks_like_sci(&item.inbound) {
            item.issues.push("inbound_sci".to_string());
        }
        if !item.waybill.is_empty() {
            waybills.push(item.waybill.clone());
        }

        items.push(item);
    }

    // 合并单元格里的字被 OCR 认在**中间行**时，同组最上面那行的入库号会是空的
    // （下面几行已被「向下填充」补上）。这里往后看一行补回来，并标出来让人确认。
    for i in 0..items.len() {
        if !items[i].inbound.is_empty() {
            continue;
        }
        let next = (i + 1..items.len())
            .take(2)
            .find(|&j| !items[j].inbound.is_empty())
            .map(|j| items[j].inbound.clone());
        if let Some(v) = next {
            items[i].issues.retain(|c| c != "inbound");
            items[i].inbound = v;
            items[i].issues.push("inbound_inherit".to_string());
        }
    }

    // 单号：整张表通常只有一个，缺的补上；出现多个要提醒
    let mut uniq: Vec<String> = Vec::new();
    for w in &waybills {
        if !uniq.contains(w) {
            uniq.push(w.clone());
        }
    }
    if uniq.is_empty() {
        if let Some(w) = table_waybill {
            uniq.push(w);
        }
    }
    sheet.waybill = uniq.first().cloned().unwrap_or_default();
    // 多个单号是常态（周报按单号分票展示），不提示；这里只记第一个当默认值
    for item in items.iter_mut() {
        if item.waybill.is_empty() {
            item.waybill = sheet.waybill.clone();
        }
    }
    if sheet.waybill.is_empty() {
        sheet.warnings.push("没识别到单号（表右侧的「单号: …」）".to_string());
    }

    // 表尾有合计行就用它；没有才退回文字里的「现N件」
    sheet.expected_qty = table_total.or_else(|| extract_expected(&rows));
    items = dedupe(items, &mut sheet.warnings);

    for (i, item) in items.iter_mut().enumerate() {
        item.seq = (i + 1) as u32;
    }
    sheet.total_qty = items.iter().map(|i| i.qty).sum();
    // 多单号的是周报/汇总表，文字里的「现N件」多半只是某一段的小结；
    // 但表尾真有合计行时那是整表数，照常校验
    if uniq.len() > 1 && table_total.is_none() {
        sheet.expected_qty = None;
    }
    if let Some(expected) = sheet.expected_qty {
        if expected != sheet.total_qty {
            sheet.warnings.push(format!(
                "合计对不上：图上 {expected} 件，解析出 {} 件",
                sheet.total_qty
            ));
        }
    }

    // 目的地 / 单号汇总（多目的地时给界面提示拆分）
    let mut dests: Vec<String> = Vec::new();
    for item in &items {
        if !item.destination.is_empty() && !dests.contains(&item.destination) {
            dests.push(item.destination.clone());
        }
    }
    sheet.destination = dests.join(" / ");

    sheet.items = items;
    sheet
}

fn parse_qty(raw: &str) -> i64 {
    let t = raw.trim().replace(',', "").replace('，', "");
    if t.is_empty() {
        return 0;
    }
    if let Ok(v) = t.parse::<i64>() {
        return v;
    }
    // "20件" 这种带单位的
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<i64>().unwrap_or(0)
}

fn looks_like_sci(s: &str) -> bool {
    let up = s.to_uppercase();
    up.contains("E+") || up.contains("E-") || s.contains('.')
}

/// 从任意单元格里找 `单号: 6812704625`；没有前缀时接受整格 8–12 位数字。
fn extract_waybill(cell: &str) -> Option<String> {
    let t = cell.trim();
    if t.is_empty() {
        return None;
    }
    if let Some(pos) = t.find("单号").or_else(|| t.find("运单")) {
        let rest = &t[pos..];
        let rest = rest.trim_start_matches(|c: char| !c.is_ascii_digit());
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 6 {
            return Some(digits);
        }
    }
    if t.len() >= 8 && t.len() <= 16 && t.chars().all(|c| c.is_ascii_digit()) {
        return Some(t.to_string());
    }
    None
}

/// 「(现168件)」里的合计。表里出现多个小结（`先174件` / `66件`）时不做校验：
/// 那是分段小计，不是整表合计，硬比只会一直报错。
fn extract_expected(rows: &[Vec<String>]) -> Option<i64> {
    let mut found: Vec<i64> = Vec::new();
    for row in rows {
        for cell in row {
            if let Some(pos) = cell.find('现') {
                let rest = &cell[pos + '现'.len_utf8()..];
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(v) = digits.parse::<i64>() {
                    if v > 0 && !found.contains(&v) {
                        found.push(v);
                    }
                }
            }
        }
    }
    if found.len() == 1 {
        found.first().copied()
    } else {
        None
    }
}

/// 完全同款的行只留一条（OCR 会把同一行认两遍）。
/// key 里必须带上数量/目的地：同一种货不同箱规是两批，不能被当成重复吃掉。
fn dedupe(items: Vec<PackingItem>, warnings: &mut Vec<String>) -> Vec<PackingItem> {
    let mut out: Vec<PackingItem> = Vec::new();
    let mut index: BTreeMap<(String, String, String, i64, String), usize> = BTreeMap::new();

    for item in items {
        if item.name.is_empty() || item.inbound.is_empty() {
            out.push(item);
            continue;
        }
        let key = (
            item.name.clone(),
            item.inbound.clone(),
            item.waybill.clone(),
            item.qty,
            item.destination.clone(),
        );
        match index.get(&key).copied() {
            None => {
                index.insert(key, out.len());
                out.push(item);
            }
            Some(pos) => {
                if item.checked && !out[pos].checked {
                    let seq = out[pos].seq;
                    out[pos] = item;
                    out[pos].seq = seq;
                    out[pos].issues.push("dup_mark".to_string());
                    warnings.push(format!(
                        "去重：{} / {} 保留已勾选的那条",
                        out[pos].name, out[pos].inbound
                    ));
                } else {
                    warnings.push(format!("去重：{} / {} 重复行已忽略", item.name, item.inbound));
                }
            }
        }
    }
    out
}

// ---------- 文件读取 ----------

fn read_text_file(path: &str) -> Result<String, PackingError> {
    let bytes = std::fs::read(path)
        .map_err(|e| PackingError::new("io", format!("读取 {path} 失败：{e}")))?;
    if let Ok(s) = String::from_utf8(bytes.clone()) {
        return Ok(s);
    }
    // Excel 另存的 csv 在中文 Windows 上通常是 GBK
    let (decoded, _, _) = encoding_rs::GBK.decode(&bytes);
    Ok(decoded.into_owned())
}

fn read_xlsx(path: &str) -> Result<Vec<Vec<String>>, PackingError> {
    let bytes = std::fs::read(path)
        .map_err(|e| PackingError::new("io", format!("读取 {path} 失败：{e}")))?;
    let files = unzip(&bytes)?;

    let shared = files
        .get("xl/sharedStrings.xml")
        .map(|b| shared_strings(b))
        .unwrap_or_default();

    let mut sheet_names: Vec<&String> = files
        .keys()
        .filter(|n| n.starts_with("xl/worksheets/sheet") && n.ends_with(".xml"))
        .collect();
    sheet_names.sort();
    let sheet_name = sheet_names
        .first()
        .ok_or_else(|| PackingError::new("xlsx", "xlsx 里没找到工作表"))?;
    let sheet_xml = &files[*sheet_name];

    let mut grid = sheet_rows(sheet_xml, &shared);
    if let Some(merges) = merge_ranges(sheet_xml) {
        apply_merges(&mut grid, &merges);
    }
    Ok(grid)
}

/// 合并单元格：把左上角的值铺满整个范围（Excel 里被合并的格子是空的）。
fn apply_merges(grid: &mut Vec<Vec<String>>, merges: &[(usize, usize, usize, usize)]) {
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

fn get_cell(grid: &[Vec<String>], r: usize, c: usize) -> String {
    grid.get(r).and_then(|row| row.get(c)).cloned().unwrap_or_default()
}

fn set_cell(grid: &mut Vec<Vec<String>>, r: usize, c: usize, v: String) {
    while grid.len() <= r {
        grid.push(Vec::new());
    }
    let row = &mut grid[r];
    while row.len() <= c {
        row.push(String::new());
    }
    row[c] = v;
}

// ---------- 极简 zip + xlsx 读取 ----------
// 只依赖 flate2：xlsx 就是个 zip，里面 sheet1.xml + sharedStrings.xml。
// 不引 calamine（当前环境无法联网拉新 crate），够用即可。

fn unzip(data: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, PackingError> {
    let n = data.len();
    let mut eocd = None;
    let from = n.saturating_sub(66_000);
    let mut i = n.saturating_sub(22);
    loop {
        if i < from {
            break;
        }
        if data[i..i + 4] == *b"PK\x05\x06" {
            eocd = Some(i);
            break;
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    let eocd = eocd.ok_or_else(|| PackingError::new("xlsx", "不是有效的 xlsx（找不到 zip 目录）"))?;

    let count = u16::from_le_bytes([data[eocd + 10], data[eocd + 11]]) as usize;
    let cd_off = u32::from_le_bytes([
        data[eocd + 16],
        data[eocd + 17],
        data[eocd + 18],
        data[eocd + 19],
    ]) as usize;

    let mut out: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut p = cd_off;
    for _ in 0..count {
        if p + 46 > n || data[p..p + 4] != *b"PK\x01\x02" {
            break;
        }
        let method = u16::from_le_bytes([data[p + 10], data[p + 11]]);
        let csize = u32::from_le_bytes([data[p + 20], data[p + 21], data[p + 22], data[p + 23]]) as usize;
        let nlen = u16::from_le_bytes([data[p + 28], data[p + 29]]) as usize;
        let elen = u16::from_le_bytes([data[p + 30], data[p + 31]]) as usize;
        let clen = u16::from_le_bytes([data[p + 32], data[p + 33]]) as usize;
        let loff = u32::from_le_bytes([data[p + 42], data[p + 43], data[p + 44], data[p + 45]]) as usize;
        let name = String::from_utf8_lossy(&data[p + 46..(p + 46 + nlen).min(n)]).into_owned();

        if loff + 30 <= n {
            let lnlen = u16::from_le_bytes([data[loff + 26], data[loff + 27]]) as usize;
            let lelen = u16::from_le_bytes([data[loff + 28], data[loff + 29]]) as usize;
            let start = loff + 30 + lnlen + lelen;
            let end = (start + csize).min(n);
            if start < end {
                let raw = &data[start..end];
                let body = if method == 8 { inflate(raw)? } else { raw.to_vec() };
                out.insert(name, body);
            }
        }
        p += 46 + nlen + elen + clen;
    }
    Ok(out)
}

fn inflate(raw: &[u8]) -> Result<Vec<u8>, PackingError> {
    let mut out = Vec::new();
    flate2::read::DeflateDecoder::new(raw)
        .read_to_end(&mut out)
        .map_err(|e| PackingError::new("xlsx", format!("解压失败：{e}")))?;
    Ok(out)
}

fn shared_strings(xml: &[u8]) -> Vec<String> {
    let s = String::from_utf8_lossy(xml).into_owned();
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(start) = s[pos..].find("<si") {
        let start = pos + start;
        let end = match s[start..].find("</si>") {
            Some(e) => start + e,
            None => break,
        };
        let block = &s[start..end];
        let mut text = String::new();
        let mut p = 0usize;
        while let Some(t) = block[p..].find("<t") {
            let t = p + t;
            if let Some(text_v) = inner_text(&block[t..], "t") {
                text.push_str(&unescape(&text_v));
            }
            p = t + 2;
        }
        out.push(text);
        pos = end + 5;
    }
    out
}

fn sheet_rows(xml: &[u8], shared: &[String]) -> Vec<Vec<String>> {
    let s = String::from_utf8_lossy(xml).into_owned();
    let mut grid: Vec<Vec<String>> = Vec::new();
    let mut pos = 0usize;

    while let Some(rs) = s[pos..].find("<row") {
        let row_start = pos + rs;
        let row_end = match s[row_start..].find("</row>") {
            Some(e) => row_start + e,
            None => break,
        };
        let row_xml = &s[row_start..row_end];
        let row_no = attr(row_xml, "r")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(grid.len() + 1);
        let r_idx = row_no.saturating_sub(1);

        let mut cp = 0usize;
        let mut col = 0usize;
        while let Some(cs) = row_xml[cp..].find("<c") {
            let c_start = cp + cs;
            let c_end = match row_xml[c_start..].find("</c>") {
                Some(e) => c_start + e + 4,
                None => break,
            };
            let c_xml = &row_xml[c_start..c_end];
            let c_idx = attr(c_xml, "r").map(|r| col_index(&r)).unwrap_or(col);
            let t = attr(c_xml, "t").unwrap_or_default();
            let value = if t == "s" {
                inner_text(c_xml, "v")
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .and_then(|i| shared.get(i).cloned())
                    .unwrap_or_default()
            } else if t == "inlineStr" {
                inner_text(c_xml, "is")
                    .map(|v| v)
                    .unwrap_or_default()
                    .pipe_strip_tags()
            } else {
                inner_text(c_xml, "v").unwrap_or_default()
            };
            set_cell(&mut grid, r_idx, c_idx, unescape(&value));
            col = c_idx + 1;
            cp = c_end;
        }
        pos = row_end + 6;
    }
    grid
}

/// `<is><t>abc</t></is>` → 取出里面所有 `<t>` 的文字。
trait StripTags {
    fn pipe_strip_tags(self) -> String;
}

impl StripTags for String {
    fn pipe_strip_tags(self) -> String {
        let mut out = String::new();
        let mut pos = 0usize;
        while let Some(t) = self[pos..].find("<t") {
            let t = pos + t;
            if let Some(v) = inner_text(&self[t..], "t") {
                out.push_str(&unescape(&v));
            }
            pos = t + 2;
        }
        if out.is_empty() {
            self
        } else {
            out
        }
    }
}

/// 取 `key="value"`（要求前面有空格，避免误配 `ref=` 之类）。
fn attr(xml: &str, key: &str) -> Option<String> {
    let pat = format!(" {key}=\"");
    let start = xml.find(&pat)? + pat.len();
    let rest = &xml[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// 取 `<tag ...>text</tag>` 的 text。
fn inner_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start = xml.find(&open)?;
    let after = &xml[start..];
    let gt = after.find('>')?;
    if after[..gt].ends_with('/') {
        return Some(String::new());
    }
    let body = &after[gt + 1..];
    let close = format!("</{tag}>");
    let end = body.find(&close)?;
    Some(body[..end].to_string())
}

/// "AB12" → 27（0 基列号）。
fn col_index(reference: &str) -> usize {
    let mut n = 0usize;
    for ch in reference.chars() {
        if ch.is_ascii_alphabetic() {
            n = n * 26 + (ch.to_ascii_uppercase() as usize - 'A' as usize + 1);
        } else {
            break;
        }
    }
    n.saturating_sub(1)
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        let end = tail.find(';').map(|e| e + 1).unwrap_or(1);
        let entity = &tail[..end];
        match entity {
            "&amp;" => out.push('&'),
            "&lt;" => out.push('<'),
            "&gt;" => out.push('>'),
            "&quot;" => out.push('"'),
            "&apos;" => out.push('\''),
            _ => {
                if let Some(num) = entity.strip_prefix("&#") {
                    let num = num.strip_suffix(';').unwrap_or(num);
                    let code = if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        num.parse::<u32>().ok()
                    };
                    if let Some(c) = code.and_then(char::from_u32) {
                        out.push(c);
                    }
                } else {
                    out.push_str(entity);
                }
            }
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// 合并单元格范围："A5:A7" → (4,0,6,0)。
fn merge_ranges(xml: &[u8]) -> Option<Vec<(usize, usize, usize, usize)>> {
    let s = String::from_utf8_lossy(xml).into_owned();
    let start = s.find("<mergeCells")?;
    let end = s[start..].find("</mergeCells>").map(|e| start + e).unwrap_or(s.len());
    let block = &s[start..end];

    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(m) = block[pos..].find("<mergeCell") {
        let m = pos + m;
        let seg = &block[m..];
        let ref_end = seg.find('>').map(|e| m + e).unwrap_or(block.len());
        if let Some(r) = attr(&block[m..ref_end + 1], "ref") {
            if let Some((a, b)) = r.split_once(':') {
                let (r1, c1) = split_ref(a);
                let (r2, c2) = split_ref(b);
                out.push((r1, c1, r2, c2));
            }
        }
        pos = ref_end + 1;
    }
    Some(out)
}

/// "A5" → (4, 0)
fn split_ref(reference: &str) -> (usize, usize) {
    let letters: String = reference.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let digits: String = reference.chars().skip_while(|c| c.is_ascii_alphabetic()).collect();
    let row = digits.parse::<usize>().unwrap_or(1).saturating_sub(1);
    (row, col_index(&letters))
}

// ---------- 测试 ----------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tsv() -> String {
        [
            "门铰链标准 20/件\t52\t37\t34\t10.90\t14.3\t20\tTK5 1\tEXX\t美西\tIN5LK113218",
            "门铰链+移除工具 18/件\t54\t39\t33\t11.58\t21.8\t33\t\tEXX\t美西\tIN5LK113220",
            "鱼鳞布10条装 100/件\t51\t36\t46\t14.07\t19.05\t5\t\tEXX\t美西\tIN5LK113223",
            "鼠标水平仪 80套一箱\t63\t38\t38\t15.16\t9.6\t10\t\tEXX\t美西\tIN5LK113230",
            "门窗密封条5米1个装 200/件\t52\t52\t32\t14.42\t21.7\t10\t\tEXX\t美西\tIN5LK113172",
            "门窗密封条5米2个装 100/件\t52\t52\t32\t14.42\t21.7\t10\t\tEXX\t",
            "门窗密封条5米3个装 40/件\t40\t43\t33\t9.46\t13.1\t10\t\tEXX\t美西\t",
            "大剪板器\t49\t41\t30\t10.04\t20.35\t20\t\tEXX\t美西\tIN5LK113165",
            "手持缝纫机 一箱46台\t62\t27\t49\t13.67\t17.4\t30\t1\tEXX\t美西\tIN5LK113249",
            "黑色三角门后挂钩, 60/件\t73\t54\t35\t22.99\t6.85\t14\t\tEXX\t美西\tIN5LK113256",
            "黑色三角门后挂钩, 64/件\t73\t54\t35\t22.99\t7.2\t1\t\tEXX\t",
            "白色三角门后挂钩, 60/件\t73\t54\t35\t22.99\t6.85\t5\t\tEXX\t",
            "单号: 6812704625",
            "(现168件)",
        ]
        .join("\n")
    }

    #[test]
    fn parses_screenshot_template() {
        let sheet = parse_text(&sample_tsv());
        assert_eq!(sheet.items.len(), 12, "12 行货物");
        assert_eq!(sheet.total_qty, 168);
        assert_eq!(sheet.expected_qty, Some(168));
        assert_eq!(sheet.waybill, "6812704625");
        assert_eq!(sheet.destination, "美西");

        let first = &sheet.items[0];
        assert_eq!(first.name, "门铰链标准 20/件");
        assert_eq!(first.qty, 20);
        assert_eq!(first.inbound, "IN5LK113218");
        assert!(first.checked);
        assert!(first.has_issue("mark_text"), "TK5 1 要标出来让人确认");

        // 合并单元格：第 5–7 行共用 IN5LK113172
        assert_eq!(sheet.items[4].inbound, "IN5LK113172");
        assert_eq!(sheet.items[5].inbound, "IN5LK113172");
        assert_eq!(sheet.items[6].inbound, "IN5LK113172");
        assert!(!sheet.items[5].checked, "合并标记不向下填");
        assert!(sheet.items[8].checked, "第 9 行有自己的 1");

        assert!(!sheet.warnings.iter().any(|w| w.contains("合计对不上")));
    }

    #[test]
    fn maps_columns_by_header() {
        let text = [
            "目的地\t商品名称\t入库号\t数量\t标记",
            "休斯顿\t门铰链\tA1001\t12\t1",
            "休斯顿\t鱼鳞布\tA1002\t8\t",
        ]
        .join("\n");
        let sheet = parse_text(&text);
        assert_eq!(sheet.items.len(), 2);
        assert_eq!(sheet.items[0].qty, 12);
        assert_eq!(sheet.items[0].inbound, "A1001");
        assert!(sheet.items[0].checked);
        assert!(!sheet.items[1].checked);
        assert!(sheet.mapping.contains("按表头"));
    }

    #[test]
    fn dedupes_by_name_and_inbound() {
        let text = [
            "商品\t数量\t入库号\t标记",
            "门铰链\t10\tA1\t",
            "门铰链\t10\tA1\t1",
        ]
        .join("\n");
        let sheet = parse_text(&text);
        assert_eq!(sheet.items.len(), 1);
        assert!(sheet.items[0].checked, "去重保留已勾选");
    }

    /// 真 xlsx（含合并单元格）：验证 zip 读取 + mergeCells 展开。
    #[test]
    fn parses_xlsx_fixture() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("装箱单样例.xlsx");
        if !path.exists() {
            return;
        }
        let sheet = parse_file(path.to_str().unwrap()).expect("解析 xlsx");
        assert_eq!(sheet.items.len(), 12);
        assert_eq!(sheet.total_qty, 168);
        assert_eq!(sheet.waybill, "6812704625", "单号在表格右侧独立格子里");
        assert!(sheet.items[0].checked && sheet.items[1].checked, "H1:H2 合并的 TK5 1");
        assert!(!sheet.items[2].checked, "没合并标记的行不能跟着勾上");
        assert_eq!(sheet.items[6].inbound, "IN5LK113172", "K5:K7 合并铺开");
        assert_eq!(sheet.items[11].inbound, "IN5LK113256", "K10:K12 合并铺开");
    }

    #[test]
    fn applies_merged_cells() {
        let mut grid = vec![
            vec!["门铰链".to_string(), "IN5".to_string()],
            vec!["鱼鳞布".to_string(), String::new()],
            vec!["大剪板器".to_string(), String::new()],
        ];
        apply_merges(&mut grid, &[(0, 1, 2, 1)]);
        assert_eq!(grid[2][1], "IN5");
    }

    #[test]
    fn extracts_waybill_from_cell() {
        assert_eq!(extract_waybill("单号: 6812704625").as_deref(), Some("6812704625"));
        assert_eq!(extract_waybill("6812704625").as_deref(), Some("6812704625"));
        assert_eq!(extract_waybill("EXX"), None);
    }

    #[test]
    fn unescapes_xml_entities() {
        assert_eq!(unescape("门&amp;窗"), "门&窗");
        assert_eq!(unescape("&#65;"), "A");
    }

    #[test]
    fn parses_col_index() {
        assert_eq!(col_index("A1"), 0);
        assert_eq!(col_index("K1"), 10);
        assert_eq!(col_index("AB1"), 27);
    }
}
