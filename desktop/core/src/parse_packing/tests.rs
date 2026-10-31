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
