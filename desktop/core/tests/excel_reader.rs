//! 文件级回归：测试样本由 xlwt / openpyxl 独立生成，生成脚本与样本一起保留。
use pdfimpose_core::{parse_file, parse_xls_file, xls::Book};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn reads_both_excel_formats_with_merges_styles_and_original_text() {
    for filename in ["excel-reader.xls", "excel-reader.xlsx"] {
        let path = fixture(filename);
        let book = Book::open(path.to_str().unwrap()).expect(filename);
        let sheet = book.sheet(" 11月 ").unwrap();
        assert_eq!(sheet.name, "11月");
        assert_eq!(
            sheet.rows[1][1], "门&窗 &#65; <样品>",
            "不能重复解码实体：{filename}"
        );
        assert_eq!(sheet.rows[2][1], "中文货物");
        assert_eq!(sheet.rows[1][4], "IB00001");
        assert_eq!(sheet.rows[2][4], "IB00001", "展开合并的入库号：{filename}");
        assert!(sheet.red[1][1]);
        assert!(sheet.red[2][4], "合并区域继承红字：{filename}");
        assert!(sheet.red[3][0], "客户列红字仍能读取：{filename}");
        assert!(!sheet.red[3][1], "黑字不能跟着客户列变红：{filename}");
        assert!(sheet.red[4][1], "自定义深红调色板：{filename}");
        assert!(!sheet.red[5][1], "珊瑚色不算已出货：{filename}");
        assert_eq!(sheet.rows[8][7], "TRUE");
        assert_eq!(sheet.rows[9][7], "123.5");
        assert_eq!(sheet.rows[10][7], "00042", "保留数字显示格式：{filename}");
        assert_eq!(
            sheet.rows[11][7],
            "共享字符串".repeat(3000),
            "长字符串跨记录/富文本：{filename}"
        );
        for (row, red) in sheet.rows.iter().zip(&sheet.red) {
            assert_eq!(row.len(), red.len());
        }
    }
}

#[test]
fn chooses_latest_populated_xls_sheet_and_preserves_explicit_selection() {
    let path = fixture("excel-reader.xls");
    let path = path.to_str().unwrap();
    let (sheet, selected, names) = parse_xls_file(path, None).unwrap();
    assert_eq!(names, ["10月", "11月", "12月模板", "说明"]);
    assert_eq!(selected, "11月", "忽略末尾说明页和只有表头的下月模板");
    assert_eq!(sheet.items.len(), 5);
    assert_eq!(sheet.total_qty, 20);
    assert_eq!(parse_file(path).unwrap().total_qty, 20);
    let (old, selected, _) = parse_xls_file(path, Some("10月")).unwrap();
    assert_eq!(selected, "10月");
    assert_eq!(old.total_qty, 9);
    assert!(parse_xls_file(path, Some("不存在")).is_err());
}

#[test]
fn business_shipped_flags_work_for_both_excel_formats() {
    for filename in ["excel-reader.xls", "excel-reader.xlsx"] {
        let path = fixture(filename);
        let sheet = parse_file(path.to_str().unwrap()).expect(filename);
        assert_eq!(sheet.items.len(), 5);
        assert_eq!(sheet.total_qty, 20);
        assert_eq!(
            sheet.items.iter().map(|i| i.checked).collect::<Vec<_>>(),
            [true, true, false, true, false],
            "{filename}"
        );
        assert_eq!(sheet.items[0].inbound, sheet.items[1].inbound);
    }
}

#[test]
fn xlsx_uses_relationships_and_reads_cached_formula_without_recalculation() {
    // 实际工作表名为 incoming-data.xml，不能假定 sheet1.xml 或靠文件名字排序。
    let path = fixture("excel-reader.xlsx");
    let book = Book::open(path.to_str().unwrap()).unwrap();
    assert_eq!(book.names(), ["11月"]);
    let sheet = book.sheet("11月").unwrap();
    assert_eq!(sheet.rows[12][7], "20");
}

#[test]
fn invalid_excel_returns_an_error() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    assert!(Book::open(path.to_str().unwrap()).is_err());
    assert!(Book::open("nonexistent-excel-reader-file.xls").is_err());
}

#[test]
fn merged_quantity_counts_once_and_keeps_both_products_traceable() {
    for filename in ["excel-quantity-merged.xls", "excel-quantity-merged.xlsx"] {
        let path = fixture(filename);
        let sheet = parse_file(path.to_str().unwrap()).unwrap();
        assert_eq!(sheet.items.len(), 2, "两款商品都保留：{filename}");
        assert_eq!(sheet.expected_qty, Some(1));
        assert_eq!(sheet.total_qty, 1, "合箱数量只计一次：{filename}");
        assert!(sheet.warnings.is_empty(), "{:?}", sheet.warnings);
        assert_eq!(sheet.items[0].source_row, Some(2));
        assert_eq!(sheet.items[1].source_row, Some(3));
        assert_eq!(sheet.items[0].quantity_group.as_deref(), Some("C2:C3"));
        assert_eq!(sheet.items[1].quantity_group.as_deref(), Some("C2:C3"));
        assert_eq!(sheet.items[0].qty, 1);
        assert_eq!(sheet.items[1].qty, 0);
        assert_eq!(sheet.items[1].qty_raw, "1", "仍可核对合并数量原文");
        assert!(sheet.items[1].has_issue("qty_shared"));
        assert!(!sheet.items[1].has_issue("qty"), "合箱不属于数量缺失");
        assert_eq!(sheet.items[1].inbound, "IB-SHARED");
    }
}
