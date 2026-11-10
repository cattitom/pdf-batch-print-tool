import type { PackingItemDto } from "../../bindings/PackingItemDto";

export function isSharedQuantity(row: PackingItemDto): boolean {
  return !!row.quantityGroup && row.issues.includes("qty_shared");
}

export function quantityText(row: PackingItemDto): string {
  return isSharedQuantity(row) ? "合箱（不另计箱数）" : `${row.qty} 件`;
}

export function quantityGroups(rows: PackingItemDto[]) {
  const groups = new Map<string, PackingItemDto[]>();
  for (const row of rows) {
    if (!row.quantityGroup || row.issues.includes("history_archive")) continue;
    const items = groups.get(row.quantityGroup) ?? [];
    items.push(row);
    groups.set(row.quantityGroup, items);
  }
  return [...groups].map(([range, items]) => ({
    range, items, total: items.reduce((n, row) => n + row.qty, 0),
  }));
}
