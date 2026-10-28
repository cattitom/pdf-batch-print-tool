import { useEffect, useRef, useState } from "react";
import { save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { api, errorHint, errorText } from "../api";

/** 产品标签：粘贴「编码,名称」快速解析，或直接在表格里改；生成后自动用 WPS 打开。 */

interface Row {
  code: string;
  name: string;
  copies: number;
}

const emptyRow = (): Row => ({ code: "", name: "", copies: 1 });

/** 每行一条：第一个逗号（半角/全角/制表符）前是编码，后面是名称。 */
function parseBulk(text: string): Row[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line !== "")
    .map((line) => {
      const i = line.search(/[,，\t]/);
      if (i < 0) return { code: line.trim(), name: "", copies: 1 };
      return {
        code: line.slice(0, i).trim(),
        name: line.slice(i + 1).trim(),
        copies: 1,
      };
    })
    .filter((r) => r.code !== "");
}

export default function ProductLabelPage() {
  const [rows, setRows] = useState<Row[]>([emptyRow()]);
  const [bulk, setBulk] = useState("");
  const [preview, setPreview] = useState<string | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [result, setResult] = useState<{ path: string; pages: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [openedWith, setOpenedWith] = useState<string | null>(null);
  const [openError, setOpenError] = useState<string | null>(null);

  const previewCache = useRef(new Map<string, string>());

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

  // 实时预览：停手 500ms 刷新；同一份数据走缓存（每次 embed 字体约 0.5s）
  useEffect(() => {
    if (items.length === 0) {
      setPreview(null);
      setPreviewError(null);
      setPreviewLoading(false);
      return;
    }
    const key = JSON.stringify(items);
    const cached = previewCache.current.get(key);
    if (cached) {
      setPreview(cached);
      setPreviewError(null);
      setPreviewLoading(false);
      return;
    }
    let cancelled = false;
    setPreviewLoading(true);
    const timer = setTimeout(async () => {
      try {
        const b64 = await api.previewProductLabel(items);
        const url = `data:image/png;base64,${b64}`;
        if (previewCache.current.size > 20) previewCache.current.clear();
        previewCache.current.set(key, url);
        if (!cancelled) {
          setPreview(url);
          setPreviewError(null);
        }
      } catch (e) {
        if (!cancelled) setPreviewError(errorText(e) + errorHint(e));
      } finally {
        if (!cancelled) setPreviewLoading(false);
      }
    }, 500);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rows]);

  // ---- 用 WPS 打开成品（生成后自动打开与「重试打开」共用） ----
  const openWithWps = async (path: string) => {
    setOpenError(null);
    try {
      const opened = await api.openPdfInWps(path);
      setOpenedWith(opened.app);
    } catch (e) {
      setOpenedWith(null);
      setOpenError(errorText(e) + errorHint(e));
    }
  };

  const doGenerate = async () => {
    setError(null);
    setResult(null);
    setOpenedWith(null);
    setOpenError(null);
    if (items.length === 0) {
      setError("请至少填写一行编码");
      return;
    }
    try {
      const outPath = await saveFileDialog({
        defaultPath: `产品标签_${new Date().toISOString().slice(0, 10)}.pdf`,
        filters: [{ name: "PDF", extensions: ["pdf"] }],
      });
      if (!outPath) return;
      setBusy(true);
      const res = await api.generateProductLabels(items, outPath);
      setResult(res);
      // 和拼版页一致：生成完直接用 WPS PDF 打开
      await openWithWps(res.path);
    } catch (e) {
      setError(errorText(e) + errorHint(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page">
      <div className="toolbar">
        <button className="btn primary big" onClick={doGenerate} disabled={busy || items.length === 0}>
          {busy ? "生成中…" : "🏷 生成产品标签"}
        </button>
        <span className="muted">
          50 × 30 mm，一页一张，共 {total} 张 · 生成后自动用 WPS 打开
        </span>
      </div>

      <div className="form-grid">
        <div className="boxplan-title">快速解析</div>
        <div className="field grow">
          <label>每行一条，第一个逗号前是编码、后面是名称（Excel 两列可直接粘进来）</label>
          <textarea
            className="boxplan-textarea mono"
            value={bulk}
            onChange={(e) => onBulkChange(e.target.value)}
            placeholder={"CX-RMG-04，染眉膏-04\nCX-RMG-02，染眉膏-02\nCX-RMG-01，染眉膏-01"}
            spellCheck={false}
          />
        </div>
        <div className="toolbar">
          <span className="muted">支持半角逗号、全角逗号、制表符；空行自动忽略</span>
        </div>
      </div>

      <div className="form-grid">
        <div className="boxplan-title">标签内容</div>
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
        <div className="toolbar">
          <button className="btn" onClick={() => setRows((prev) => [...prev, emptyRow()])}>
            ＋ 添加一行
          </button>
          <span className="muted">编码必填；份数 = 这个标签打印几张（同一条会连续出页）</span>
        </div>
      </div>

      <div className="preview-wrap">
        <div className="preview-title">
          首页预览{previewLoading ? " · 生成中…" : ""}
        </div>
        {preview ? (
          <img src={preview} alt="产品标签预览" className="preview-img" />
        ) : (
          <div className="preview-empty">
            填好第一行编码后，这里会自动显示 50 × 30 mm 的标签效果
          </div>
        )}
        {previewError && <div className="banner err">❌ {previewError}</div>}
      </div>

      {result && (
        <div className="banner ok">
          ✅ 已生成 {result.pages} 张产品标签
          {openedWith && <> · 已在 {openedWith} 中打开</>}
          <div className="banner-path">{result.path}</div>
        </div>
      )}
      {openError && (
        <div className="banner warn">
          ⚠️ 文件已生成，但自动用 WPS 打开失败：{openError}
          {result && (
            <button className="btn sm banner-btn" onClick={() => openWithWps(result.path)}>
              重试打开
            </button>
          )}
        </div>
      )}
      {error && <div className="banner err">❌ {error}</div>}
    </div>
  );
}
