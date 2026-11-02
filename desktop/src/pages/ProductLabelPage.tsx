import { useState } from "react";
import { open as openFiles, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import HistoryButton from "../components/HistoryButton";
import { api, errorHint, errorText } from "../api";

/**
 * 产品标签：只负责「数据录入 + 分份」。
 * 版式和打印交给 gLabels——它输出矢量，比本工具扁平化出来的 PDF 清晰。
 */

interface Row {
  code: string;
  name: string;
  copies: number;
}

const emptyRow = (): Row => ({ code: "", name: "", copies: 1 });

/** 上次导出 CSV 的完整路径（存在本机设置里，下次直接覆盖同名文件）。 */
const KEY_CSV_PATH = "product_label_csv_path";

const CJK = /[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff]/;

/**
 * 拆一行：编码是英文/数字，名称是中文，所以不用逗号也能分。
 * 切法优先级：Tab 或逗号（Excel 两列粘贴）→ 第一个空白（空白在汉字之前才算）
 * → 第一个汉字（编码和名称直接连写时）→ 整行当编码。
 */
function splitLine(line: string): { code: string; name: string } {
  const sep = line.search(/[,，\t]/);
  if (sep >= 0) {
    return { code: line.slice(0, sep).trim(), name: line.slice(sep + 1).trim() };
  }
  const space = line.search(/\s/);
  const cjk = line.search(CJK);
  // 行首就是汉字说明没有编码，整行当名称、丢掉（避免中文编码混进表格）
  if (cjk === 0) return { code: "", name: line.trim() };
  if (cjk > 0 && (space < 0 || space >= cjk)) {
    return { code: line.slice(0, cjk).trim(), name: line.slice(cjk).trim() };
  }
  if (space > 0) {
    return { code: line.slice(0, space).trim(), name: line.slice(space + 1).trim() };
  }
  return { code: line.trim(), name: "" };
}

/** 每行一条产品。 */
function parseBulk(text: string): Row[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line !== "")
    .map((line) => ({ ...splitLine(line), copies: 1 }))
    .filter((r) => r.code !== "");
}

/** CSV 单元格：含逗号/引号/换行时用双引号包起来（标准 CSV，gLabels 认）。 */
function csvCell(s: string): string {
  return /[",\r\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

export default function ProductLabelPage() {
  const [rows, setRows] = useState<Row[]>([emptyRow()]);
  const [bulk, setBulk] = useState("");
  const [csvPath, setCsvPath] = useState<string | null>(null);
  /** 首行留空：gLabels 默认把第一行当表头，空白行让数据从第二行开始。 */
  const [blankFirst, setBlankFirst] = useState(true);
  const [importing, setImporting] = useState(false);
  const [importNote, setImportNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const items = rows
    .filter((r) => r.code.trim() !== "")
    .map((r) => ({
      code: r.code.trim(),
      name: r.name.trim(),
      copies: Math.max(1, Math.floor(r.copies || 1)),
    }));
  const total = items.reduce((n, r) => n + r.copies, 0);

  const patchRow = (i: number, p: Partial<Row>) =>
    setRows((prev) => prev.map((r, j) => (j === i ? { ...r, ...p } : r)));

  /** 解析区改动即刷新表格；同一个编码保留已填的份数。 */
  const onBulkChange = (text: string) => {
    setBulk(text);
    const parsed = parseBulk(text);
    if (parsed.length === 0) return;
    setRows((prev) => {
      const copiesByCode = new Map(prev.map((r) => [r.code.trim(), r.copies]));
      return parsed.map((r) => ({ ...r, copies: copiesByCode.get(r.code) ?? 1 }));
    });
  };

  /** 导出 CSV 给 gLabels：一条记录一行，份数按行展开（Merge 一条记录出一张）。 */
  const doExportCsv = async () => {
    setError(null);
    setCsvPath(null);
    if (items.length === 0) {
      setError("请至少填写一行编码");
      return;
    }
    try {
      // 默认路径：上次导出的那份（通常是 gLabels 正指着的 labels.csv），没有就用同名字
      let lastPath = "labels.csv";
      try {
        const saved = await api.allSettings();
        if (saved[KEY_CSV_PATH]) lastPath = saved[KEY_CSV_PATH];
      } catch {
        // 读设置失败就用默认名，不挡导出
      }
      const outPath = await saveFileDialog({
        defaultPath: lastPath,
        filters: [{ name: "CSV", extensions: ["csv"] }],
      });
      if (!outPath) return;

      const lines: string[] = [];
      for (const it of items) {
        for (let i = 0; i < it.copies; i++) {
          lines.push(`${csvCell(it.code)},${csvCell(it.name)}`);
        }
      }
      // save_text_file 用 UTF-8 写盘、不带 BOM，gLabels 直接能读
      const body = lines.join("\r\n");
      await api.saveTextFile(outPath, (blankFirst ? "\r\n" : "") + body + "\r\n");
      setCsvPath(outPath);
      // 导出完直接打开文件所在的位置并选中它，省得用户自己找
      revealItemInDir(outPath).catch(() => {});
      // 记一笔历史，方便之后从「历史」里找回这份 CSV
      api
        .addPrintHistory({
          outputName: outPath.split(/[\\/]/).pop() ?? outPath,
          outputPath: outPath,
          outputPages: 0,
          paper: null,
          sepPages: 0,
          params: { kind: "product-label-csv", rows: lines.length },
          files: [],
        })
        .catch(() => {});
      api.setSetting(KEY_CSV_PATH, outPath).catch(() => {
        // 记不住路径不影响本次导出
      });
    } catch (e) {
      setError(
        errorText(e) +
          errorHint(e) +
          "（如果 gLabels 正开着这个 CSV，先关掉再导出）",
      );
    }
  };

  /** 拍/选产品标签照片 → OCR 认编码和名称，追加到快速解析（已有编码跳过）。 */
  const importImages = async () => {
    setError(null);
    setImportNote(null);
    const picked = await openFiles({
      multiple: true,
      filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp", "bmp"] }],
    });
    if (!picked) return;
    const paths = Array.isArray(picked) ? picked : [picked];
    setImporting(true);
    try {
      const r = await api.parseLabelImages(paths);
      const seen = new Set(items.map((it) => it.code.toUpperCase()));
      const fresh: { code: string; name: string }[] = [];
      for (const row of r.rows) {
        const code = row.code.trim();
        if (!code || seen.has(code.toUpperCase())) continue;
        seen.add(code.toUpperCase());
        fresh.push({ code, name: row.name.trim() });
      }
      if (fresh.length) {
        // 用「现有行 + 新认出来的」重建文本，别只拿图片结果覆盖——
        // 用户可能是在表格里一行行手填的，bulk 里没有那些内容
        const merged = [...items, ...fresh];
        const next = merged
          .map((it) => (it.name ? `${it.code} ${it.name}` : it.code))
          .join("\n");
        onBulkChange(next);
      }
      const notes: string[] = [];
      if (fresh.length) notes.push(`从 ${paths.length} 张图认出 ${fresh.length} 条`);
      if (r.rows.length > fresh.length) {
        notes.push(`跳过 ${r.rows.length - fresh.length} 条重复或空编码`);
      }
      if (r.failed.length) notes.push(`没读出来：${r.failed.join("；")}`);
      setImportNote(notes.length ? notes.join("；") : "图片里没认出编码");
    } catch (e) {
      setError(errorText(e) + errorHint(e));
    } finally {
      setImporting(false);
    }
  };

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1 className="page-title">产品标签</h1>
          <p className="page-sub">录入编码和名称、分配份数，导出 CSV 交给 gLabels 排版打印</p>
        </div>
        <div className="page-actions">
          <HistoryButton />
          <label className="setting-toggle" title="gLabels 默认把第一行当表头；留空后数据从第二行开始">
            <input
              type="checkbox"
              checked={blankFirst}
              onChange={(e) => setBlankFirst(e.target.checked)}
            />
            首行留空
          </label>
          <button className="btn primary big" onClick={doExportCsv} disabled={items.length === 0}>
            导出 CSV（给 gLabels）
          </button>
        </div>
      </header>

      <section className="card">
        <div className="card-head">
          <span className="card-title">快速解析</span>
          <span className="card-sub">每行一条：英文编码 + 中文名称</span>
          <button
            className="btn sm"
            style={{ marginLeft: "auto" }}
            onClick={importImages}
            disabled={importing}
            title="选箱子上的产品标签照片，自动认出编码和名称"
          >
            {importing ? "识别中…" : "从图片导入"}
          </button>
        </div>
        <div className="card-body">
          <textarea
            className="boxplan-textarea mono"
            value={bulk}
            onChange={(e) => onBulkChange(e.target.value)}
            placeholder={"ZX-WYB-LF 晚宴包绿粉色\nZX-WYB-DALF 晚宴包深绿粉\nZX-WYB-HZ 晚宴包黑棕色"}
            spellCheck={false}
          />
          <div className="muted" style={{ marginTop: 8 }}>
            编码读到第一个汉字或空白为止；Excel 两列直接粘（Tab / 逗号）也认；空行自动忽略
          </div>
          {importNote && (
            <div className="muted" style={{ marginTop: 4 }}>
              {importNote}
            </div>
          )}
        </div>
      </section>

      <section className="card">
        <div className="card-head">
          <span className="card-title">标签内容</span>
          <span className="card-sub">共 {total} 张</span>
        </div>
        <div className="card-body tight">
          <div className="boxplan-table-wrap">
            <table className="hist-table boxplan-input-table product-label-table">
            <thead>
              <tr>
                <th style={{ width: "40%" }}>编码（条码内容）</th>
                <th>名称</th>
                <th style={{ width: 84 }}>份数</th>
                <th style={{ width: 48 }}></th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row, i) => (
                <tr key={i}>
                  <td>
                    <input
                      value={row.code}
                      placeholder="如 CX-RMG-04"
                      onChange={(e) => patchRow(i, { code: e.target.value })}
                    />
                  </td>
                  <td>
                    <input
                      value={row.name}
                      placeholder="如 染眉膏-04"
                      onChange={(e) => patchRow(i, { name: e.target.value })}
                    />
                  </td>
                  <td>
                    <input
                      type="number"
                      min={1}
                      value={row.copies}
                      onChange={(e) =>
                        patchRow(i, { copies: Math.max(1, +e.target.value || 1) })
                      }
                    />
                  </td>
                  <td>
                    <button
                      className="btn danger sm"
                      title="删除这一行"
                      onClick={() =>
                        setRows((prev) =>
                          prev.length > 1 ? prev.filter((_, j) => j !== i) : [emptyRow()]
                        )
                      }
                    >
                      ✕
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          </div>
          <div className="toolbar" style={{ padding: "10px 4px 2px" }}>
            <button className="btn" onClick={() => setRows((prev) => [...prev, emptyRow()])}>
              ＋ 添加一行
            </button>
            <span className="muted">编码必填；份数 = 这个标签打几张（CSV 里按份数展开成多行）</span>
          </div>
        </div>
      </section>

      {csvPath && (
        <div className="banner ok">
          <span>已导出 CSV（UTF-8 无 BOM），共 {total} 行</span>
          <div className="banner-path">{csvPath}</div>
        </div>
      )}
      {error && <div className="banner err">{error}</div>}
    </div>
  );
}
