import { CopyOutlined } from "@ant-design/icons";

/** 一个客户的汇总行（跟着当前筛选走） */
export type CustomerStat = {
  customer: string;
  boxes: number;
  rows: number;
  done: number;
  left: number;
  tickets: Set<string>;
};

type Props = {
  /** 已经按件数排好序的客户汇总 */
  stats: CustomerStat[];
  /** 总件数（占比分母，也是合计行的大字） */
  total: number;
  /** 当前是否带筛选（带了就提醒一句「只统计筛出来的」） */
  filtered: boolean;
  /** 筛出来的行数（提醒文案用） */
  shownRows: number;
  onCopy: () => void;
  onClose: () => void;
};

/** 客户件数统计弹窗：纯展示，数据由页面算好传进来。 */
export function StatsModal({ stats, total, filtered, shownRows, onCopy, onClose }: Props) {
  const tickets = stats.reduce((n, c) => n + c.tickets.size, 0);
  const rows = stats.reduce((n, c) => n + c.rows, 0);
  const done = stats.reduce((n, c) => n + c.done, 0);
  const left = stats.reduce((n, c) => n + c.left, 0);

  return (
    <div className="modal-mask" onClick={onClose}>
      <div className="modal stats-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <span>
            📊 客户件数{" "}
            <span className="muted">
              · {stats.length} 个客户 · 共 {total} 件
            </span>
          </span>
          <span className="spacer" />
          <button className="btn sm" onClick={onCopy} disabled={stats.length === 0}>
            <CopyOutlined />
            复制
          </button>
          <button className="btn sm" onClick={onClose}>
            关闭
          </button>
        </div>
        <div className="modal-body">
          {filtered && (
            <div className="stats-note">
              只统计当前筛选出来的 {shownRows} 行（点「统计」前清掉筛选就是全部）
            </div>
          )}
          <table className="hist-table stats-table">
            <thead>
              <tr>
                <th style={{ width: 40 }}>#</th>
                <th>客户</th>
                <th className="num">票数</th>
                <th className="num">款数</th>
                <th className="num">总件数</th>
                <th className="num">已出</th>
                <th className="num">未出</th>
                <th style={{ width: 140 }}>占比</th>
              </tr>
            </thead>
            <tbody>
              {stats.map((c, i) => {
                const pct = total ? Math.round((c.boxes / total) * 100) : 0;
                return (
                  <tr key={c.customer}>
                    <td className="muted">{i + 1}</td>
                    <td>{c.customer}</td>
                    <td className="num">{c.tickets.size}</td>
                    <td className="num">{c.rows}</td>
                    <td className="num">
                      <b>{c.boxes}</b>
                    </td>
                    <td className={`num ${c.done ? "ok" : "muted"}`}>{c.done}</td>
                    <td className={`num ${c.left ? "warn" : "muted"}`}>{c.left}</td>
                    <td>
                      <span className="stats-bar">
                        <i style={{ width: `${pct}%` }} />
                      </span>
                      <span className="muted stats-pct">{pct}%</span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
            <tfoot>
              <tr>
                <td />
                <td>合计</td>
                <td className="num">{tickets}</td>
                <td className="num">{rows}</td>
                <td className="num">
                  <b>{total}</b>
                </td>
                <td className="num ok">{done}</td>
                <td className="num warn">{left}</td>
                <td />
              </tr>
            </tfoot>
          </table>
        </div>
      </div>
    </div>
  );
}
