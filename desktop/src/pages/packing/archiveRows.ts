import type { PackingItemDto } from "../../bindings/PackingItemDto";
export const HISTORY_ISSUE = "history_archive";
export const ARCHIVED_WAYBILL_ISSUE = "waybill_archive";
const identity = (r: PackingItemDto) => JSON.stringify([r.waybill.trim(), r.inbound.trim(), r.name.trim(), r.customer.trim(), r.destination.trim(), r.sourceRow || r.seq || 0]);
const cargoIdentity = (r: PackingItemDto) => JSON.stringify([r.inbound.trim(), r.name.trim(), r.customer.trim(), r.destination.trim(), r.qty]);
// 一件货的业务身份：不含数量、也不含原表行号（行号每次重新解析都会整体偏移）。
const goodsIdentity = (r: PackingItemDto) => JSON.stringify([r.inbound.trim(), r.name.trim(), r.customer.trim(), r.destination.trim()]);
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
    let candidates = archivedByCargo.get(key) ?? [];
    const exact = candidates.filter(r => row.sourceRow && r.sourceRow ? row.sourceRow === r.sourceRow : row.seq && r.seq && row.seq === r.seq);
    if (exact.length) candidates = exact;
    const waybills = new Set(candidates.map((r) => r.waybill.trim()));
    // 同一货物存在多票或多行时，不凭商品名推断单号。
    if ((!exact.length && sourceCounts.get(key) !== 1) || waybills.size !== 1) return row;
    const match = candidates[0];
    const restored = { ...row, waybill: match.waybill.trim() };
    return {
      ...restored,
      checked: row.checked || (savedChecks[checkKey(restored)] ?? match.checked),
      issues: [...row.issues.filter((c) => c !== ARCHIVED_WAYBILL_ISSUE), ARCHIVED_WAYBILL_ISSUE],
    };
  });
  const keys = new Set(live.map(identity));
  // 这票里还活着的货：存档副本一律不再显示，否则同一批货会被算两遍
  // （拆批 20+15 合成 35、数量被改过、行号整体偏移都属于这种情况）。
  const liveGoods = new Set(live.filter((r) => r.inbound.trim()).map(goodsIdentity));
  const history: PackingItemDto[] = [];
  for (const row of archived) {
    if (keys.has(identity(row))) continue;
    if (row.inbound.trim() && liveGoods.has(goodsIdentity(row))) continue;
    const split = live.filter(r => row.inbound.trim() && r.inbound.trim() === row.inbound.trim() && r.name.trim() === row.name.trim() && r.destination.trim() === row.destination.trim());
    if (split.length > 1 && split.reduce((n,r)=>n+r.qty,0) === row.qty && !split.some(r=>r.qty===row.qty)) continue;
    // 手机主库已明确移动该明细时只展示现分组，原始历史存档仍保留在数据库中。
    if (row.inbound.trim() && live.some(r=>r.manualFields?.includes("waybill")&&r.inbound.trim()===row.inbound.trim()&&r.name.trim()===row.name.trim()&&r.destination.trim()===row.destination.trim()&&r.waybill!==row.waybill)) continue;
    if (!row.inbound.trim() && live.some((r) => r.waybill === row.waybill && r.name === row.name && r.customer === row.customer && r.destination === row.destination)) continue;
    keys.add(identity(row));
    history.push({ ...row, checked: savedChecks[checkKey(row)] ?? row.checked, issues: [...row.issues.filter((c) => c !== HISTORY_ISSUE), HISTORY_ISSUE] });
  }
  return [...live, ...history];
}
