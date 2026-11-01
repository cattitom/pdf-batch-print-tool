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
        // 全表只有一个单号 → 每一行都补上（装箱单/OCR 老写法：单号只写在末尾一行）
        assert_eq!(first.waybill, "6812704625", "唯一单号要铺满整表");

        // 合并单元格：第 5–7 行共用 IN5LK113172
        assert_eq!(sheet.items[4].inbound, "IN5LK113172");
        assert_eq!(sheet.items[5].inbound, "IN5LK113172");
        assert_eq!(sheet.items[6].inbound, "IN5LK113172");
        assert!(!sheet.items[5].checked, "合并标记不向下填");
        assert!(sheet.items[8].checked, "第 9 行有自己的 1");

        assert!(!sheet.warnings.iter().any(|w| w.contains("合计对不上")));
    }

    #[test]
    fn waybill_written_per_block_does_not_backfill_earlier_rows() {
        // 云表格的真实形状：单号写在数据行右侧（合并单元格跨段），前面那些行没有单号。
        // 以前会把第一个单号倒填给全表，导致「翰林 六角柄…」凭空多出一个单号。
        let rows: Vec<Vec<String>> = vec![
            vec!["来货日期".into(), "客户名称".into(), "产品名称".into(), "总箱数".into(), "收货仓".into(), "入库号".into()],
            vec!["10/1".into(), "翰林".into(), "六角柄沉孔钻头套装，200/件".into(), "5".into(), "SAM美西".into(), "IB058261029RS".into()],
            vec!["10/2".into(), "九宸".into(), "钉子50包装".into(), "2".into(), "休斯顿".into(), "IB064261028RS".into(), "单号：6812705130".into()],
            vec!["10/2".into(), "九宸".into(), "一个装-炮钉枪50".into(), "15".into(), "休斯顿".into(), "IB064261028RU".into(), "单号：6812705130".into()],
            // 同一票后面又有一行没写单号：没写就是没写，不许按上一个单号往下顺延
            vec!["10/3".into(), "九宸".into(), "钉子100包装".into(), "7".into(), "休斯顿".into(), "IB064261028RV".into()],
        ];
        let sheet = parse_table(rows, true);
        assert_eq!(sheet.items.len(), 4);
        assert_eq!(sheet.items[0].waybill, "", "前面没有单号的行不许倒填");
        assert_eq!(sheet.items[1].waybill, "6812705130");
        assert_eq!(sheet.items[2].waybill, "6812705130");
        assert_eq!(sheet.items[3].waybill, "", "有多个单号时，缺单号的行必须留空");
    }

    #[test]
    fn text_segment_waybill_still_inherits_down() {
        // 文本/OCR 拿不到合并信息：段首写一次「单号: xxx」，段内沿用；多单号时不做整表回填
        let text = [
            "商品名称\t箱数\t收货仓\t入库号",
            "单号: 6812704625",
            "门铰链\t20\t美西\tIN5LK113218",
            "鱼鳞布\t5\t美西\tIN5LK113220",
            "单号: 6812704626",
            "大剪板器\t3\t美西\tIN5LK113223",
        ]
        .join("\n");
        let sheet = parse_text(&text);
        assert_eq!(sheet.items.len(), 3);
        assert_eq!(sheet.items[0].waybill, "6812704625");
        assert_eq!(sheet.items[1].waybill, "6812704625");
        assert_eq!(sheet.items[2].waybill, "6812704626");
    }

    #[test]
    fn header_waybill_still_fills_whole_table() {
        // 老写法：整张表只有表头一处「单号: xxx」，缺的照旧补上
        let text = [
            "单号: 6812704625",
            "商品名称	箱数	收货仓	入库号",
            "门铰链 20/件	20	美西	IN5LK113218",
            "鱼鳞布 100/件	5	美西	",
        ]
        .join("\n");
        let sheet = parse_text(&text);
        assert_eq!(sheet.items.len(), 2);
        assert_eq!(sheet.items[0].waybill, "6812704625");
        assert_eq!(sheet.items[1].waybill, "6812704625", "表头单号要铺满整表");
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

    #[test]
    fn red_font_between_name_and_inbound_marks_shipped() {
        // 云表格的写法：从「产品名称」到「入库号」这几列刷红字 = 已出货。
        let rows: Vec<Vec<String>> = vec![
            vec!["客户名称".into(), "产品名称".into(), "总箱数".into(), "收货仓".into(), "入库号".into()],
            vec!["九宸".into(), "一个装防尘带，300 套/件".into(), "1".into(), "休斯顿".into(), "IB064261030RV".into()],
            vec!["九宸".into(), "钉子50包装，75 套/件".into(), "2".into(), "休斯顿".into(), "IB064261030RW".into()],
            vec!["九宸".into(), "一个装-炮钉枪50，8 套/件".into(), "3".into(), "休斯顿".into(), "IB064261030RX".into()],
        ];
        let red = vec![
            vec![false; 5],
            // 红字只刷在中间几格也要算（WPS 里手刷时常常整段一起刷）
            vec![false, false, true, false, false],
            // 只有客户那格红：在「产品名称 → 入库号」区间外，不算已出
            vec![true, false, false, false, false],
            vec![false; 5],
        ];
        let sheet = parse_table_styled(rows, true, Some(&red));
        assert_eq!(sheet.items.len(), 3);
        assert!(sheet.items[0].checked, "区间里的红字 = 已出货");
        assert!(!sheet.items[1].checked, "客户列的红字不算");
        assert!(!sheet.items[2].checked, "没红字 = 未出货");
    }

    #[test]
    fn red_font_and_mark_column_are_either() {
        let rows: Vec<Vec<String>> = vec![
            vec!["产品名称".into(), "总箱数".into(), "标记".into(), "入库号".into()],
            vec!["只有红字".into(), "1".into(), "".into(), "IB1".into()],
            vec!["只有标记".into(), "2".into(), "1".into(), "IB2".into()],
            vec!["都没有".into(), "3".into(), "".into(), "IB3".into()],
        ];
        let red = vec![
            vec![false; 4],
            vec![true, false, false, false],
            vec![false; 4],
            vec![false; 4],
        ];
        let sheet = parse_table_styled(rows, true, Some(&red));
        assert_eq!(sheet.items.len(), 3);
        assert!(sheet.items[0].checked, "红字");
        assert!(sheet.items[1].checked, "老表的标记列 1");
        assert!(!sheet.items[2].checked);
    }

    #[test]
    fn parses_arrival_column() {
        // 云表格里的「到货情况」：界面上插在出货详情与备注之间
        let text = [
            "客户名称\t产品名称\t总箱数\t到货情况\t入库号",
            "翰林\t门铰链\t5\t未到需打包\tA1001",
            "翰林\t鱼鳞布\t8\t\tA1002",
        ]
        .join("\n");
        let sheet = parse_text(&text);
        assert_eq!(sheet.items.len(), 2);
        assert_eq!(sheet.items[0].arrival, "未到需打包");
        assert_eq!(sheet.items[1].arrival, "");
        // 别把「到货情况」认成标记列或收货仓
        assert!(!sheet.items[0].checked);
        assert_eq!(sheet.items[0].inbound, "A1001");
    }
