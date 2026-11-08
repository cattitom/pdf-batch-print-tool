import { useEffect, useState } from "react";
import { Alert, Button, Input } from "antd";
import { CopyOutlined, SaveOutlined } from "@ant-design/icons";
import { api, errorText } from "../../api";
import type { PackingItemDto } from "../../bindings/PackingItemDto";
import { customerMessageError, customerMessageOverrides, customerMessageKey, customerMessageText, defaultCustomerMessage, parseCustomerMessage, readCustomerMessage, type CustomerMessageData } from "./customerMessage";

export function CustomerMessageEditor({ row }: { row: PackingItemDto }) {
  const [data, setData] = useState(() => defaultCustomerMessage(row));
  const [sourceData, setSourceData] = useState(() => defaultCustomerMessage(row));
  const [loading, setLoading] = useState(true), [loadError, setLoadError] = useState("");
  const [attempt, setAttempt] = useState(0), [saving, setSaving] = useState(false);
  const [status, setStatus] = useState(""), [failure, setFailure] = useState("");
  const [oldMessage, setOldMessage] = useState("");
  const key = customerMessageKey(row);
  useEffect(() => {
    let cancelled = false;
    setLoading(true); setLoadError("");
    Promise.all([api.allSettings(), api.packingCargo(row)]).then(([settings, cached]) => {
      const base = defaultCustomerMessage({ ...row, cargo: { ...cached?.cargo, ...row.cargo } });
      const result = readCustomerMessage(settings[key], base);
      if (cancelled) return;
      setSourceData(base); setData(result.data); setStatus(result.updatedAt ? "已载入上次保存的客户资料" : "已带入 WPS / 本地货物资料，未填写的字段可补充");
    }).catch((e) => { if (!cancelled) setLoadError("读取客户资料失败：" + errorText(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
    // 外层按货物身份重新挂载，避免切行时串用资料。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, attempt]);
  const change = (field: keyof CustomerMessageData, value: string) => {
    setData((current) => ({ ...current, [field]: value })); setStatus("有修改，保存或复制后会记住"); setFailure("");
  };
  const invalid = customerMessageError(data);
  const preview = customerMessageText(data);
  const disabled = loading || !!loadError || saving;
  const persist = async (copy: boolean) => {
    if (disabled || invalid) return;
    if (!data.customer.trim() || !data.product.trim()) { setFailure("请先填写客户和商品名称"); return; }
    setSaving(true); setFailure("");
    try {
      await api.setSetting(key, JSON.stringify({ schema: 2, updatedAt: new Date().toISOString(), overrides: customerMessageOverrides(data, sourceData) }));
      if (copy) {
        try { await navigator.clipboard.writeText(preview); }
        catch {
          const input = document.createElement("textarea");
          input.value = preview; input.style.position = "fixed"; input.style.opacity = "0";
          const parent = document.querySelector(".packing-row-details .ant-drawer-body") || document.body;
          parent.appendChild(input); input.select();
          const copied = document.execCommand("copy"); input.remove();
          if (!copied) throw new Error("未能写入剪贴板，可选中预览文字复制");
        }
      }
      setStatus(copy ? "已保存并复制，可直接粘贴发给客户" : "客户资料已保存，下次打开仍可复制");
    } catch (e) { setFailure("操作未完成：" + errorText(e)); }
    finally { setSaving(false); }
  };
  const field = (name: keyof CustomerMessageData, label: string, numeric = false) =>
    <label className="packing-message-field"><span>{label}</span><Input value={data[name]} disabled={disabled}
      inputMode={numeric ? "decimal" : undefined} onChange={(e) => change(name, e.target.value)} aria-label={label} /></label>;
  return <section className="packing-detail-section packing-customer-message">
    <h3>发客户资料</h3>
    {loadError && <Alert type="error" title={loadError} action={<Button size="small" onClick={() => setAttempt((v) => v + 1)}>重试</Button>} />}
    <div className="packing-message-source"><span>原表数据自动填入，手动修订优先</span><Button type="link" size="small" disabled={disabled} onClick={() => { setData(sourceData); setFailure(""); setStatus("已恢复原表资料，保存后生效"); }}>恢复原表数据</Button></div>
    <div className="packing-message-fields">
      {field("customer", "客户名称")}{field("inbound", "入库号")}
      {field("product", "商品描述")}{field("packages", "总件数（箱）", true)}
    </div>
    <div className="packing-message-sizes">
      {field("length", "长 cm", true)}{field("width", "宽 cm", true)}{field("height", "高 cm", true)}
    </div>
    <div className="packing-message-fields">{field("actualWeight", "实重 kg/箱", true)}<div className="packing-message-formula">体积重 = 长 × 宽 × 高 ÷ 6000<br />自动计算，保留最多 3 位小数</div></div>
    <details className="packing-message-import"><summary>粘贴以前发过的消息</summary>
      <Input.TextArea rows={5} value={oldMessage} disabled={disabled} onChange={(e) => setOldMessage(e.target.value)} aria-label="以前发过的货物消息" placeholder="粘贴客户、入库号、规格、尺寸和实重…" />
      <Button size="small" disabled={disabled || !oldMessage.trim()} onClick={() => {
        try {
          const parsed = parseCustomerMessage(oldMessage);
          setData((current) => ({ ...current, ...parsed })); setFailure("");
          setStatus("已识别并填入，请核对后保存");
        } catch (e) { setFailure(errorText(e)); }
      }}>解析并填入</Button>
    </details>
    <div className="packing-message-preview-label">发送预览</div>
    <Input.TextArea readOnly autoSize={{ minRows: 5, maxRows: 12 }} value={preview} aria-label="客户消息预览" />
    {(invalid || failure) && <div className="packing-message-error" role="alert">{invalid || failure}</div>}
    <div className="packing-message-actions">
      <Button icon={<SaveOutlined />} disabled={disabled || !!invalid} onClick={() => void persist(false)}>保存资料</Button>
      <Button type="primary" icon={<CopyOutlined />} disabled={disabled || !!invalid} onClick={() => void persist(true)}>复制发客户</Button>
    </div>
    <p className="packing-detail-muted" role="status">{loading ? "正在读取以前保存的资料…" : status}</p>
  </section>;
}
