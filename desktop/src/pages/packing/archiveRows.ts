import type { PackingItemDto } from "../../bindings/PackingItemDto";
export const HISTORY_ISSUE = "history_archive";
export const ARCHIVED_WAYBILL_ISSUE = "waybill_archive";
const identity = (r: PackingItemDto) => JSON.stringify([r.waybill.trim(), r.inbound.trim(), r.name.trim(), r.customer.trim(), r.destination.trim()]);
const cargoIdentity = (r: PackingItemDto) => JSON.stringify([r.inbound.trim(), r.name.trim(), r.customer.trim(), r.destination.trim(), r.qty]);
const checkKey = (r: PackingItemDto) => JSON.stringify([r.waybill.trim(), r.inbound.trim(), r.name.trim()]);
export function withArchiveRows(current: PackingItemDto[], archived: PackingItemDto[], savedChecks: Record<string, boolean> = {}): PackingItemDto[] {
  const source = current.filter((r) => !r.issues.includes(HISTORY_ISSUE));
  const sourceCounts = new Map<string, number>();
  const archivedByCargo = new Map<string, PackingItemDto[]>();
  for (const row of source) sourceCounts.set(cargoIdentity(row), (sourceCounts.get(cargoIdentity(row)) ?? 0) + 1);
  for (const row of archived) {
    if (!row.inbound.trim() || !row.waybill.trim() || !row.name.trim()) continue;
    const key = cargoIdentity(row);
    const list = archivedByCargo.get(key) ?? [];
    list.push(row);
    archivedByCargo.set(key, list);
  }
  const live = source.map((row) => {
    if (row.waybill.trim() || !row.inbound.trim() || !row.name.trim()) return row;
    const key = cargoIdentity(row);
    const candidates = archivedByCargo.get(key) ?? [];
    const waybills = new Set(candidates.map((r) => r.waybill.trim()));
    // 同一货物存在多票或多行时，不凭商品名推断单号。
    if (sourceCounts.get(key) !== 1 || waybills.size !== 1) return row;
    const match = candidates[0];
    const restored = { ...row, waybill: match.waybill.trim() };
    return {
      ...restored,
      checked: row.checked || (savedChecks[checkKey(restored)] ?? match.checked),
      issues: [...row.issues.filter((c) => c !== ARCHIVED_WAYBILL_ISSUE), ARCHIVED_WAYBILL_ISSUE],
    };
  });
  const keys = new Set(live.map(identity));
  const history: PackingItemDto[] = [];
  for (const row of archived) {
    if (keys.has(identity(row))) continue;
    if (!row.inbound.trim() && live.some((r) => r.waybill === row.waybill && r.name === row.name && r.customer === row.customer && r.destination === row.destination)) continue;
    keys.add(identity(row));
    history.push({ ...row, checked: savedChecks[checkKey(row)] ?? row.checked, issues: [...row.issues.filter((c) => c !== HISTORY_ISSUE), HISTORY_ISSUE] });
  }
  return [...live, ...history];
}