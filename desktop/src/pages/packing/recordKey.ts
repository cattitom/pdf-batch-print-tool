import type { PackingItemDto } from "../../bindings/PackingItemDto";

export const rowKey = (r: Pick<PackingItemDto, "waybill" | "inbound" | "name">) =>
  JSON.stringify([r.waybill.trim(), r.inbound.trim(), r.name.trim()]);
