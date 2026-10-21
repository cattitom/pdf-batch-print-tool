// Tauri 后端 API 封装：类型 + invoke 包装
// JSON 契约与 src-tauri/src/commands.rs 的 DTO 一致（camelCase + type tag）

import { invoke } from "@tauri-apps/api/core";

// ---------- 类型 ----------
//
// 全部由 Rust 侧生成（ts-rs），不再手写，避免字段名/可选性双写漂移。
// 重新生成：TS_RS_EXPORT_DIR=<abs>/src/bindings cargo test export_bindings
import type { CropDto } from "./bindings/CropDto";
import type { ImposeFileDto } from "./bindings/ImposeFileDto";
import type { ImposeOptsDto } from "./bindings/ImposeOptsDto";
import type { ImposeResultDto } from "./bindings/ImposeResultDto";
import type { PdfInfo } from "./bindings/PdfInfo";
import type { PrintHistoryEntry } from "./bindings/PrintHistoryEntry";
import type { LabelSku } from "./bindings/LabelSku";
import type { LabelSpecDto } from "./bindings/LabelSpecDto";
import type { LabelDocEntry } from "./bindings/LabelDocEntry";
import type { LabelResultDto } from "./bindings/LabelResultDto";

export type CropSpec = CropDto;
export type ImposeFile = ImposeFileDto;
export type ImposeOpts = ImposeOptsDto;
export type ImposeResult = ImposeResultDto;
export type { PdfInfo, PrintHistoryEntry, LabelSku, LabelDocEntry };
export type LabelSpec = LabelSpecDto;
export type LabelResult = LabelResultDto;

// ---------- 进度事件 ----------

export const IMPOSE_PROGRESS_EVENT = "impose-progress";

export interface ImposeProgress {
  done: number;
  total: number;
}

export async function listenImposeProgress(
  cb: (p: ImposeProgress) => void
): Promise<() => void> {
  const { listen } = await import("@tauri-apps/api/event");
  const un = await listen<ImposeProgress>(IMPOSE_PROGRESS_EVENT, (e) => cb(e.payload));
  return un;
}

// ---------- 偏好键 ----------

/** 是否在页签栏显示「箱唛生成」（"1" = 显示，缺省 = 隐藏）。 */
export const SHOW_LABEL_TAB_KEY = "show_label_tab";

// ---------- 错误 ----------

/** 后端错误结构（commands.rs 的 ApiError）：code 用于分支，message 可直接展示。 */
export interface ApiError {
  code: string;
  message: string;
}

export function isApiError(e: unknown): e is ApiError {
  return (
    typeof e === "object" &&
    e !== null &&
    typeof (e as ApiError).message === "string" &&
    typeof (e as ApiError).code === "string"
  );
}

/** 稳定错误码；非后端错误返回 null。 */
export function errorCode(e: unknown): string | null {
  return isApiError(e) ? e.code : null;
}

/** 可直接展示的错误文案。 */
export function errorText(e: unknown): string {
  if (isApiError(e)) return e.message;
  return String(e);
}

/** 按错误码补一句人话建议（分级提示）。 */
export function errorHint(e: unknown): string {
  switch (errorCode(e)) {
    case "io":
      return "（文件可能被占用、已移动，或当前目录没有写权限）";
    case "pdf":
      return "（该文件可能不是有效的 PDF，或已损坏）";
    case "invalid":
      return "（请检查参数，例如裁剪范围是否超过页面大小）";
    case "db":
      return "（本机历史库异常，可尝试重启程序）";
    default:
      return "";
  }
}

// ---------- 命令封装 ----------

export const api = {
  // PDF 信息
  pdfInfos: (paths: string[]) => invoke<PdfInfo[]>("pdf_infos", { paths }),

  // 预览：渲染前 maxPages 页为 base64 PNG 数组
  previewPdf: (path: string, dpi = 96, maxPages = 1) =>
    invoke<string[]>("preview_pdf", { path, dpi, maxPages }),

  // 拼版（outPath 先经 dialog 保存对话框取得）
  imposePdfs: (files: ImposeFile[], opts: ImposeOpts, outPath: string) =>
    invoke<ImposeResult>("impose_pdfs", { files, opts, outPath }),

  // 打印历史
  addPrintHistory: (entry: {
    outputName: string;
    outputPath: string | null;
    outputPages: number;
    paper: string | null;
    sepPages: number;
    params: unknown;
    files: unknown;
  }) => invoke<number>("add_print_history", { entry }),
  listPrintHistory: (limit = 100, offset = 0) =>
    invoke<PrintHistoryEntry[]>("list_print_history", { limit, offset }),
  deletePrintHistory: (id: number) => invoke<void>("delete_print_history", { id }),
  clearPrintHistory: () => invoke<void>("clear_print_history"),

  // 箱唛
  generateLabels: (spec: LabelSpec, outPath: string) =>
    invoke<LabelResult>("generate_labels", { spec, outPath }),
  previewLabel: (spec: LabelSpec) => invoke<string>("preview_label", { spec }),
  listLabelDocs: (limit = 50) => invoke<LabelDocEntry[]>("list_label_docs", { limit }),
  latestLabelDoc: () => invoke<LabelDocEntry | null>("latest_label_doc"),

  // 日志
  logDir: () => invoke<string>("log_dir"),

  // 设置
  allSettings: () => invoke<Record<string, string>>("all_settings"),
  setSetting: (key: string, value: string) => invoke<void>("set_setting", { key, value }),
};
