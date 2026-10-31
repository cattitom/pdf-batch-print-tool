// 表头识别、列映射与逐行解析（核心规则）

use std::collections::BTreeMap;

use super::{PackingItem, PackingSheet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Col {
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

pub(crate) fn classify(header: &str) -> Col {
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
pub(crate) struct RawRow {
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
pub(crate) struct Strategy {
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
pub(crate) fn has_mark_one(cell: &str) -> bool {
    cell.split_whitespace().any(|t| t == "1") || cell.trim() == "1"
}

pub(crate) fn is_only_one(cell: &str) -> bool {
    let t = cell.trim();
    t == "1" || t == "１"
}

/// 没有表头也没有 11 列时的兜底：从右往左找像入库号的那一格。
pub(crate) fn guess_row(row: &[String]) -> RawRow {
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

pub(crate) fn is_id_like(cell: &str) -> bool {
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

pub(crate) fn detect_header(rows: &[Vec<String>]) -> Option<usize> {
    for (i, row) in rows.iter().enumerate().take(3) {
        let hits = row.iter().filter(|c| classify(c) != Col::Unknown).count();
        if hits >= 2 {
            return Some(i);
        }
    }
    None
}

pub(crate) fn is_header_like(row: &[String]) -> bool {
    // 数据行里常常也带着「报关」「收货仓」「单号：xxx」这些词，光数关键词会把货行误杀。
    // 真表头不会有纯数字格子（长宽高、数量、体积都空着），先按这条挡一道。
    if row.iter().any(|c| c.trim().parse::<f64>().is_ok()) {
        return false;
    }
    row.iter().filter(|c| classify(c) != Col::Unknown).count() >= 3
}

/// `merged_expanded`：传进来的二维表里，合并单元格是否已经按真实 mergeCells 铺开
/// （xlsx 走 `apply_merges` 就是 true；复制粘贴的文本 / OCR 拿不到合并信息，是 false）。
/// 为 true 时不再用「空单元格就抄上一行」猜合并——那会把真的空单元格伪造成上一票的值；
/// 复用只发生在合并区内部，而这是 Excel 自己给的。
pub(crate) fn parse_table(rows: Vec<Vec<String>>, merged_expanded: bool) -> PackingSheet {
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
    // 有真实合并信息（xlsx）时，空白就是空白，不许从上下行借
    let inherit_blank = !merged_expanded;
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
        // 收货仓：表里通常只在段首写一次、并不合并，所以照旧按上一行补齐
        if !item.destination.is_empty() {
            last_dest = item.destination.clone();
        } else {
            item.destination = last_dest.clone();
            if !item.destination.is_empty() {
                item.issues.push("dest_inherit".to_string());
            }
        }
        // 入库号：同上。合并区里复用，合并区外留空（标 missing 让人看）
        if !item.inbound.is_empty() {
            last_inbound = item.inbound.clone();
        } else if inherit_blank {
            item.inbound = last_inbound.clone();
            if !item.inbound.is_empty() {
                item.issues.push("inbound_inherit".to_string());
            }
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
    // xlsx 有真实 mergeCells，不需要这套补救——套上去会把空白行串到下一票去。
    for i in (0..items.len()).filter(|_| inherit_blank) {
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

pub(crate) fn parse_qty(raw: &str) -> i64 {
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

pub(crate) fn looks_like_sci(s: &str) -> bool {
    let up = s.to_uppercase();
    up.contains("E+") || up.contains("E-") || s.contains('.')
}

/// 从任意单元格里找 `单号: 6812704625`；没有前缀时接受整格 8–12 位数字。
pub(crate) fn extract_waybill(cell: &str) -> Option<String> {
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
pub(crate) fn extract_expected(rows: &[Vec<String>]) -> Option<i64> {
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
pub(crate) fn dedupe(items: Vec<PackingItem>, warnings: &mut Vec<String>) -> Vec<PackingItem> {
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

