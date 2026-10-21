import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, errorText, type PrintHistoryEntry } from "../api";

export default function HistoryPage() {
  const [rows, setRows] = useState<PrintHistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);

  const reload = () => {
    setLoading(true);
    api
      .listPrintHistory(200)
      .then(setRows)
      .finally(() => setLoading(false));
  };

  useEffect(reload, []);

  const fmtTime = (iso: string) => iso.replace("T", " ").slice(0, 19);

  return (
    <div className="page">
      <div className="toolbar">
        <button className="btn" onClick={reload}>
          ↻ 刷新
        </button>
        <div className="spacer" />
        <button
          className="btn danger"
          onClick={async () => {
            if (confirm("确定清空全部生成历史？")) {
              await api.clearPrintHistory();
              reload();
            }
          }}
        >
          清空历史
        </button>
      </div>

      {loading ? (
        <div className="empty">加载中…</div>
      ) : rows.length === 0 ? (
        <div className="empty">
          <div className="empty-icon">🕘</div>
          <div>还没有生成记录</div>
          <div className="empty-sub">拼版 / 箱唛生成后会自动记录到这里</div>
        </div>
      ) : (
        <table className="hist-table">
          <thead>
            <tr>
              <th>时间</th>
              <th>输出文件</th>
              <th>页数</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.id}>
                <td className="mono">{fmtTime(r.createdAt)}</td>
                <td title={r.outputPath ?? ""}>{r.outputName}</td>
                <td>{r.outputPages}</td>
                <td>
                  <div className="row-actions">
                    {r.outputPath && (
                      <button
                        className="btn sm"
                        title="在文件夹中显示"
                        onClick={() =>
                          revealItemInDir(r.outputPath!).catch((e) => alert(`打开失败：${errorText(e)}\n（文件可能已被移动或删除）`))
                        }
                      >
                        📂 打开文件夹
                      </button>
                    )}
                    <button
                      className="btn danger sm"
                      onClick={async () => {
                        await api.deletePrintHistory(r.id);
                        reload();
                      }}
                    >
                      删除
                    </button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
