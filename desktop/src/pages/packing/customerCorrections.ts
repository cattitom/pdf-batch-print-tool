import type { PackingItemDto } from "../../bindings/PackingItemDto";

export const CUSTOMER_CORRECTIONS_KEY = "packing_customer_corrections_v1";
type Identity = Pick<PackingItemDto, "inbound" | "name" | "destination">;
export const customerCorrectionKey = (row: Identity) => JSON.stringify([row.inbound.trim(), row.name.trim(), row.destination.trim()]);

/** 人工确认的客户归属独立保存，原表客户文字缺失时也不会在下次同步丢失。 */
export function applyCustomerCorrections(rows: PackingItemDto[], raw?: string): PackingItemDto[] {
  if (!raw) return rows;
  let corrections: unknown;
  try { corrections = JSON.parse(raw); } catch { return rows; }
  if (!corrections || typeof corrections !== "object" || Array.isArray(corrections)) return rows;
  const entries = corrections as Record<string, unknown>;
  return rows.map((row) => {
    const value = entries[customerCorrectionKey(row)];
    return typeof value === "string" && value.trim() ? { ...row, customer: value.trim() } : row;
  });
}
