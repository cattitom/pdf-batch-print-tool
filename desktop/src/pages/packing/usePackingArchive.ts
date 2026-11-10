import { withArchiveRows } from "./archiveRows";
import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText } from "../../api";
import type { PackingItemDto } from "../../bindings/PackingItemDto";

/** 一票存档。由后端 archive_ticket / archive_item 两张表按票聚合而来。 */
export type ArchivedTicket = { waybill: string; updatedAt: string; items: PackingItemDto[] };

/** 只把「这一票当前的行」交给后端，去重与覆盖由数据库主键负责。 */
function archiveKeyOf(row: PackingItemDto) {
  return [row.waybill.trim(), row.inbound.trim(), row.name.trim(), row.customer.trim(), row.destination.trim(), row.seq || 0];
}

/**
 * 历史存档（独立于原表：原表刷新、删行、云表移除单号都不影响这里）。
 *
 * 存储从 settings 里的一堆 JSON 键换成了 ledger.rs 的正规化表：
 * archive_item 以 (票,入库号,商品,客户,目的地,序号) 为主键，
 * 因此同一个单号不会再出现「原文键 + URL 编码键」两份互不清账的副本。
 */
export function usePackingArchive(rows: PackingItemDto[]) {
  const queue = useRef(Promise.resolve());
  const [archiveItems, setArchiveItems] = useState<PackingItemDto[]>([]);
  // 启动恢复的异步回调可能来自首帧，必须通过 ref 读取已加载的最新存档。
  const archiveItemsRef = useRef<PackingItemDto[]>([]);
  const mergeArchiveRows = useCallback((current: PackingItemDto[], savedChecks: Record<string, boolean> = {}) =>
    withArchiveRows(current, archiveItemsRef.current, savedChecks), []);
  const [archiveError, setArchiveError] = useState<string | null>(null);
  // 每票上一次落库的内容指纹，内容没变就不重复写库。
  const savedSignature = useRef(new Map<string, string>());

  const loadTickets = useCallback(async (): Promise<ArchivedTicket[]> => {
    const tickets = await api.ledgerArchiveTickets();
    return [...tickets].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  }, []);

  useEffect(() => {
    let cancelled = false;
    loadTickets()
      .then((tickets) => {
        if (cancelled) return;
        archiveItemsRef.current = tickets.flatMap((t) => t.items);
        setArchiveItems(archiveItemsRef.current);
      })
      .catch((e) => { if (!cancelled) setArchiveError(`读取历史存档失败：${errorText(e)}`); });
    return () => { cancelled = true; };
  }, [loadTickets]);

  useEffect(() => {
    const grouped = new Map<string, PackingItemDto[]>();
    for (const row of rows) {
      const waybill = row.waybill.trim();
      if (!waybill) continue;
      const list = grouped.get(waybill) ?? [];
      list.push({ ...row, waybill, issues: row.issues.filter((c) => c !== "history_archive" && c !== "waybill_archive") });
      grouped.set(waybill, list);
    }
    if (!grouped.size) return;
    queue.current = queue.current.catch(() => {}).then(async () => {
      let changed = false;
      for (const [waybill, current] of grouped) {
        const saved = current.map((row) => ({ ...row, waybill }));
        const signature = JSON.stringify(saved.map((r) => [archiveKeyOf(r), r.qty, r.qtyRaw, r.checked, r.arrival, r.cargo ?? null, r.issues]));
        if (savedSignature.current.get(waybill) === signature) continue;
        await api.ledgerArchiveSave(saved);
        savedSignature.current.set(waybill, signature);
        // 内存视图与数据库保持同一套唯一键：同键覆盖，后补入库号时缺号的旧行让位。
        const keys = new Set(saved.map((r) => JSON.stringify(archiveKeyOf(r))));
        const filled = new Set(saved.filter((r) => r.inbound.trim()).map((r) => JSON.stringify([r.name.trim(), r.customer.trim(), r.destination.trim()])));
        const kept = archiveItemsRef.current.filter((r) => {
          if (r.waybill.trim() !== waybill) return true;
          if (keys.has(JSON.stringify(archiveKeyOf(r)))) return false;
          return !(!r.inbound.trim() && filled.has(JSON.stringify([r.name.trim(), r.customer.trim(), r.destination.trim()])));
        });
        archiveItemsRef.current = [...kept, ...saved];
        changed = true;
      }
      if (changed) setArchiveItems(archiveItemsRef.current);
      setArchiveError(null);
    }).catch((error) => { setArchiveError(`出货档案保存失败：${errorText(error)}`); });
  }, [rows]);

  const readArchive = loadTickets;
  return { archiveError, readArchive, archiveItems, mergeArchiveRows };
}
