import type { PackingItemDto } from "../../bindings/PackingItemDto";
export const HISTORY_ISSUE = "history_archive";
const identity = (r: PackingItemDto) => JSON.stringify([r.waybill.trim(), r.inbound.trim(), r.name.trim(), r.customer.trim(), r.destination.trim()]);
export function withArchiveRows(current: PackingItemDto[], archived: PackingItemDto[]): PackingItemDto[] {
  const live = current.filter((r) => !r.issues.includes(HISTORY_ISSUE));
  const keys = new Set(live.map(identity));
  const history: PackingItemDto[] = [];
  for (const row of archived) {
    if (keys.has(identity(row))) continue;
    // 当前原表补了入库号，不再重复展示旧的缺号存档。
    if (!row.inbound.trim() && live.some((r) => r.waybill === row.waybill && r.name === row.name && r.customer === row.customer && r.destination === row.destination)) continue;
    keys.add(identity(row));
    history.push({ ...row, issues: [...row.issues.filter((c) => c !== HISTORY_ISSUE), HISTORY_ISSUE] });
  }
  return [...live, ...history];
}