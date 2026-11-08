import type { PackingItemDto } from "../../bindings/PackingItemDto";

export type CustomerMessageData = {
  customer: string; inbound: string; product: string; specification: string; perBox: string;
  length: string; width: string; height: string; actualWeight: string; packages: string;
};
export const customerMessageKey = (r: PackingItemDto) =>
  "packing_customer_message_v1:" + JSON.stringify([r.customer.trim(), r.inbound.trim(), r.name.trim(), r.destination.trim(), r.waybill.trim()]);

/** 原表数值只去除明确的单位；无法确定的文本留空，不从多个数中猜一个。 */
function cargoNumber(raw: string | undefined): string {
  const text = (raw || "").trim().replace(/\s*(?:cm|厘米|kg|公斤|千克|件|个)\s*$/i, "").trim();
  return /^\d+(\.\d+)?$/.test(text) ? text : "";
}
function splitPackedProduct(name: string) {
  const box = name.match(/[,，]?\s*(\d+)\s*[/／]\s*箱/);
  const description = name.replace(/[,，]?\s*\d+\s*[/／]\s*箱/, "").trim().replace(/[,，]\s*$/, "");
  const spec = description.match(/\s+(\d+\s*件装.*)$/);
  return { product: spec ? description.slice(0, spec.index).trim() : description, specification: spec?.[1].trim() || "", perBox: box?.[1] || "" };
}
export function defaultCustomerMessage(r: PackingItemDto): CustomerMessageData {
  const cargo = r.cargo || {};
  const packed = splitPackedProduct(r.name);
  const dimensions = (cargo.dimensions || "").match(/^(\d+(?:\.\d+)?)\s*[×xX*＊]\s*(\d+(?:\.\d+)?)\s*[×xX*＊]\s*(\d+(?:\.\d+)?)\s*(?:cm|厘米)?$/i);
  return { customer: r.customer, inbound: r.inbound,
    product: packed.product,
    specification: cargo.specification?.trim() || packed.specification,
    perBox: cargoNumber(cargo.perBox) || packed.perBox,
    length: cargoNumber(cargo.length) || dimensions?.[1] || "",
    width: cargoNumber(cargo.width) || dimensions?.[2] || "",
    height: cargoNumber(cargo.height) || dimensions?.[3] || "",
    actualWeight: cargoNumber(cargo.actualWeight), packages: String(r.qty) };
}
const fields: (keyof CustomerMessageData)[] = ["customer", "inbound", "product", "specification", "perBox", "length", "width", "height", "actualWeight", "packages"];
export function readCustomerMessage(raw: string | undefined, fallback: CustomerMessageData): { data: CustomerMessageData; updatedAt?: string } {
  if (!raw) return { data: fallback };
  const saved = JSON.parse(raw);
  if (saved?.schema === 2 && saved.overrides && typeof saved.overrides === "object" && !Array.isArray(saved.overrides)) {
    if (Object.entries(saved.overrides).some(([key, value]) => !fields.includes(key as keyof CustomerMessageData) || typeof value !== "string")) throw new Error("保存的客户资料格式不正确");
    return { data: { ...fallback, ...saved.overrides }, updatedAt: saved.updatedAt };
  }
  if (!saved || saved.schema !== 1 || !saved.data || fields.some((field) => typeof saved.data[field] !== "string")) throw new Error("保存的客户资料格式不正确");
  const legacy = { ...saved.data };
  if (!legacy.specification.trim() && !legacy.perBox.trim()) Object.assign(legacy, splitPackedProduct(legacy.product));
  // 旧版空格是尚未采集到原表数据，不将其当作用户主动清空。
  return { data: Object.fromEntries(fields.map((field) => [field, legacy[field].trim() ? legacy[field] : fallback[field]])) as CustomerMessageData, updatedAt: saved.updatedAt };
}
export function customerMessageOverrides(data: CustomerMessageData, source: CustomerMessageData) {
  return Object.fromEntries(fields.filter((field) => data[field] !== source[field]).map((field) => [field, data[field]]));
}
function positive(text: string) { return /^\d+(\.\d+)?$/.test(text.trim()) && Number.isFinite(Number(text)) && Number(text) > 0; }
export function customerMessageError(d: CustomerMessageData): string | null {
  const sizes = [d.length, d.width, d.height];
  if (sizes.some((v) => v.trim()) && !sizes.every(positive)) return "长、宽、高请填写完整的正数，单位 cm";
  if (d.actualWeight.trim() && !positive(d.actualWeight)) return "实重请填写正数，单位 kg/箱";
  if (d.perBox.trim() && (!positive(d.perBox) || !Number.isInteger(Number(d.perBox)))) return "每箱数量请填写正整数";
  if (d.packages.trim() && (!/^\d+$/.test(d.packages.trim()) || !Number.isSafeInteger(Number(d.packages)))) return "总件数请填写非负整数";
  if (sizes.every(positive) && !Number.isFinite(Number(d.length) * Number(d.width) * Number(d.height))) return "尺寸数值过大，请核对";
  return null;
}
function clean(v: string) { return v.trim(); }
export function customerMessageText(d: CustomerMessageData): string {
  const lines = [clean(d.customer), clean(d.inbound)].filter(Boolean);
  let product = [clean(d.product), clean(d.specification)].filter(Boolean).join("  ");
  if (clean(d.perBox)) product += (product ? "，" : "") + clean(d.perBox) + "/箱";
  if (product) lines.push(product);
  if ([d.length, d.width, d.height].every(positive)) {
    const volume = Number(d.length) * Number(d.width) * Number(d.height) / 6000;
    if (Number.isFinite(volume)) lines.push([d.length, d.width, d.height].map(clean).join("×") + "/6000=" + volume.toFixed(3).replace(/\.?0+$/, ""));
  }
  const weights = [];
  if (clean(d.actualWeight)) weights.push("实" + clean(d.actualWeight));
  if (clean(d.packages)) weights.push("共（" + clean(d.packages) + " 件）");
  if (weights.length) lines.push(weights.join("，"));
  return lines.join("\n");
}

/** 解析常用五行通知，不从不完整的旧消息推断尺寸或重量。 */
export function parseCustomerMessage(text: string): Partial<CustomerMessageData> {
  const lines = text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  if (!lines.length) throw new Error("请先粘贴以前发过的货物信息");
  const data: Partial<CustomerMessageData> = {};
  const sizeIndex = lines.findIndex((line) => /\d\s*[×xX*＊]\s*\d/.test(line) && /[/÷]\s*6000/.test(line));
  const weightIndex = lines.findIndex((line) => /实\s*\d|共\s*[（(]?\s*\d/.test(line));
  const inboundIndex = lines.findIndex((line) => /^(?:IB|FBA|HOU|LAX)[a-z0-9_-]+$/i.test(line));
  if (inboundIndex < 0 && sizeIndex < 0 && weightIndex < 0) throw new Error("没有识别到入库号、尺寸或实重，请核对消息格式");
  if (inboundIndex > 0) data.customer = lines[inboundIndex - 1];
  if (inboundIndex >= 0) data.inbound = lines[inboundIndex];
  const productIndex = inboundIndex >= 0 ? inboundIndex + 1 : 1;
  const productLine = lines[productIndex];
  if (productLine && productIndex !== sizeIndex && productIndex !== weightIndex) {
    const box = productLine.match(/[,，]?\s*(\d+)\s*[/／]\s*箱/);
    const withoutBox = productLine.replace(/[,，]?\s*\d+\s*[/／]\s*箱/, "").trim().replace(/[,，]\s*$/, "");
    const specification = withoutBox.match(/\s+(\d+\s*件装.*)$/);
    data.product = specification ? withoutBox.slice(0, specification.index).trim() : withoutBox;
    if (specification) data.specification = specification[1].trim();
    if (box) data.perBox = box[1];
  }
  if (sizeIndex >= 0) {
    const size = lines[sizeIndex].match(/(\d+(?:\.\d+)?)\s*[×xX*＊]\s*(\d+(?:\.\d+)?)\s*[×xX*＊]\s*(\d+(?:\.\d+)?)\s*[/÷]\s*6000/);
    if (size) { data.length = size[1]; data.width = size[2]; data.height = size[3]; }
  }
  if (weightIndex >= 0) {
    const weight = lines[weightIndex].match(/实\s*(\d+(?:\.\d+)?)/);
    const count = lines[weightIndex].match(/共\s*[（(]?\s*(\d+)\s*件/);
    if (weight) data.actualWeight = weight[1];
    if (count) data.packages = count[1];
  }
  return data;
}
