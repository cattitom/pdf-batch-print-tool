import HistoryButton from "../components/HistoryButton";
import { Fragment, useCallback, useEffect, useState, type ReactElement } from "react";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { tempDir, join } from "@tauri-apps/api/path";
import {
  CloseOutlined,
  ExportOutlined,
  EyeOutlined,
  FilePdfOutlined,
  FileTextOutlined,
  InboxOutlined,
  PlusOutlined,
  PrinterOutlined,
  SplitCellsOutlined,
} from "@ant-design/icons";
import {
  api,
  errorHint,
  errorText,
  listenImposeProgress,
  type CropSpec,
  type ImposeFile,
  type ImposeOpts,
  type PdfInfo,
} from "../api";

const ROTATIONS = [0, 90, 180, 270];

// ---------- 类型 ----------

interface FileRow {
  uid: string;
  name: string;
  path: string;
  pages: number;
  widthMm: number;
  heightMm: number;
  targetPages: number;
  rotate: number;
  cropMode: "none" | "auto" | "manual";
  autoMarginMm: number;
  manual: { topMm: number; bottomMm: number; leftMm: number; rightMm: number };
}

let uidSeq = 0;
const nextUid = () => `f${Date.now().toString(36)}_${uidSeq++}`;

function toCropSpec(r: FileRow): CropSpec | null {
  if (r.cropMode === "auto") return { type: "auto", marginMm: r.autoMarginMm };
  if (r.cropMode === "manual") return { type: "manual", ...r.manual };
  return null;
}

function buildOutputName(rows: FileRow[]): string {
  if (rows.length === 0) {
    const ts = new Date().toISOString().slice(0, 19).replace(/[-:T]/g, "");
    return `批量打印_${ts}.pdf`;
  }
  const MAX_BASE = 180;
  const sanitize = (s: string) => s.replace(/[\\/:*?"<>|]/g, "_").trim() || "file";
  const parts = rows.map((r) => ({
    stem: sanitize(r.name.replace(/\.[^.]+$/, "")),
    cnt: String(r.targetPages || r.pages || 0),
  }));
  const overhead = parts.reduce((a, p) => a + p.cnt.length + 1, 0) + Math.max(0, parts.length - 1);
  const budget = MAX_BASE - overhead;
  let names = parts.map((p) => p.stem);
  const total = names.reduce((a, n) => a + n.length, 0);
  if (total > budget) {
    const per = Math.max(4, Math.floor(budget / names.length));
    names = names.map((n) => (n.length > per ? n.slice(0, per) : n));
  }
  let base = names.map((n, i) => `${n}_${parts[i].cnt}`).join("_");
  if (base.length > MAX_BASE) base = base.slice(0, MAX_BASE);
  return `${base}.pdf`;
}

/** 页码范围文字：第 a–b 页 / 第 a 页 */
function rangeText(start: number, count: number): string {
  return count <= 1 ? `第 ${start} 页` : `第 ${start}–${start + count - 1} 页`;
}

/** 拼版排布：横向卡片流（文件 / 分隔页 / 页码区间），改设置即时重算。 */
function SequenceBar({ rows, sepPages, copies }: { rows: FileRow[]; sepPages: number; copies: number }) {
  let cursor = 1;
  let contentTotal = 0;
  let sepTotal = 0;
  const parts: ReactElement[] = [];

  rows.forEach((r, i) => {
    // 第一个文件前面不插分隔页（与后端规则一致）
    if (i > 0 && sepPages > 0) {
      const rng = rangeText(cursor, sepPages);
      cursor += sepPages;
      sepTotal += sepPages;
      parts.push(
        <div className="sq-sep" key={`sep-${r.uid}`}>
          <span>分隔页 ×{sepPages}</span>
          <span className="r">{rng}</span>
        </div>
      );
    }
    // 每页连印 copies 份，所以实际输出页数是 页数 × 份数
    const n = Math.max(1, r.targetPages) * Math.max(1, copies);
    const rng = rangeText(cursor, n);
    cursor += n;
    contentTotal += n;

    const badges: string[] = [];
    if (r.rotate) badges.push(`旋转 ${r.rotate}°`);
    if (r.cropMode === "auto") badges.push("去白边");
    if (r.cropMode === "manual") badges.push("手动裁剪");
    if (copies > 1) badges.push(`每页 ${copies} 份`);

    parts.push(
      <div className="sq-item" key={r.uid}>
        <span className="t">
          {i + 1}. {r.name}
        </span>
        <span className="m">
          {n} 页{r.rotate ? ` · ${r.rotate}°` : ""}
        </span>
        {badges.length > 0 ? (
          badges.map((b) => (
            <span className="b" key={b}>
              {b}
            </span>
          ))
        ) : (
          <span className="b nb">原样</span>
        )}
        <span className="r">{rng}</span>
      </div>
    );
  });

  const total = contentTotal + sepTotal;
  return (
    <section className="card">
      <div className="card-head">
        <span className="card-title">拼版排布</span>
        <span className="card-sub">
          共 {rows.length} 个文件 · 内容 {contentTotal} 页 + 分隔 {sepTotal} 页
        </span>
        <div className="spacer" />
        <span className="card-sub">
          输出 <b style={{ color: "var(--text)", fontSize: 13 }}>{total}</b> 页
        </span>
      </div>
      <div className="card-body">
        <div className="seq-wrap">
          <div className="seq-flow">
            {parts.map((p, i) => (
              <Fragment key={i}>
                {i > 0 && <span className="sq-arrow">›</span>}
                {p}
              </Fragment>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}

export default function ImposePage({ pendingDrop }: { pendingDrop?: { paths: string[]; nonce: number } | null }) {
  const [rows, setRows] = useState<FileRow[]>([]);
  const [defaultTarget, setDefaultTarget] = useState(1);
  const [sepPages, setSepPages] = useState(1);
  const [copies, setCopies] = useState(1);
  const [sepText, setSepText] = useState("这是分隔线");
  const [pageNum, setPageNum] = useState(false);
  const [outputName, setOutputName] = useState("批量打印.pdf");
  const [outputDirty, setOutputDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [busyAction, setBusyAction] = useState<"compose" | "composeOpen" | null>(null);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [result, setResult] = useState<{ path: string; pages: number; elapsedMs: number; sizeBytes: number } | null>(null);
  const [openedWith, setOpenedWith] = useState<string | null>(null);
  const [openError, setOpenError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [previewPages, setPreviewPages] = useState<string[] | null>(null);
  const [previewTitle, setPreviewTitle] = useState("");

  // 输出文件名自动跟随（用户改过就不再自动）
  useEffect(() => {
    if (!outputDirty) setOutputName(buildOutputName(rows));
  }, [rows, outputDirty]);

  // 进度事件
  useEffect(() => {
    let un: (() => void) | undefined;
    listenImposeProgress((p) => setProgress(p)).then((u) => (un = u));
    return () => un?.();
  }, []);

  const addPaths = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) return;
      const infos: PdfInfo[] = await api.pdfInfos(paths);
      const newRows: FileRow[] = paths.map((p, i) => {
        const name = p.split(/[\\/]/).pop() ?? p;
        const info = infos[i];
        return {
          uid: nextUid(),
          name,
          path: p,
          pages: info.pages,
          widthMm: Math.round(info.widthMm),
          heightMm: Math.round(info.heightMm),
          targetPages: defaultTarget,
          rotate: 0,
          cropMode: "none",
          autoMarginMm: 2,
          manual: { topMm: 5, bottomMm: 5, leftMm: 5, rightMm: 5 },
        };
      });
      setRows((prev) => [...prev, ...newRows]);
    },
    [defaultTarget]
  );

  const addFiles = useCallback(async () => {
    const picked = await openFileDialog({
      multiple: true,
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
    if (!picked) return;
    const paths = Array.isArray(picked) ? picked : [picked];
    await addPaths(paths);
  }, [addPaths]);

  // 系统拖放进来的文件（App 层全局监听，自动切到本页）
  useEffect(() => {
    if (pendingDrop && pendingDrop.paths.length > 0) {
      addPaths(pendingDrop.paths).catch((e) => setError(errorText(e) + errorHint(e)));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingDrop?.nonce]);

  const patch = (uid: string, p: Partial<FileRow>) =>
    setRows((prev) => prev.map((r) => (r.uid === uid ? { ...r, ...p } : r)));

  const remove = (uid: string) => setRows((prev) => prev.filter((r) => r.uid !== uid));

  // ---- 排序（↑↓ 按钮） ----
  const move = (idx: number, delta: number) =>
    setRows((prev) => {
      const j = idx + delta;
      if (j < 0 || j >= prev.length) return prev;
      const next = [...prev];
      [next[idx], next[j]] = [next[j], next[idx]];
      return next;
    });

  // ---- 单文件预览（应用该文件设置临时合成 → 渲染前几页） ----
  const previewFile = async (row: FileRow) => {
    setError(null);
    setBusy(true);
    setPreviewPages(null);
    try {
      const tmp = await join(await tempDir(), `pdf_preview_${Date.now()}.pdf`);
      const files: ImposeFile[] = [
        {
          name: row.name,
          path: row.path,
          targetPages: Math.max(1, row.targetPages),
          rotate: row.rotate,
          crop: toCropSpec(row),
        },
      ];
      const opts: ImposeOpts = {
        sepPages: 0,
        sepText: null,
        pageNumbers: pageNum,
        // 预览也按同样的份数排，看到的就是印出来的顺序
        copies: Math.max(1, copies),
      };
      const res = await api.imposePdfs(files, opts, tmp);
      const imgs = await api.previewPdf(res.path, 150, 6);
      setPreviewTitle(row.name);
      setPreviewPages(imgs);
    } catch (e) {
      setError(errorText(e) + errorHint(e));
    } finally {
      setBusy(false);
      setProgress(null);
    }
  };

  // ---- 用 WPS 打开成品（自动打开与「重试打开」共用） ----
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

  // ---- 合成：openInWps=true 时合成完自动用 WPS PDF 打开 ----
  const runImpose = async (openInWps = false) => {
    if (rows.length === 0) return;
    setError(null);
    setResult(null);
    setOpenedWith(null);
    setOpenError(null);
    setBusy(true);
    setBusyAction(openInWps ? "composeOpen" : "compose");
    try {
      // 不弹另存为对话框：直接输出到第一个源文件所在文件夹
      const first = rows[0].path;
      const cut = Math.max(first.lastIndexOf("\\"), first.lastIndexOf("/"));
      const outPath = cut > 0 ? `${first.slice(0, cut + 1)}${outputName}` : outputName;
      // 重名冲突由后端自动加 " (n)" 后缀处理（返回的 res.path 是最终路径）
      const files: ImposeFile[] = rows.map((r) => ({
        name: r.name,
        path: r.path,
        targetPages: Math.max(1, r.targetPages),
        rotate: r.rotate,
        crop: toCropSpec(r),
      }));
      const opts: ImposeOpts = {
        sepPages,
        sepText: sepPages > 0 ? sepText : null,
        pageNumbers: pageNum,
        copies: Math.max(1, copies),
      };

      const res = await api.imposePdfs(files, opts, outPath);
      setResult(res);
      // 写入历史（失败不打扰主流程）
      api
        .addPrintHistory({
          outputName: outPath.split(/[\\/]/).pop() ?? outPath,
          outputPath: outPath,
          outputPages: res.pages,
          paper: null,
          sepPages,
          params: { sepText, rotateByFile: files.map((f) => f.rotate) },
          files: files.map(({ name, targetPages, rotate, crop }) => ({ name, targetPages, rotate, crop })),
        })
        .catch(() => {});
      // 合成成功后自动在 WPS PDF 里打开
      if (openInWps) await openWithWps(res.path);
    } catch (e) {
      setError(errorText(e) + errorHint(e));
    } finally {
      setBusy(false);
      setBusyAction(null);
      setProgress(null);
    }
  };

  const inputPages = rows.reduce((a, r) => a + r.pages, 0);
  const contentPages = rows.reduce((a, r) => a + Math.max(1, r.targetPages) * Math.max(1, copies), 0);
  const sepTotal = rows.length > 1 ? (rows.length - 1) * sepPages : 0;

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1 className="page-title">PDF 拼版</h1>
          <p className="page-sub">多个 PDF 合成一份，按目标页数凑页，文件之间自动补分隔页</p>
        </div>
        <div className="page-actions">
          <HistoryButton />
          <button className="btn" onClick={() => runImpose(false)} disabled={busy || rows.length === 0}>
            {busy && busyAction === "compose" ? "合成中…" : "仅合成"}
          </button>
          <button className="btn primary big" onClick={() => runImpose(true)} disabled={busy || rows.length === 0}>
            {busy && busyAction === "composeOpen" ? "合成中…" : "合成并打开 WPS PDF"}
          </button>
        </div>
      </header>

      {rows.length > 0 && (
        <section className="kpi-row">
          <div className="kpi">
            <div className="kpi-label">
              <FilePdfOutlined />
              文件
            </div>
            <div className="kpi-value">
              {rows.length}
              <small>个</small>
            </div>
            <div className="kpi-hint">按队列顺序合并</div>
          </div>
          <div className="kpi">
            <div className="kpi-label">
              <FileTextOutlined />
              输入页
            </div>
            <div className="kpi-value">
              {inputPages}
              <small>页</small>
            </div>
            <div className="kpi-hint">原始内容总量</div>
          </div>
          <div className="kpi">
            <div className="kpi-label">
              <PrinterOutlined />
              输出页
            </div>
            <div className="kpi-value">
              {contentPages + sepTotal}
              <small>页</small>
            </div>
            <div className="kpi-hint">
              内容 {contentPages} 页 + 分隔 {sepTotal} 页
            </div>
          </div>
          <div className="kpi">
            <div className="kpi-label">
              <SplitCellsOutlined />
              分隔页
            </div>
            <div className="kpi-value">
              {sepTotal}
              <small>页</small>
            </div>
            <div className="kpi-hint">每个文件 {sepPages} 页</div>
          </div>
        </section>
      )}

      {rows.length === 0 ? (
        <div className="empty lg" onClick={addFiles}>
          <div className="empty-icon">
            <InboxOutlined />
          </div>
          <div className="empty-title">把 PDF 拖到这里，或点击添加</div>
          <div className="empty-sub">
            支持多选 · 合并顺序用 ↑↓ 调整 · 每个文件可单独设页数 / 旋转 / 裁剪
          </div>
          <div className="empty-actions">
            <button
              className="btn primary"
              onClick={(e) => {
                e.stopPropagation();
                addFiles();
              }}
            >
              <PlusOutlined />
              选择 PDF
            </button>
          </div>
        </div>
      ) : (
        <section className="card">
          <div className="card-head">
            <span className="card-title">文件队列</span>
            <span className="card-sub">{rows.length} 个文件 · 按顺序合并</span>
            <label
              className="ctl queue-unify"
              title="改一次，队列里所有文件的页数一起改（单个文件还能单独微调）"
            >
              统一页数
              <input
                type="number"
                min={1}
                value={defaultTarget}
                aria-label="统一页数"
                onChange={(e) => {
                  const v = Math.max(1, +e.target.value || 1);
                  setDefaultTarget(v);
                  // 所有已添加文件的目标页数同步更新
                  setRows((prev) => prev.map((r) => ({ ...r, targetPages: v })));
                }}
              />
            </label>
            <div className="spacer" />
            <button className="btn soft sm" onClick={addFiles} disabled={busy}>
              <PlusOutlined />
              添加 PDF
            </button>
          </div>
          <div className="card-body tight">
            <div className="file-list">
              {rows.map((r, idx) => (
                <div key={r.uid} className="file-card">
                  <div className="reorder">
                    <button
                      className="btn"
                      disabled={idx === 0}
                      onClick={() => move(idx, -1)}
                      title="上移"
                      aria-label="上移"
                    >
                      ▲
                    </button>
                    <button
                      className="btn"
                      disabled={idx === rows.length - 1}
                      onClick={() => move(idx, 1)}
                      title="下移"
                      aria-label="下移"
                    >
                      ▼
                    </button>
                  </div>
                  <div className="file-index">{idx + 1}</div>
                  <div className="file-main">
                    <div className="file-name" title={r.name}>
                      {r.name}
                    </div>
                    <div className="file-meta">
                      <span>
                        原 {r.pages} 页 · {r.widthMm}×{r.heightMm} mm
                      </span>
                    </div>
                  </div>
                  <div className="file-controls">
                    <button
                      className="btn preview-btn"
                      onClick={() => previewFile(r)}
                      disabled={busy}
                      title="按当前设置预览该文件"
                    >
                      <EyeOutlined />
                      预览
                    </button>
                    <label className="ctl">
                      页数
                      <input
                        type="number"
                        min={1}
                        value={r.targetPages}
                        onChange={(e) => patch(r.uid, { targetPages: Math.max(1, +e.target.value || 1) })}
                      />
                    </label>
                    <label className="ctl">
                      旋转
                      <select value={r.rotate} onChange={(e) => patch(r.uid, { rotate: +e.target.value })}>
                        {ROTATIONS.map((a) => (
                          <option key={a} value={a}>
                            {a === 0 ? "不旋转" : `${a}°`}
                          </option>
                        ))}
                      </select>
                    </label>
                    <label className="ctl">
                      裁剪
                      <select
                        value={r.cropMode}
                        onChange={(e) => patch(r.uid, { cropMode: e.target.value as FileRow["cropMode"] })}
                      >
                        <option value="none">不裁剪</option>
                        <option value="auto">自动去白边</option>
                        <option value="manual">手动裁剪</option>
                      </select>
                    </label>
                    {r.cropMode === "auto" && (
                      <label className="ctl">
                        留白
                        <input
                          type="number"
                          min={0}
                          step={0.5}
                          style={{ width: 52 }}
                          value={r.autoMarginMm}
                          onChange={(e) =>
                            patch(r.uid, { autoMarginMm: Math.max(0, +e.target.value || 0) })
                          }
                        />
                        mm
                      </label>
                    )}
                    {r.cropMode === "manual" && (
                      <div className="manual-crop">
                        {(
                          [
                            ["topMm", "上"],
                            ["rightMm", "右"],
                            ["bottomMm", "下"],
                            ["leftMm", "左"],
                          ] as const
                        ).map(([k, label]) => (
                          <label className="ctl" key={k}>
                            {label}
                            <input
                              type="number"
                              min={0}
                              style={{ width: 44 }}
                              value={r.manual[k]}
                              onChange={(e) =>
                                patch(r.uid, {
                                  manual: { ...r.manual, [k]: Math.max(0, +e.target.value || 0) },
                                })
                              }
                            />
                          </label>
                        ))}
                      </div>
                    )}
                    <button className="file-del" onClick={() => remove(r.uid)} title="移除" aria-label="移除">
                      <CloseOutlined />
                    </button>
                  </div>
                </div>
              ))}
            </div>
          </div>
        </section>
      )}

      <section className="card">
        <div className="card-head">
          <span className="card-title">输出设置</span>
          <span className="card-sub">保存到第一个源文件所在文件夹，重名自动加 (1)</span>
        </div>
        <div className="card-body">
          <div className="opts-row">
            <div className="field" style={{ width: 86 }}>
              <label>分隔页数量</label>
              <input
                type="number"
                min={0}
                value={sepPages}
                onChange={(e) => setSepPages(Math.max(0, +e.target.value || 0))}
              />
            </div>
            <div className="field" style={{ width: 240 }}>
              <label>分隔页文字</label>
              <input
                value={sepText}
                onChange={(e) => setSepText(e.target.value)}
                disabled={sepPages === 0}
                title="分隔页上印的文字"
              />
            </div>
            <div className="field" style={{ width: 96 }}>
              <label>每页份数</label>
              <input
                type="number"
                min={1}
                max={20}
                value={copies}
                title="每页连印几份：页数 3、份数 2 → 1 1 2 2 3 3（箱子标签一箱两份）"
                onChange={(e) => setCopies(Math.min(20, Math.max(1, +e.target.value || 1)))}
              />
            </div>
            <div className="field" style={{ width: 168 }}>
              <label>页码</label>
              <label className="check-line">
                <input type="checkbox" checked={pageNum} onChange={(e) => setPageNum(e.target.checked)} />
                <span>内容页右下角编号</span>
              </label>
            </div>
            <div className="field grow" style={{ minWidth: 220 }}>
              <label>输出文件名</label>
              <input
                value={outputName}
                title="保存到第一个源文件所在文件夹，重名自动加 (1) 后缀"
                onChange={(e) => {
                  setOutputName(e.target.value);
                  setOutputDirty(true);
                }}
              />
            </div>
          </div>
        </div>
      </section>

      {rows.length > 0 && <SequenceBar rows={rows} sepPages={sepPages} copies={copies} />}

      {/* 进度 / 结果 */}
      {busy && progress && (
        <div className="progress-wrap">
          <div className="progress-bar">
            <div
              className="progress-fill"
              style={{ width: `${progress.total > 0 ? (progress.done / progress.total) * 100 : 0}%` }}
            />
          </div>
          <div className="progress-text">
            {progress.done} / {progress.total} 页
          </div>
        </div>
      )}
      {result && (
        <div className="banner ok">
          <span>合成完成</span>
          <span style={{ fontWeight: 400 }}>
            {result.pages} 页 · {(result.sizeBytes / 1024 / 1024).toFixed(1)} MB ·{" "}
            {(result.elapsedMs / 1000).toFixed(2)}s
            {openedWith && <> · 已在 {openedWith} 中打开</>}
          </span>
          <div className="banner-path">
            <ExportOutlined /> {result.path}
          </div>
        </div>
      )}
      {openError && (
        <div className="banner warn">
          <span>文件已合成，但自动用 WPS 打开失败：{openError}</span>
          {result && (
            <button className="btn sm banner-btn" onClick={() => openWithWps(result.path)}>
              重试打开
            </button>
          )}
        </div>
      )}
      {error && <div className="banner err">{error}</div>}

      {/* 预览弹窗 */}
      {previewPages && (
        <div className="modal-mask" onClick={() => setPreviewPages(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-head">
              <span>预览 · {previewTitle}（最多前 6 页）</span>
              <button className="btn sm" onClick={() => setPreviewPages(null)}>
                关闭
              </button>
            </div>
            <div className="modal-body">
              {previewPages.map((b64, i) => (
                <div className="preview-page-wrap" key={i}>
                  <img
                    src={`data:image/png;base64,${b64}`}
                    className="preview-page"
                    alt={`第 ${i + 1} 页`}
                  />
                  <div className="preview-page-no">第 {i + 1} 页</div>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
