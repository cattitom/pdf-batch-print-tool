import { withArchiveRows } from "./archiveRows";
import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText } from "../../api";
import type { PackingItemDto } from "../../bindings/PackingItemDto";

const PREFIX = "packing_archive_v1:";
export type ArchivedTicket = { waybill: string; updatedAt: string; items: PackingItemDto[] };
const itemKey = (r: PackingItemDto) => JSON.stringify([r.inbound.trim(), r.name.trim(), r.customer.trim(), r.destination.trim()]);

/** 独立落入 SQLite settings 表，原表刷新或删行不会删除历史明细。 */
export function usePackingArchive(rows: PackingItemDto[]) {
  const queue = useRef(Promise.resolve());
  const [archiveItems, setArchiveItems] = useState<PackingItemDto[]>([]);
  // 启动恢复的异步回调可能来自首帧，必须通过 ref 读取已加载的最新存档。
  const archiveItemsRef = useRef<PackingItemDto[]>([]);
  const mergeArchiveRows = useCallback((current: PackingItemDto[], savedChecks: Record<string, boolean> = {}) =>
    withArchiveRows(current, archiveItemsRef.current, savedChecks), []);
  useEffect(() => {
    let cancelled = false;
    api.allSettings().then((settings) => {
      const tickets = Object.entries(settings).filter(([key]) => key.startsWith(PREFIX)).map(([, value]) => JSON.parse(value) as ArchivedTicket);
      if (!cancelled) {
        archiveItemsRef.current = tickets.flatMap((t) => t.items);
        setArchiveItems(archiveItemsRef.current);
      }
    }).catch((e) => { if (!cancelled) setArchiveError(`读取历史存档失败：${errorText(e)}`); });
    return () => { cancelled = true; };
  }, []);
  const [archiveError, setArchiveError] = useState<string | null>(null);
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
      const settings = await api.allSettings();
      for (const [waybill, current] of grouped) {
        const key = PREFIX + encodeURIComponent(waybill);
        const previous: ArchivedTicket | null = settings[key] ? JSON.parse(settings[key]) : null;
        const merged = new Map((previous?.items ?? []).map((r) => [itemKey(r), r]));
        for (const row of current) {
          // 后补入库号时，用完整记录替换同商品的缺号记录。
          if (row.inbound.trim()) {
            for (const [oldKey, old] of merged) {
              if (!old.inbound.trim() && old.name === row.name && old.customer === row.customer && old.destination === row.destination) merged.delete(oldKey);
            }
          }
          merged.set(itemKey(row), row);
        }
        const items = [...merged.values()];
        if (JSON.stringify(previous?.items) === JSON.stringify(items)) continue;
        await api.setSetting(key, JSON.stringify({ waybill, updatedAt: new Date().toISOString(), items }));
        archiveItemsRef.current = [...archiveItemsRef.current.filter((r) => r.waybill.trim() !== waybill), ...items];
      }
      setArchiveError(null);
    }).catch((error) => { setArchiveError(`出货档案保存失败：${errorText(error)}`); });
  }, [rows]);
  const readArchive = async (): Promise<ArchivedTicket[]> => {
    await queue.current;
    const settings = await api.allSettings();
    return Object.entries(settings).filter(([key]) => key.startsWith(PREFIX))
      .map(([, value]) => JSON.parse(value) as ArchivedTicket)
      .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  };
  return { archiveError, readArchive, archiveItems, mergeArchiveRows };
}