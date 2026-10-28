import { useEffect, useState } from "react";
import { Button, Empty, Modal, Space, Tag, Tooltip, Typography } from "antd";
import {
  DeleteOutlined,
  ExportOutlined,
  FilePdfOutlined,
  FileTextOutlined,
  FolderOpenOutlined,
  HistoryOutlined,
} from "@ant-design/icons";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, errorText, type PrintHistoryEntry } from "../api";

const { Text } = Typography;

/** 最近生成记录：按钮唤起居中弹窗，点条目直接用 WPS / 系统默认程序打开。 */
export default function HistoryButton({ compact = false }: { compact?: boolean }) {
  const [open, setOpen] = useState(false);
  const [rows, setRows] = useState<PrintHistoryEntry[]>([]);
  const [loading, setLoading] = useState(false);

  const load = () => {
    setLoading(true);
    api
      .listPrintHistory(100)
      .then(setRows)
      .catch(() => setRows([]))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    if (open) load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const openFile = async (path: string) => {
    try {
      if (/\.pdf$/i.test(path)) {
        await api.openPdfInWps(path);
        return;
      }
    } catch {
      // WPS 打不开就退回系统默认程序
    }
    await openPath(path);
  };

  /** 今天只显示时分，更早的带上月日 */
  const fmtTime = (iso: string) => {
    const d = new Date(iso.replace(" ", "T"));
    if (Number.isNaN(d.getTime())) return iso;
    const hh = String(d.getHours()).padStart(2, "0");
    const mm = String(d.getMinutes()).padStart(2, "0");
    const today = new Date().toDateString() === d.toDateString();
    if (today) return `今天 ${hh}:${mm}`;
    const mo = String(d.getMonth() + 1).padStart(2, "0");
    const dd = String(d.getDate()).padStart(2, "0");
    return `${mo}-${dd} ${hh}:${mm}`;
  };

  const isPdf = (name: string) => /\.pdf$/i.test(name);

  return (
    <>
      <Button
        icon={<HistoryOutlined />}
        size={compact ? "small" : "middle"}
        onClick={() => setOpen(true)}
      >
        历史
      </Button>

      <Modal
        title={
          <Space size={8}>
            <span>最近生成</span>
            <Text type="secondary" style={{ fontSize: 12, fontWeight: 400 }}>
              {rows.length ? `最近 ${rows.length} 条` : ""}
            </Text>
          </Space>
        }
        open={open}
        onCancel={() => setOpen(false)}
        width={720}
        style={{ top: 72 }}
        styles={{ body: { maxHeight: "58vh", overflowY: "auto", padding: "4px 8px 8px" } }}
        footer={
          <Space>
            <Button onClick={load} loading={loading}>
              刷新
            </Button>
            <Button
              danger
              onClick={async () => {
                if (!confirm("清空全部生成记录？（只删记录，不删文件）")) return;
                await api.clearPrintHistory();
                load();
              }}
            >
              清空历史
            </Button>
            <Button type="primary" onClick={() => setOpen(false)}>
              关闭
            </Button>
          </Space>
        }
      >
        {rows.length === 0 && !loading ? (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={<span style={{ fontSize: 13 }}>还没有生成记录</span>}
            style={{ padding: "32px 0" }}
          />
        ) : (
          <div className="hist-list">
            {rows.map((r) => (
              <div key={r.id} className="hist-item">
                <span className={`hist-icon ${isPdf(r.outputName) ? "pdf" : "txt"}`}>
                  {isPdf(r.outputName) ? <FilePdfOutlined /> : <FileTextOutlined />}
                </span>
                <div className="hist-main">
                  <div className="hist-name" title={r.outputName}>
                    {r.outputName}
                  </div>
                  <div className="hist-meta">
                    <span>{fmtTime(r.createdAt)}</span>
                    {r.outputPages > 0 && (
                      <>
                        <span className="dot" />
                        <Tag color="blue" bordered={false} style={{ margin: 0, fontSize: 11 }}>
                          {r.outputPages} 页
                        </Tag>
                      </>
                    )}
                    {r.outputPath && (
                      <Tooltip title={r.outputPath}>
                        <span className="hist-path">{r.outputPath}</span>
                      </Tooltip>
                    )}
                  </div>
                </div>
                <div className="hist-actions">
                  <Button
                    type="text"
                    size="small"
                    icon={<ExportOutlined />}
                    disabled={!r.outputPath}
                    title="打开文件"
                    onClick={() =>
                      r.outputPath && openFile(r.outputPath).catch((e) => alert(errorText(e)))
                    }
                  />
                  <Button
                    type="text"
                    size="small"
                    icon={<FolderOpenOutlined />}
                    disabled={!r.outputPath}
                    title="在文件夹中显示"
                    onClick={() => r.outputPath && revealItemInDir(r.outputPath).catch(() => {})}
                  />
                  <Button
                    type="text"
                    size="small"
                    danger
                    icon={<DeleteOutlined />}
                    title="删除这条记录（不删文件）"
                    onClick={async () => {
                      await api.deletePrintHistory(r.id);
                      load();
                    }}
                  />
                </div>
              </div>
            ))}
          </div>
        )}
      </Modal>
    </>
  );
}
