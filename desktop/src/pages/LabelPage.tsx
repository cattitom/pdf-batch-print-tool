import { useEffect, useRef, useState } from "react";
import { save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { api, errorHint, errorText, type LabelSku } from "../api";

interface SkuRow {
  sku: string;
  boxes: number;
  pcsPerBox: number;
  quantities: string;
  newFlag: boolean;
}

const emptyRow = (): SkuRow => ({ sku: "", boxes: 1, pcsPerBox: 1, quantities: "", newFlag: true });

export default function LabelPage() {
  const [inboundNo, setInboundNo] = useState("");
  const [warehouse, setWarehouse] = useState("");
  const [client, setClient] = useState("");
  const [shelf, setShelf] = useState("1");
  const [skus, setSkus] = useState<SkuRow[]>([emptyRow()]);
  const [preview, setPreview] = useState<string | null>(null);
  const [result, setResult] = useState<{ path: string; boxes: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loadedLatest, setLoadedLatest] = useState(false);

  // 打开页面时带出上一次单据
  useEffect(() => {
    if (loadedLatest) return;
    setLoadedLatest(true);
    api.latestLabelDoc().then((doc) => {
      if (!doc) return;
      setInboundNo(doc.inboundNo);
      setWarehouse(doc.warehouse);
      setClient(doc.client);
      setShelf(doc.shelf);
      try {
        // 历史库里的旧记录是 snake_case（pcs_per_box / new_flag），一并兼容
        type StoredSku = Partial<LabelSku> & { pcs_per_box?: number; new_flag?: boolean };
        const rows = doc.skus as StoredSku[];
        if (Array.isArray(rows) && rows.length > 0) {
          setSkus(
            rows.map((r) => ({
              sku: r.sku ?? "",
              boxes: r.boxes ?? 1,
              pcsPerBox: r.pcsPerBox ?? r.pcs_per_box ?? 1,
              quantities: r.quantities ?? "",
              newFlag: r.newFlag ?? r.new_flag ?? true,
            }))
          );
        }
      } catch {
        /* 忽略旧格式 */
      }
    });
  }, [loadedLatest]);

  const patchRow = (i: number, p: Partial<SkuRow>) =>
    setSkus((prev) => prev.map((r, j) => (j === i ? { ...r, ...p } : r)));

  const toSpec = () => ({
    inboundNo,
    warehouse,
    client,
    shelf,
    skus: skus.filter((s) => s.sku.trim() !== ""),
  });

  // 实时预览：停手 450ms 自动刷新，不需要按钮；同一份参数直接走缓存。
  // 单次渲染约 0.5s（genpdf 要把整份中文字体嵌进 PDF），所以不按键触发、只按「停手」触发。
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const previewCache = useRef(new Map<string, string>());
  useEffect(() => {
    const ready = inboundNo.trim() !== "" && skus.some((s) => s.sku.trim() !== "");
    if (!ready) {
      setPreview(null);
      setPreviewError(null);
      setPreviewLoading(false);
      return;
    }
    const spec = toSpec();
    const key = JSON.stringify(spec);
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
        const b64 = await api.previewLabel(spec);
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
    }, 450);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [inboundNo, warehouse, client, shelf, skus]);

  const doGenerate = async () => {
    setError(null);
    setResult(null);
    try {
      const outPath = await saveFileDialog({
        defaultPath: `箱唛_${inboundNo || "单据"}.pdf`,
        filters: [{ name: "PDF", extensions: ["pdf"] }],
      });
      if (!outPath) return;
      setBusy(true);
      const res = await api.generateLabels(toSpec(), outPath);
      setResult(res);
    } catch (e) {
      setError(errorText(e) + errorHint(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1 className="page-title">箱唛生成</h1>
          <p className="page-sub">一箱一页，100×100mm 热敏标签，条码 + 二维码 + 中文排版</p>
        </div>
        <div className="page-actions">
          <button className="btn primary big" onClick={doGenerate} disabled={busy}>
            {busy ? "生成中…" : "生成箱唛"}
          </button>
        </div>
      </header>

      <div className="split">
        <div className="split-main">
          <section className="card">
            <div className="card-head">
              <span className="card-title">单据信息</span>
              <span className="card-sub">入库单号必填，其余可留空</span>
            </div>
            <div className="card-body">
              <div className="form-grid flat">
                <div className="field">
                  <label>入库单号 *</label>
                  <input
                    value={inboundNo}
                    onChange={(e) => setInboundNo(e.target.value)}
                    placeholder="如 IN20261020001"
                  />
                </div>
                <div className="field">
                  <label>客户</label>
                  <input value={client} onChange={(e) => setClient(e.target.value)} />
                </div>
                <div className="field">
                  <label>仓库</label>
                  <input value={warehouse} onChange={(e) => setWarehouse(e.target.value)} />
                </div>
                <div className="field">
                  <label>货架号</label>
                  <input value={shelf} onChange={(e) => setShelf(e.target.value)} />
                </div>
              </div>
            </div>
          </section>

          <section className="card">
            <div className="card-head">
              <span className="card-title">SKU 与箱数</span>
              <span className="card-sub">件数明细不填时按「箱数 × 每箱件数」</span>
            </div>
            <div className="card-body">
              <div className="sku-table flat">
                <div className="sku-head">
                  <span>SKU</span>
                  <span>箱数</span>
                  <span>每箱件数</span>
                  <span>件数明细（如 250*6,167*2,166）</span>
                  <span>New</span>
                  <span />
                </div>
                {skus.map((row, i) => (
                  <div className="sku-row" key={i}>
                    <input
                      value={row.sku}
                      onChange={(e) => patchRow(i, { sku: e.target.value })}
                      placeholder="SKU 编码"
                    />
                    <input
                      type="number"
                      min={1}
                      value={row.boxes}
                      onChange={(e) => patchRow(i, { boxes: Math.max(1, +e.target.value || 1) })}
                    />
                    <input
                      type="number"
                      min={1}
                      value={row.pcsPerBox}
                      onChange={(e) => patchRow(i, { pcsPerBox: Math.max(1, +e.target.value || 1) })}
                    />
                    <input
                      value={row.quantities}
                      onChange={(e) => patchRow(i, { quantities: e.target.value })}
                      placeholder="可选，填了按明细展开"
                    />
                    <input
                      type="checkbox"
                      checked={row.newFlag}
                      onChange={(e) => patchRow(i, { newFlag: e.target.checked })}
                    />
                    <button
                      className="btn danger sm"
                      title="删除这行"
                      onClick={() =>
                        setSkus((prev) => (prev.length > 1 ? prev.filter((_, j) => j !== i) : prev))
                      }
                    >
                      ✕
                    </button>
                  </div>
                ))}
                <button className="btn" onClick={() => setSkus((prev) => [...prev, emptyRow()])}>
                  ＋ 添加 SKU
                </button>
              </div>
            </div>
          </section>
        </div>

        <section className="card split-side">
          <div className="card-head">
            <span className="card-title">首页预览</span>
            <span className="card-sub">{previewLoading ? "生成中…" : "实时"}</span>
          </div>
          <div className="card-body">
            {preview ? (
              <img src={preview} alt="箱唛预览" className="preview-img" />
            ) : (
              <div className="preview-empty">
                填好「入库单号」和至少一个 SKU 后，这里会自动显示第一张箱唛
              </div>
            )}
            {previewError && <div className="banner err" style={{ marginTop: 10 }}>{previewError}</div>}
          </div>
        </section>
      </div>

      {result && (
        <div className="banner ok">
          <span>已生成 {result.boxes} 张箱唛</span>
          <div className="banner-path">{result.path}</div>
        </div>
      )}
      {error && <div className="banner err">{error}</div>}
    </div>
  );
}
