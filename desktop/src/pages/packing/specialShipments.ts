import type { PackingItemDto } from "../../bindings/PackingItemDto";

export type SpecialLabel = "亚马逊" | "发深圳";
export const specialIssue = (label: SpecialLabel) => label === "亚马逊" ? "special_amazon" : "special_shenzhen";
export const specialLabel = (row: Pick<PackingItemDto, "issues">): SpecialLabel | undefined =>
  row.issues.includes("special_shenzhen") ? "发深圳" : row.issues.includes("special_amazon") ? "亚马逊" : undefined;
export const hasSpecialShipment = (row: Pick<PackingItemDto, "issues">) => !!specialLabel(row);
export const isSpecialLabel = (label: unknown): label is SpecialLabel => label === "亚马逊" || label === "发深圳";
export function suggestedSpecialLabel(row: PackingItemDto): SpecialLabel | undefined {
  if ([row.customer, row.destination, row.name, row.arrival, row.cargo?.remarks || ""].some(v => v.trim() === "发深圳") || row.destination.trim() === "深圳") return "发深圳";
  if (row.customer.trim() === "果粒橙") return "亚马逊";
  return undefined;
}
