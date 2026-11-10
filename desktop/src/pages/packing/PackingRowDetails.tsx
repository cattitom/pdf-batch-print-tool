import { isSharedQuantity, quantityText } from "./quantityGroups";
import { Drawer } from "antd";
import type { PackingItemDto } from "../../bindings/PackingItemDto";
import { CustomerMessageEditor } from "./CustomerMessageEditor";
import { customerMessageKey } from "./customerMessage";

export function businessHints(row: PackingItemDto): string[] {
  const amazon = row.issues.includes("special_amazon");
  const shenzhen = row.issues.includes("special_shenzhen");
  const hints: string[] = [];
  if (amazon) hints.push("亚马逊");
  if (shenzhen) hints.push("发深圳");
  if (row.quantityGroup) hints.push(`合箱 ${row.quantityGroup}，共 ${row.qtyRaw} 箱，只计一次`);
  if (row.cargo?.remarks?.trim()) hints.push(row.cargo.remarks.trim());
  if (!row.name.trim()) hints.push("待补商品名");
  if (!amazon && !shenzhen && !row.inbound.trim()) hints.push("待补入库号");
  if (!amazon && !shenzhen && !row.waybill.trim()) hints.push("待补单号");
  if (row.issues.includes("inbound_sci")) hints.push("入库号待核对");
  return hints;
}

const issueLabels: Record<string, string> = {
  name: "商品名称为空，数量仍计入合计",
  qty: "原表数量为空或未识别，请核对原表",
  qty_shared: "与同组合箱商品共用箱数，本行不另计箱数",
  inbound: "原表未填写入库号",
  inbound_sci: "入库号像科学计数或数值，请核对是否丢失位数",
  inbound_inherit: "原表为合并单元格，入库号沿用上一行",
  dest_inherit: "收货仓沿用上一行",
  mark_text: "原表标记列含其他文字",
  dup_mark: "重复标记已合并保留",
  history_archive: "原表中已无此记录，明细来自本地存档",
  waybill_archive: "原表单号为空，已按明细从本地存档恢复",
  special_amazon: "果粒橙的特殊出货记录，统一归入亚马逊单号",
  special_shenzhen: "发深圳特殊出货记录，单独分组，允许免单号出货",
};

export function PackingRowDetails({ row, source, mapping, onClose }: {
  row?: PackingItemDto; source: string; mapping?: string; onClose: () => void;
}) {
  const hints = row ? businessHints(row) : [];
  return <Drawer title="行详情" open={!!row} onClose={onClose} size={460} className="packing-row-details">
    {row && <>
      <div className="packing-detail-title">{row.name.trim() || "待补商品名"}</div>
      <div className="packing-detail-subtitle">{row.customer || "未填写客户"} · {quantityText(row)} · {row.checked ? "已出货" : "未出货"}</div>
      <CustomerMessageEditor key={customerMessageKey(row)} row={row} />
      {row.cargo && <section className="packing-detail-section">
        <h3>原表货物资料</h3>
        <dl>{[
          ["长 cm", row.cargo.length], ["宽 cm", row.cargo.width], ["高 cm", row.cargo.height],
          ["实重 kg/箱", row.cargo.actualWeight], ["原表体积/箱", row.cargo.sourceVolume],
          ["原表备注", row.cargo.remarks],
        ].map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value || "—"}</dd></div>)}</dl>
      </section>}
      <section className="packing-detail-section">
        <h3>货物与出货</h3>
        <dl>{[
          ["客户名称", row.customer], ["商品名称", row.name], ["箱数", isSharedQuantity(row) ? "合箱（本行不另计）" : String(row.qty)],
          ["单号", row.waybill], ["收货仓", row.destination], ["入库号", row.inbound],
          ["到货情况", row.arrival], ["出货状态", row.checked ? "已出货" : "未出货"],
        ].map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value?.trim() || "—"}</dd></div>)}</dl>
      </section>
      <section className="packing-detail-section">
        <h3>业务提示</h3>
        {hints.length ? <div className="packing-detail-hints">{hints.map((hint) => <span key={hint}>{hint}</span>)}</div> : <p className="packing-detail-muted">暂无业务提示</p>}
      </section>
      <section className="packing-detail-section">
        <h3>原表与识别说明</h3>
        <dl><div><dt>数据来源</dt><dd>{source}</dd></div><div><dt>原表行号</dt><dd>{row.sourceRow ? `第 ${row.sourceRow} 行` : "无 Excel 行号"}</dd></div><div><dt>明细序号</dt><dd>{row.seq || "—"}</dd></div><div><dt>原始数量</dt><dd>{row.qtyRaw || "—"}</dd></div></dl>
        {row.quantityGroup && <p className="packing-detail-muted">
          原表 {row.quantityGroup} 合并写了 {row.qtyRaw} 箱，组内所有商品共用这份箱数。
          {isSharedQuantity(row) ? "本行保留商品资料，不重复计入总箱数。" : "共用箱数在本行计入一次。"}
        </p>}
        {row.issues.length > 0 && <ul>{[...new Set(row.issues)].map((issue) => <li key={issue}>{issueLabels[issue] || "其他识别提示，请核对原表"}</li>)}</ul>}
        {mapping && <p className="packing-detail-muted">{mapping}</p>}
      </section>
    </>}
  </Drawer>;
}
