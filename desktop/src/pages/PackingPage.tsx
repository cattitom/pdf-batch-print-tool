import { useEffect, useMemo, useRef, useState } from "react";
import { confirm as confirmDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, errorHint, errorText } from "../api";
import type { PackingItemDto } from "../bindings/PackingItemDto";
import type { PackingSheetDto } from "../bindings/PackingSheetDto";

/**
 * 录货清单：粘贴 Excel 表格 / 拖 xlsx·csv / 拖截图，自动拆成「目的地 + 单号」分组，
 * 结果可逐格改、可勾选，最后导出 Markdown 清单。
 */

const ISSUE_TEXT: Record<string, string> = {
  name: "商品名空",
  qty: "数量没读到",
  inbound: "入库号空",
  inbound_sci: "入库号像数值",
  inbound_inherit: "入库号沿用上行",
  dest_inherit: "目的地沿用上行",
  mark_text: "标记列含其它字",
  dup_mark: "去重后保留",
};

const IMAGE_EXT = /\.(png|jpe?g|webp|bmp|gif)$/i;

function emptyRow(seq: number): PackingItemDto {
  return {
    seq,
    name: "",
    qty: 0,
    qtyRaw: "",
    inbound: "",
    destination: "",
    waybill: "",
    checked: false,
    issues: [],
  };
}

/** 按「目的地 + 单号」分组，生成可勾选的 Markdown 清单。 */
function toMarkdown(rows: PackingItemDto[]): string {
  const groups = new Map<string, PackingItemDto[]>();
  for (const row of rows) {
    const dest = row.destination.trim() || "未填目的地";
    const waybill = row.waybill.trim() || "未填单号";
    const key = `${dest}\u0000${waybill}`;
    const list = groups.get(key);
    if (list) list.push(row);
    else groups.set(key, [row]);
  }
  if (groups.size === 0) return "";

  const out: string[] = ["# 装箱单录货待办", ""];
  for (const [key, list] of groups) {
    const [dest, waybill] = key.split("\u0000");
    out.push(`## 目的地：${dest} | 单号：${waybill}`, "");
    for (const row of list) {
      const box = row.checked ? "x" : " ";
      out.push(
        `- [${box}] ${row.name} | 数量：${row.qty} | 入库号：${row.inbound} | 单号：${waybill}`
      );
    }
    out.push("");
  }
  return out.join("\n").trimEnd() + "\n";
}

/** 日志行结构化：`2026-10-28 08:24:26.745 INFO  desktop_lib: 消息` */
interface LogRow {
  time: string;
  level: string;
  target: string;
  message: string;
}

// target 里可能带 `::`（如 tao::platform_impl::…），所以取第一个「: 」作为分隔
const LOG_LINE =
  /^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3})\s+(\w+)\s+(.*?):\s([\s\S]*)$/;

function parseLog(text: string): LogRow[] {
  const rows: LogRow[] = [];
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const m = line.match(LOG_LINE);
    if (m) {
      rows.push({ time: m[1], level: m[2], target: m[3], message: m[4] });
    } else if (rows.length > 0) {
      // 续行（消息里带换行时）挂回上一条
      rows[rows.length - 1].message += `\n${line}`;
    }
  }
  return rows;
}

/** 包一层「用户改过的行」：编辑后重新算 md，不回后端。 */
export default function PackingPage({
  pendingDrop,
}: {
  pendingDrop?: { paths: string[]; nonce: number } | null;
}) {
  const [text, setText] = useState("");
  const [rows, setRows] = useState<PackingItemDto[]>([]);
  const [sheet, setSheet] = useState<PackingSheetDto | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedPath, setSavedPath] = useState<string | null>(null);
  const [copyHint, setCopyHint] = useState<string | null>(null);
  /** 结果来自截图 OCR：必须提示人工核对（系统 OCR 会读丢个别格子） */
  const [fromOcr, setFromOcr] = useState(false);
  /** 日志弹窗内容；null = 不显示 */
  const [logText, setLogText] = useState<string | null>(null);
  const [logBusy, setLogBusy] = useState(false);
  const [logView, setLogView] = useState<"structured" | "raw">("structured");
  const [logQuery, setLogQuery] = useState("");
  const [logLevel, setLogLevel] = useState("全部");

  const parseSeq = useRef(0);
  /** 当前结果是从粘贴框来的吗——拖文件后把粘贴框清空，别顺手把结果也清了 */
  const fromText = useRef(false);

  const md = useMemo(() => toMarkdown(rows), [rows]);
  const checkedCount = rows.filter((r) => r.checked).length;
  const totalQty = rows.reduce((n, r) => n + (Number.isFinite(r.qty) ? r.qty : 0), 0);
  const issueCount = rows.filter((r) => r.issues.length > 0).length;

  const logRows = useMemo(() => parseLog(logText ?? ""), [logText]);
  const filteredLog = useMemo(() => {
    const q = logQuery.trim().toLowerCase();
    return logRows.filter((r) => {
      if (logLevel !== "全部" && r.level !== logLevel) return false;
      if (!q) return true;
      return (
        r.message.toLowerCase().includes(q) ||
        r.target.toLowerCase().includes(q) ||
        r.level.toLowerCase().includes(q)
      );
    });
  }, [logRows, logQuery, logLevel]);

  const applySheet = (s: PackingSheetDto) => {
    setSheet(s);
    setRows(s.items);
  };

  /** 粘贴：停手 350ms 再解析，避免每敲一个字都跑一次。 */
  useEffect(() => {
    const body = text.trim();
    if (body === "") {
      if (fromText.current) {
        setSheet(null);
        setRows([]);
        fromText.current = false;
      }
      return;
    }
    const seq = ++parseSeq.current;
    const timer = setTimeout(async () => {
      try {
        setBusy(true);
        const s = await api.parsePackingText(body);
        if (seq === parseSeq.current) {
          applySheet(s);
          fromText.current = true;
          setFromOcr(false);
          setError(null);
          setSavedPath(null);
        }
      } catch (e) {
        if (seq === parseSeq.current) setError(errorText(e) + errorHint(e));
      } finally {
        if (seq === parseSeq.current) setBusy(false);
      }
    }, 350);
    return () => clearTimeout(timer);
  }, [text]);

  /** 拖进来的文件：图片走系统 OCR，xlsx / csv / txt 直接解析。 */
  useEffect(() => {
    const path = pendingDrop?.paths?.[0];
    if (!path) return;
    const isImage = IMAGE_EXT.test(path);
    let cancelled = false;
    (async () => {
      try {
        setBusy(true);
        const s = isImage ? await api.parsePackingImage(path) : await api.parsePackingFile(path);
        if (cancelled) return;
        applySheet(s);
        setFromOcr(isImage);
        fromText.current = false;
        setText("");
        setError(null);
        setSavedPath(null);
      } catch (e) {
        if (!cancelled) setError(errorText(e) + errorHint(e));
      } finally {
        if (!cancelled) setBusy(false);
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingDrop?.nonce]);

  const patch = (i: number, p: Partial<PackingItemDto>) => {
    setRows((prev) => prev.map((r, j) => (j === i ? { ...r, ...p } : r)));
    setSavedPath(null);
  };

  const copyMd = async () => {
    if (!md) return;
    try {
      await navigator.clipboard.writeText(md);
      setCopyHint("已复制");
    } catch {
      // WebView 里 clipboard 权限可能没有：退回到选中 textarea
      const ta = document.createElement("textarea");
      ta.value = md;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
      setCopyHint("已复制");
    }
    setTimeout(() => setCopyHint(null), 1800);
  };

  const saveMd = async () => {
    if (!md) return;
    try {
      const day = new Date().toISOString().slice(0, 10);
      const out = await saveFileDialog({
        defaultPath: `装箱单待办_${sheet?.waybill || day}.md`,
        filters: [{ name: "Markdown", extensions: ["md"] }],
      });
      if (!out) return;
      await api.saveTextFile(out, md);
      setSavedPath(out);
      setError(null);
    } catch (e) {
      setError(errorText(e) + errorHint(e));
    }
  };

  const openLog = async () => {
    setLogBusy(true);
    try {
      setLogText(await api.readLogTail(500));
    } catch (e) {
      setLogText(`读取日志失败：${errorText(e)}${errorHint(e)}`);
    } finally {
      setLogBusy(false);
    }
  };

  const copyLog = async () => {
    if (!logText) return;
    try {
      await navigator.clipboard.writeText(logText);
      setCopyHint("日志已复制");
      setTimeout(() => setCopyHint(null), 1800);
    } catch {
      setError("复制失败，可以点「打开日志目录」直接看文件");
    }
  };

  return (
    <div className="page">
      <div className="toolbar">
        <span className="muted">
          把 Excel 里整块表格复制粘进下面的框，或把 xlsx / csv / <b>截图（png、jpg）</b>拖进窗口
          {busy ? " · 解析中…" : ""}
        </span>
        <span className="spacer" />
        <button className="btn sm" onClick={openLog} disabled={logBusy} title="查看最近的程序日志">
          {logBusy ? "读取中…" : "📄 日志"}
        </button>
        {rows.length > 0 && (
          <span className="muted">
            共 {rows.length} 条 · 合计 {totalQty} 件
            {sheet?.expectedQty != null && sheet.expectedQty !== totalQty ? (
              <b className="warn-text"> · 图上 {sheet.expectedQty} 件</b>
            ) : sheet?.expectedQty != null ? (
              " · 与图上一致"
            ) : null}
            {issueCount > 0 ? ` · ${issueCount} 行要确认` : ""}
          </span>
        )}
      </div>

      <div className="form-grid">
        <div className="boxplan-title">粘贴表格</div>
        <div className="field grow">
          <label>
            支持从 Excel 直接复制（制表符分隔）；带表头会自动按表头认列，没表头就按截图模板的列序。
            拖图片进来则走系统 OCR。
          </label>
          <textarea
            className="boxplan-textarea mono packing-paste"
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={
              "门铰链标准 20/件\t52\t37\t34\t10.90\t14.3\t20\t\tEXX\t美西\tIN5LK113218\n" +
              "鱼鳞布10条装 100/件\t51\t36\t46\t14.07\t19.05\t5\t\tEXX\t美西\tIN5LK113223"
            }
            spellCheck={false}
          />
        </div>
        <div className="toolbar">
          {sheet?.mapping && <span className="muted">{sheet.mapping}</span>}
          <span className="spacer" />
          <button
            className="btn sm"
            onClick={() => {
              setText("");
              setSheet(null);
              setRows([]);
              setError(null);
              setSavedPath(null);
              setFromOcr(false);
            }}
            disabled={rows.length === 0 && text === ""}
          >
            清空
          </button>
        </div>
      </div>

      {fromOcr && rows.length > 0 && (
        <div className="banner warn">
          🔍 这结果来自截图 OCR（{sheet?.engine === "paddle" ? "PaddleOCR，离线运行" : "Windows 自带引擎"}）：
          {sheet?.engine === "paddle"
            ? "文字和数字基本都能读到，仍建议扫一眼标黄行（多为合并单元格的入库号）。"
            : "没装 PaddleOCR 模型时会读丢个别格子，标黄行必须对照原图改一下。"}
        </div>
      )}

      {sheet?.warnings?.length ? (
        <div className="banner warn">
          ⚠️ {sheet.warnings.join("；")}
        </div>
      ) : null}

      {rows.length > 0 && (
        <div className="form-grid">
          <div className="boxplan-title">
            解析结果（可直接改；勾选框 = 原表标记列里有 1，即已录过）
          </div>
          <div className="boxplan-table-wrap">
            <table className="hist-table boxplan-input-table packing-table">
              <thead>
                <tr>
                  <th style={{ width: 44 }}>已录</th>
                  <th style={{ width: 40 }}>#</th>
                  <th>商品名称</th>
                  <th style={{ width: 76 }}>数量</th>
                  <th style={{ width: 150 }}>入库号</th>
                  <th style={{ width: 90 }}>目的地</th>
                  <th style={{ width: 120 }}>单号</th>
                  <th style={{ width: 96 }}>提示</th>
                  <th style={{ width: 44 }}></th>
                </tr>
              </thead>
              <tbody>
                {rows.map((row, i) => (
                  <tr key={i} className={row.issues.length > 0 ? "packing-row-warn" : ""}>
                    <td style={{ textAlign: "center" }}>
                      <input
                        type="checkbox"
                        checked={row.checked}
                        onChange={(e) => patch(i, { checked: e.target.checked })}
                      />
                    </td>
                    <td className="muted" style={{ textAlign: "center" }}>
                      {i + 1}
                    </td>
                    <td>
                      <input
                        value={row.name}
                        onChange={(e) => patch(i, { name: e.target.value })}
                      />
                    </td>
                    <td>
                      <input
                        type="number"
                        min={0}
                        value={row.qty}
                        onChange={(e) => patch(i, { qty: Math.max(0, +e.target.value || 0) })}
                      />
                    </td>
                    <td>
                      <input
                        className="mono"
                        value={row.inbound}
                        onChange={(e) => patch(i, { inbound: e.target.value })}
                      />
                    </td>
                    <td>
                      <input
                        value={row.destination}
                        onChange={(e) => patch(i, { destination: e.target.value })}
                      />
                    </td>
                    <td>
                      <input
                        className="mono"
                        value={row.waybill}
                        onChange={(e) => patch(i, { waybill: e.target.value })}
                      />
                    </td>
                    <td>
                      {row.issues.length > 0 ? (
                        <span className="issue-tag" title={row.issues.join("、")}>
                          {row.issues.map((c) => ISSUE_TEXT[c] ?? c).join("、")}
                        </span>
                      ) : (
                        <span className="muted">✓</span>
                      )}
                    </td>
                    <td style={{ textAlign: "center" }}>
                      <button
                        className="btn danger sm row-del"
                        title="删除这一行（不进清单，比如表头/小计/不是本单的货）"
                        onClick={() => setRows((prev) => prev.filter((_, j) => j !== i))}
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
            <button className="btn" onClick={() => setRows((prev) => [...prev, emptyRow(prev.length + 1)])}>
              ＋ 添加一行
            </button>
            <button
              className="btn danger sm"
              onClick={() =>
                setRows((prev) => (prev.length > 1 ? prev.slice(0, -1) : prev))
              }
              disabled={rows.length <= 1}
            >
              删除末行
            </button>
            <span className="spacer" />
            <span className="muted">
              已勾选 {checkedCount} 条 · 待录 {rows.length - checkedCount} 条
            </span>
          </div>
        </div>
      )}

      {rows.length > 0 && (
        <div className="form-grid">
          <div className="boxplan-title packing-md-head">
            <span>Markdown 清单（按目的地 + 单号自动分块）</span>
            <span className="spacer" />
            <button className="btn sm" onClick={copyMd}>
              {copyHint ?? "复制"}
            </button>
            <button className="btn sm primary" onClick={saveMd}>
              另存为 .md
            </button>
          </div>
          <textarea className="boxplan-textarea mono packing-md" value={md} readOnly />
        </div>
      )}

      {savedPath && <div className="banner ok">✅ 已保存：{savedPath}</div>}
      {error && <div className="banner err">❌ {error}</div>}

      {logText !== null && (
        <div className="modal-mask" onClick={() => setLogText(null)}>
          <div className="modal packing-log-modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-head">
              <span>
                📄 日志 <span className="muted">· {logRows.length} 条</span>
              </span>
              <span className="spacer" />
              <div className="log-view-toggle">
                <button
                  className={`btn sm ${logView === "structured" ? "primary" : ""}`}
                  onClick={() => setLogView("structured")}
                >
                  结构化
                </button>
                <button
                  className={`btn sm ${logView === "raw" ? "primary" : ""}`}
                  onClick={() => setLogView("raw")}
                >
                  原始
                </button>
              </div>
              <button className="btn sm" onClick={openLog} disabled={logBusy}>
                刷新
              </button>
              <button
                className="btn sm danger"
                onClick={async () => {
                  const ok = await confirmDialog("清空当前日志？清空后无法恢复。", {
                    title: "清空日志",
                    kind: "warning",
                  });
                  if (!ok) return;
                  try {
                    await api.clearLog();
                    setLogText(await api.readLogTail(500));
                  } catch (e) {
                    setError(errorText(e) + errorHint(e));
                  }
                }}
              >
                清空
              </button>
              <button className="btn sm" onClick={copyLog}>
                复制
              </button>
              <button
                className="btn sm"
                onClick={async () => {
                  try {
                    await revealItemInDir(await api.logDir());
                  } catch (e) {
                    setError(errorText(e));
                  }
                }}
              >
                打开目录
              </button>
              <button className="btn sm" onClick={() => setLogText(null)}>
                关闭
              </button>
            </div>

            {logView === "structured" && (
              <div className="log-filterbar">
                <input
                  className="log-search"
                  placeholder="搜索关键词（如「网格」「OCR」「ERROR」）"
                  value={logQuery}
                  onChange={(e) => setLogQuery(e.target.value)}
                />
                {["全部", "INFO", "WARN", "ERROR", "DEBUG"].map((lv) => (
                  <button
                    key={lv}
                    className={`btn sm ${logLevel === lv ? "primary" : ""}`}
                    onClick={() => setLogLevel(lv)}
                  >
                    {lv}
                  </button>
                ))}
                <span className="spacer" />
                <span className="muted">
                  显示 {filteredLog.length} / {logRows.length} 条
                </span>
              </div>
            )}

            <div className="modal-body">
              {logView === "raw" ? (
                <pre className="packing-log">{logText || "（日志为空）"}</pre>
              ) : filteredLog.length === 0 ? (
                <div className="packing-log-empty">没有匹配的日志</div>
              ) : (
                <div className="log-scroll">
                  <table className="hist-table log-table">
                    <thead>
                      <tr>
                        <th style={{ width: 92 }}>时间</th>
                        <th style={{ width: 62 }}>级别</th>
                        <th style={{ width: 140 }}>模块</th>
                        <th>内容</th>
                      </tr>
                    </thead>
                    <tbody>
                      {filteredLog.map((r, i) => (
                        <tr key={i}>
                          <td className="t" title={r.time}>
                            {r.time.slice(11)}
                          </td>
                          <td>
                            <span className={`log-lvl ${r.level}`}>{r.level}</span>
                          </td>
                          <td className="muted">{r.target}</td>
                          <td className="m">{r.message}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
