import { Button, Modal } from "antd";

export type ShipImageResult = {
  /** 认出来的入库号个数 */
  total: number;
  /** 在原表里匹配到的行数 */
  matched: number;
  /** 原表里没有的码 */
  unknown: string[];
  /** 没读出内容的图 */
  failed: string[];
};

type Props = {
  paths: string[];
  busy: boolean;
  result: ShipImageResult | null;
  onPick: () => void;
  onRemove: (path: string) => void;
  onRun: () => void;
  onClose: () => void;
};

const baseName = (p: string) => p.split(/[\\/]/).pop() || p;

/**
 * 导入出货截图：拖图 / 选图 → 本地 OCR 认入库号 → 匹配当前表。
 *
 * 拖放走的是 Tauri 的全局 onDragDropEvent（dragDropEnabled=true 时网页层的 drop 事件收不到），
 * 所以文案写「拖到窗口任意位置」，不要求拖进这个框里。
 */
export function ShipImagesModal({ paths, busy, result, onPick, onRemove, onRun, onClose }: Props) {
  return (
    <Modal
      title="导入出货截图"
      open
      onCancel={onClose}
      footer={null}
      width={580}
      destroyOnHidden
    >
      <div className="ship-drop" onClick={onPick} role="button" tabIndex={0}>
        <div className="ship-drop-plus">＋</div>
        <div className="ship-drop-title">把「本次要出货」的截图拖进来</div>
        <div className="ship-drop-sub">
          拖到窗口任意位置都行；也可以点这里选文件（可多选）
        </div>
      </div>

      {paths.length > 0 && (
        <ul className="ship-list">
          {paths.map((p) => (
            <li key={p}>
              <span className="mono">{baseName(p)}</span>
              <button className="link-btn" onClick={() => onRemove(p)} disabled={busy}>
                移除
              </button>
            </li>
          ))}
        </ul>
      )}

      {result && (
        <div className="ship-result">
          <div>
            认出 <b>{result.total}</b> 个入库号，匹配到 <b>{result.matched}</b> 行
          </div>
          {result.unknown.length > 0 && (
            <div className="ship-result-warn">原表没有：{result.unknown.join("、")}</div>
          )}
          {result.failed.length > 0 && (
            <div className="ship-result-warn">没读出来：{result.failed.join("；")}</div>
          )}
        </div>
      )}

      <div className="ship-actions">
        <Button onClick={onClose}>关闭</Button>
        <Button type="primary" onClick={onRun} loading={busy} disabled={paths.length === 0}>
          {busy ? "识别中…" : `开始识别${paths.length ? `（${paths.length} 张）` : ""}`}
        </Button>
      </div>
    </Modal>
  );
}
