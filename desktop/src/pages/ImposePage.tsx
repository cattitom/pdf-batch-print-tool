import { Fragment, useCallback, useEffect, useState, type ReactElement } from "react";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { tempDir, join } from "@tauri-apps/api/path";
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

/// 输出纸张：none = 保持原尺寸；其余按 (宽mm, 高mm) 缩放并自动跟随页面方向。
const PAPERS = {
  none: null,
  a4: { w: 210, h: 297 },
  a3: { w: 297, h: 420 },
} as const;
type PaperKey = keyof typeof PAPERS;

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

function buildOutputName(rows: FileRow[]): string {  if (rows.length === 0) {
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

// ---------- 组件 ----------

/** 页码范围文字：第 a–b 页 / 第 a 页 */
function rangeText(start: number, count: number): string {
  return count <= 1 ? `第 ${start} 页` : `第 ${start}–${start + count - 1} 页`;
}

/** 拼版排布：横向卡片流（文件 / 分隔页 / 页码区间），改设置即时重算。 */
function SequenceBar({ rows, sepPages }: { rows: FileRow[]; sepPages: number }) {
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
    const n = Math.max(1, r.targetPages);
    const rng = rangeText(cursor, n);
    cursor += n;
    contentTotal += n;

    const badges: string[] = [];
    if (r.rotate) badges.push(`旋转 ${r.rotate}°`);
    if (r.cropMode === "auto") badges.push("去白边");
    if (r.cropMode === "manual") badges.push("手动裁剪");

    parts.push(
      <div className="sq-item" key={r.uid}>
        <span className="t">
          {i + 1}. {r.name}
        </span>
        <span className="m">{n} 页</span>
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
    <div className="seq-wrap">
      <div className="seq-title">拼版排布</div>
      <div className="seq-flow">
        {parts.map((p, i) => (
          <Fragment key={i}>
            {i > 0 && <span className="sq-arrow">›</span>}
            {p}
          </Fragment>
        ))}
      </div>
      <div className="seq-total">
        共 {rows.length} 个文件 · 内容页 {contentTotal} 页 + 分隔页 {sepTotal} 页 ={" "}
        <b>输出 {total} 页</b>
      </div>
    </div>
  );
}

export default function ImposePage({ pendingDrop }: { pendingDrop?: { paths: string[]; nonce: number } | null }) {
  const [rows, setRows] = useState<FileRow[]>([]);
  const [defaultTarget, setDefaultTarget] = useState(1);
  const [sepPages, setSepPages] = useState(1);
  const [sepText, setSepText] = useState("这是分隔线");
  const [pageNum, setPageNum] = useState(false);
  const [paper, setPaper] = useState<PaperKey>("none");
  const [outputName, setOutputName] = useState("批量打印.pdf");
  const [outputDirty, setOutputDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [result, setResult] = useState<{ path: string; pages: number; elapsedMs: number; sizeBytes: number } | null>(null);
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
        paperWmm: PAPERS[paper]?.w ?? null,
        paperHmm: PAPERS[paper]?.h ?? null,
        sepPages: 0,
        sepText: null,
        pageNumbers: pageNum,
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

  // ---- 合成 ----
  const runImpose = async () => {
    if (rows.length === 0) return;
    setError(null);
    setResult(null);
    setBusy(true);
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
        paperWmm: PAPERS[paper]?.w ?? null,
        paperHmm: PAPERS[paper]?.h ?? null,
        sepPages,
        sepText: sepPages > 0 ? sepText : null,
        pageNumbers: pageNum,
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
          params: { sepText, paper, rotateByFile: files.map((f) => f.rotate) },
          files: files.map(({ name, targetPages, rotate, crop }) => ({ name, targetPages, rotate, crop })),
        })
        .catch(() => {});
    } catch (e) {
      setError(errorText(e) + errorHint(e));
    } finally {
      setBusy(false);
      setProgress(null);
    }
  };

  return (
    <div className="page">
      {/* 顶栏操作 */}
      <div className="toolbar">
        <button className="btn primary" onClick={addFiles} disabled={busy}>
          ＋ 添加 PDF
        </button>
        <div className="field">
          <label>默认目标页数</label>
          <input
            type="number"
            min={1}
            value={defaultTarget}
            onChange={(e) => {
              const v = Math.max(1, +e.target.value || 1);
              setDefaultTarget(v);
              // 默认值联动：所有已添加文件的目标页数同步更新
              setRows((prev) => prev.map((r) => ({ ...r, targetPages: v })));
            }}
          />
        </div>
        <div className="spacer" />
        <button className="btn primary big" onClick={runImpose} disabled={busy || rows.length === 0}>
          {busy ? "合成中…" : "🖨 开始合成"}
        </button>
      </div>

      {/* 文件列表 */}
      {rows.length === 0 ? (
        <div className="empty" onClick={addFiles}>
          <div className="empty-icon">📄</div>
          <div>点击添加 PDF 文件，开始拼版</div>
          <div className="empty-sub">支持多选，也可直接把 PDF 拖进本窗口；用 ↑↓ 调整合并顺序</div>
        </div>
      ) : (
        <div className="file-list">
          {rows.map((r, idx) => (
            <div key={r.uid} className="file-card">
              <div className="reorder">
                <button className="btn sm" disabled={idx === 0} onClick={() => move(idx, -1)} title="上移">
                  ↑
                </button>
                <button
                  className="btn sm"
                  disabled={idx === rows.length - 1}
                  onClick={() => move(idx, 1)}
                  title="下移"
                >
                  ↓
                </button>
              </div>
              <div className="file-index">{idx + 1}</div>
              <div className="file-main">
                <div className="file-name" title={r.path}>
                  {r.name}
                </div>
                <div className="file-meta">
                  原 {r.pages} 页 · {r.widthMm}×{r.heightMm}mm
                </div>
              </div>
              <div className="file-controls">
                <button
                  className="btn preview-btn"
                  onClick={() => previewFile(r)}
                  disabled={busy}
                  title="按当前设置预览该文件"
                >
                  👁 预览
                </button>
                <div className="field">
                  <label>目标页数</label>
                  <input
                    type="number"
                    min={1}
                    value={r.targetPages}
                    onChange={(e) => patch(r.uid, { targetPages: Math.max(1, +e.target.value || 1) })}
                  />
                </div>
                <div className="field">
                  <label>旋转</label>
                  <select value={r.rotate} onChange={(e) => patch(r.uid, { rotate: +e.target.value })}>
                    {ROTATIONS.map((a) => (
                      <option key={a} value={a}>
                        {a === 0 ? "不旋转" : `${a}°`}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="field">
                  <label>裁剪</label>
                  <select
                    value={r.cropMode}
                    onChange={(e) => patch(r.uid, { cropMode: e.target.value as FileRow["cropMode"] })}
                  >
                    <option value="none">不裁剪</option>
                    <option value="auto">自动去白边</option>
                    <option value="manual">手动裁剪</option>
                  </select>
                </div>
                {r.cropMode === "auto" && (
                  <div className="field">
                    <label>留白 mm</label>
                    <input
                      type="number"
                      min={0}
                      step={0.5}
                      value={r.autoMarginMm}
                      onChange={(e) => patch(r.uid, { autoMarginMm: Math.max(0, +e.target.value || 0) })}
                    />
                  </div>
                )}
                {r.cropMode === "manual" && (
                  <div className="manual-crop">
                    {(["topMm", "rightMm", "bottomMm", "leftMm"] as const).map((k) => (
                      <div className="field mini" key={k}>
                        <label>{{ topMm: "上", rightMm: "右", bottomMm: "下", leftMm: "左" }[k]} mm</label>
                        <input
                          type="number"
                          min={0}
                          value={r.manual[k]}
                          onChange={(e) =>
                            patch(r.uid, { manual: { ...r.manual, [k]: Math.max(0, +e.target.value || 0) } })
                          }
                        />
                      </div>
                    ))}
                  </div>
                )}
                <button className="btn danger" onClick={() => remove(r.uid)} title="移除">
                  ✕
                </button>
              </div>
            </div>
          ))}
          <SequenceBar rows={rows} sepPages={sepPages} />
        </div>
      )}

      {/* 全局设置 */}
      <div className="global-opts">
        <div className="field">
          <label>分隔页数量 / 文件</label>
          <input type="number" min={0} value={sepPages} onChange={(e) => setSepPages(Math.max(0, +e.target.value || 0))} />
        </div>
        {sepPages > 0 && (
          <div className="field grow">
            <label>分隔页文字</label>
            <input value={sepText} onChange={(e) => setSepText(e.target.value)} />
          </div>
        )}
        <div className="field">
          <label title="把每页等比缩放并居中到目标纸张；方向按每页自动跟随。">
            输出纸张
          </label>
          <select value={paper} onChange={(e) => setPaper(e.target.value as PaperKey)}>
            <option value="none">保持原尺寸</option>
            <option value="a4">A4</option>
            <option value="a3">A3</option>
          </select>
        </div>
        <div className="field">
          <label title="在每个文件的内容页右下角盖页码 1,2,3…（每个文件都从 1 重新开始，分隔页不编号）">
            页码
            <input
              type="checkbox"
              checked={pageNum}
              onChange={(e) => setPageNum(e.target.checked)}
              style={{ marginLeft: 6, verticalAlign: "middle" }}
            />
          </label>
        </div>
        <div className="field grow">
          <label title="保存到第一个源文件所在文件夹，重名自动加 (1) 后缀">输出文件名</label>
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
          ✅ 合成完成：{result.pages} 页 · {(result.sizeBytes / 1024 / 1024).toFixed(1)} MB ·{" "}
          {(result.elapsedMs / 1000).toFixed(2)}s
          <div className="banner-path">{result.path}</div>
        </div>
      )}
      {error && <div className="banner err">❌ {error}</div>}

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
