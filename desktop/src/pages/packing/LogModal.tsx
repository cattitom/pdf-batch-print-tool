import { useMemo, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { confirm as confirmDialog } from "@tauri-apps/plugin-dialog";
import { api, errorHint, errorText } from "../../api";

/** 日志行结构化：`2026-10-28 08:24:26.745 INFO  desktop_lib: 消息` */
export interface LogRow {
  time: string;
  level: string;
  target: string;
  message: string;
}

// target 里可能带 `::`（如 tao::platform_impl::…），所以取第一个「: 」作为分隔
const LOG_LINE = /^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3})\s+(\w+)\s+(.*?):\s([\s\S]*)$/;

/** 把日志文本切成结构化行；续行（消息里带换行）挂回上一条 */
export function parseLog(text: string): LogRow[] {
  const rows: LogRow[] = [];
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const m = line.match(LOG_LINE);
    if (m) {
      rows.push({ time: m[1], level: m[2], target: m[3], message: m[4] });
    } else if (rows.length > 0) {
      rows[rows.length - 1].message += `\n${line}`;
    }
  }
  return rows;
}

type Props = {
  /** 原始日志文本；null 表示弹窗关着（调用方用 logText !== null 控制） */
  text: string;
  busy: boolean;
  onRefresh: () => void;
  onCopy: () => void;
  /** 清空日志后把最新文本塞回来（页面那边 setLogText） */
  onText: (text: string) => void;
  onError: (message: string) => void;
  onClose: () => void;
};

const LEVELS = ["全部", "INFO", "WARN", "ERROR", "DEBUG"];

/** 日志弹窗：结构化 / 原始两种视图 + 关键词与级别筛选。 */
export function LogModal({ text, busy, onRefresh, onCopy, onText, onError, onClose }: Props) {
  const [view, setView] = useState<"structured" | "raw">("structured");
  const [query, setQuery] = useState("");
  const [level, setLevel] = useState("全部");

  const rows = useMemo(() => parseLog(text), [text]);
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return rows.filter((r) => {
      if (level !== "全部" && r.level !== level) return false;
      if (!q) return true;
      return (
        r.message.toLowerCase().includes(q) ||
        r.target.toLowerCase().includes(q) ||
        r.level.toLowerCase().includes(q)
      );
    });
  }, [rows, query, level]);

  const clear = async () => {
    const ok = await confirmDialog("清空当前日志？清空后无法恢复。", {
      title: "清空日志",
      kind: "warning",
    });
    if (!ok) return;
    try {
      await api.clearLog();
      onText(await api.readLogTail(500));
    } catch (e) {
      onError(errorText(e) + errorHint(e));
    }
  };

  const openDir = async () => {
    try {
      await revealItemInDir(await api.logDir());
    } catch (e) {
      onError(errorText(e));
    }
  };

  return (
    <div className="modal-mask" onClick={onClose}>
      <div className="modal packing-log-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <span>
            📄 日志 <span className="muted">· {rows.length} 条</span>
          </span>
          <span className="spacer" />
          <div className="log-view-toggle">
            <button
              className={`btn sm ${view === "structured" ? "primary" : ""}`}
              onClick={() => setView("structured")}
            >
              结构化
            </button>
            <button className={`btn sm ${view === "raw" ? "primary" : ""}`} onClick={() => setView("raw")}>
              原始
            </button>
          </div>
          <button className="btn sm" onClick={onRefresh} disabled={busy}>
            刷新
          </button>
          <button className="btn sm danger" onClick={clear}>
            清空
          </button>
          <button className="btn sm" onClick={onCopy}>
            复制
          </button>
          <button className="btn sm" onClick={openDir}>
            打开目录
          </button>
          <button className="btn sm" onClick={onClose}>
            关闭
          </button>
        </div>

        {view === "structured" && (
          <div className="log-filterbar">
            <input
              className="log-search"
              placeholder="搜索关键词（如「网格」「OCR」「ERROR」）"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            {LEVELS.map((lv) => (
              <button
                key={lv}
                className={`btn sm ${level === lv ? "primary" : ""}`}
                onClick={() => setLevel(lv)}
              >
                {lv}
              </button>
            ))}
            <span className="spacer" />
            <span className="muted">
              显示 {filtered.length} / {rows.length} 条
            </span>
          </div>
        )}

        <div className="modal-body">
          {view === "raw" ? (
            <pre className="packing-log">{text || "（日志为空）"}</pre>
          ) : filtered.length === 0 ? (
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
                  {filtered.map((r, i) => (
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
  );
}
