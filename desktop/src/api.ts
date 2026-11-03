// Tauri 后端 API 封装：类型 + invoke 包装
// JSON 契约与 src-tauri/src/commands.rs 的 DTO 一致（camelCase + type tag）

import { invoke } from "@tauri-apps/api/core";
// 静态引入：App.tsx 已静态依赖 @tauri-apps/api/webview（内部再引 event.js），
// 这里再动态 import 无法拆出独立 chunk，只会触发 INEFFECTIVE_DYNAMIC_IMPORT。
import { listen } from "@tauri-apps/api/event";

// ---------- 类型 ----------
//
// 全部由 Rust 侧生成（ts-rs），不再手写，避免字段名/可选性双写漂移。
// 重新生成：TS_RS_EXPORT_DIR=<abs>/src/bindings cargo test export_bindings
import type { CropDto } from "./bindings/CropDto";
import type { ImposeFileDto } from "./bindings/ImposeFileDto";
import type { ImposeOptsDto } from "./bindings/ImposeOptsDto";
import type { ImposeResultDto } from "./bindings/ImposeResultDto";
import type { PdfInfo } from "./bindings/PdfInfo";
import type { PackingCheckDto } from "./bindings/PackingCheckDto";
import type { PrintHistoryEntry } from "./bindings/PrintHistoryEntry";
import type { LabelSku } from "./bindings/LabelSku";
import type { LabelSpecDto } from "./bindings/LabelSpecDto";
import type { LabelDocEntry } from "./bindings/LabelDocEntry";
import type { LabelResultDto } from "./bindings/LabelResultDto";
import type { ProductLabelItem } from "./bindings/ProductLabelItem";
import type { ProductLabelResult } from "./bindings/ProductLabelResult";
import type { OpenPdfResultDto } from "./bindings/OpenPdfResultDto";
import type { PackingSheetDto } from "./bindings/PackingSheetDto";
import type { PackingItemDto } from "./bindings/PackingItemDto";
import type { KdocsSyncDto } from "./bindings/KdocsSyncDto";

// ---------- 微信消息（后端 wechat 模块，手写类型） ----------

export interface WechatAccount {
  dataDir: string;
  wxid: string;
  decrypted: boolean;
}

export interface WechatStatus {
  accounts: WechatAccount[];
  selected: string | null;
}

export interface WechatScanResult {
  ok: number;
  failed: number;
  skipped: number;
  walFrames: number;
  ms: number;
  outDir: string;
}

export interface WechatRoom {
  id: string;
  name: string;
  count: number;
  last: number;
}

export interface WechatMsgFields {
  customer: string;
  sku: string;
  qty: string;
}

export interface WechatMsg {
  ts: number;
  time: string;
  sender: string;
  cat: string;
  text: string;
  fields?: WechatMsgFields;
}

export interface WechatExport {
  rows: number;
  path: string;
}

export type CropSpec = CropDto;
export type ImposeFile = ImposeFileDto;
export type ImposeOpts = ImposeOptsDto;
export type ImposeResult = ImposeResultDto;
export type OpenPdfResult = OpenPdfResultDto;
export type { PdfInfo, PrintHistoryEntry, LabelSku, LabelDocEntry };
export type LabelSpec = LabelSpecDto;
export type LabelResult = LabelResultDto;
export type { ProductLabelItem, ProductLabelResult };
export type { PackingItemDto, PackingSheetDto, KdocsSyncDto };

// ---------- 进度事件 ----------

export const IMPOSE_PROGRESS_EVENT = "impose-progress";

export interface ImposeProgress {
  done: number;
  total: number;
}

export async function listenImposeProgress(
  cb: (p: ImposeProgress) => void
): Promise<() => void> {
  const un = await listen<ImposeProgress>(IMPOSE_PROGRESS_EVENT, (e) => cb(e.payload));
  return un;
}

// ---------- 偏好键 ----------

/** 是否在页签栏显示「箱唛生成」（"1" = 显示，缺省 = 隐藏）。 */
export const SHOW_LABEL_TAB_KEY = "show_label_tab";

/** 是否在页签栏显示「箱规计划」（"1" = 显示，缺省 = 隐藏）。 */
export const SHOW_BOXPLAN_TAB_KEY = "show_boxplan_tab";

/** 是否在页签栏显示「产品标签」（"1" = 显示，缺省 = 隐藏）。 */
export const SHOW_PRODUCT_TAB_KEY = "show_product_label_tab";

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
    case "wps_not_found":
      return "（未检测到 WPS Office；文件已合成，可手动打开）";
    default:
      return "";
  }
}


// ---------- 手机出货（局域网） ----------

/** 手机端一行：只带勾选要用的字段。 */
export interface MobileRowDto {
  waybill: string;
  inbound: string;
  name: string;
  customer: string;
  qty: number;
  checked: boolean;
  issues: string[];
  /** 既没入库号也没单号：不能标已出（桌面/手机都封），但照常算未出、照常下发。 */
  blocked: boolean;
  /** 本次出货任务（桌面从出货截图里导入的那批）。 */
  task: boolean;
  /** 任务号（按单号分组，T1/T2…） */
  taskId: string;
  /** 这张票被谁领了（空 = 没人领） */
  claimedBy: string;
}

export interface MobileInfoDto {
  url: string;
  /** 所有可试的地址，第一个是二维码里的 */
  urls: string[];
  token: string;
  port: number;
  qrSvg: string;
  /** 没找到局域网地址时的提示 */
  warn: string | null;
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

  // 用 WPS PDF 打开成品（不依赖系统默认 PDF 关联）
  openPdfInWps: (path: string) => invoke<OpenPdfResult>("open_pdf_in_wps", { path }),

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

  // 产品标签（50×30mm）不再由本工具出 PDF，只导出 CSV 交给 gLabels；
  // 后端 generate_product_labels / preview_product_label 命令保留，需要时可直接调。
  // 日志
  logDir: () => invoke<string>("log_dir"),
  readLogTail: (lines = 400) => invoke<string>("read_log_tail", { lines }),
  clearLog: () => invoke<void>("clear_log"),

  // 装箱单解析（粘贴文本 / 拖文件 → 按目的地+单号分组的待办）
  parsePackingText: (text: string) =>
    invoke<PackingSheetDto>("parse_packing_text", { text }),
  parsePackingFile: (path: string) =>
    invoke<PackingSheetDto>("parse_packing_file", { path }),
  parsePackingImage: (path: string) =>
    invoke<PackingSheetDto>("parse_packing_image", { path }),
  /** 指定 .xls 里的一张表解析（云表格缓存恢复用；sheet 省略 = 自动挑最新的来货登记表） */
  parsePackingXls: (path: string, sheet?: string | null) =>
    invoke<PackingSheetDto>("parse_packing_xls", { path, sheet: sheet ?? null }),
  // WPS 云表格同步：借已登录的 Chrome 拉 .xls → Rust 直接解析（不依赖 Python）。
  // sheet 传 null = 自动挑最新一张来货登记表；传表名 = 只重解析那张（用刚下的文件）
  syncKdocsPacking: (sheet?: string | null) =>
    invoke<KdocsSyncDto>("sync_kdocs_packing", { sheet: sheet ?? null }),
  saveTextFile: (path: string, content: string) =>
    invoke<void>("save_text_file", { path, content }),

  // 手机出货：桌面端起局域网服务，手机浏览器勾选
  mobileStart: (rows: MobileRowDto[]) => invoke<MobileInfoDto>("mobile_start", { rows }),
  mobileSync: (rows: MobileRowDto[]) => invoke<void>("mobile_sync", { rows }),
  mobileSetCheck: (waybill: string, inbound: string, name: string, checked: boolean) =>
    invoke<void>("mobile_set_check", { waybill, inbound, name, checked }),
  mobileRows: () => invoke<MobileRowDto[]>("mobile_rows"),
  mobileResetToken: () => invoke<MobileInfoDto>("mobile_reset_token"),
  mobileStop: () => invoke<void>("mobile_stop"),
  mobileInfo: () => invoke<MobileInfoDto | null>("mobile_info"),
  /** 一键放行局域网端口（内部会请一次 UAC）；返回一句给用户看的结果。 */
  mobileAllowFirewall: () => invoke<string>("mobile_allow_firewall"),
  /** 从「本次要出货」的截图里认入库号：codes 是认出来的，failed 是没读成功的图。 */
  parseShipImages: (paths: string[]) =>
    invoke<{ codes: string[]; failed: string[] }>("parse_ship_images", { paths }),
  /** 产品标签照片 → 编码 + 名称（一张图一条），给「快速解析」的图片导入用。 */
  parseLabelImages: (paths: string[]) =>
    invoke<{ rows: { code: string; name: string }[]; failed: string[] }>(
      "parse_label_images",
      { paths },
    ),

  // 微信消息（本机微信 4.x 库解密后读取）
  wechatStatus: (extra: string[] = []) =>
    invoke<WechatStatus>("wechat_status", { extra }),
  /** 校验手选的目录，返回里面认出的账号（空数组 = 不是微信数据目录）。 */
  wechatCheckDir: (path: string) => invoke<WechatAccount[]>("wechat_check_dir", { path }),
  wechatScan: (account: string) => invoke<WechatScanResult>("wechat_scan", { account }),
  wechatRooms: (account: string) => invoke<WechatRoom[]>("wechat_rooms", { account }),
  wechatMessages: (
    account: string,
    chatId: string,
    days: number | null,
    limit: number | null,
    kind: number | null,
  ) =>
    invoke<WechatMsg[]>("wechat_messages", { account, chatId, days, limit, kind }),
  wechatPoll: (account: string, chatId: string, afterTs: number) =>
    invoke<WechatMsg[]>("wechat_poll", { account, chatId, afterTs }),
  wechatExportCsv: (
    account: string,
    chatId: string,
    days: number | null,
    kind: number | null,
    path: string,
  ) =>
    invoke<WechatExport>("wechat_export_csv", { account, chatId, days, kind, path }),

  // 设置
  allSettings: () => invoke<Record<string, string>>("all_settings"),
  setSetting: (key: string, value: string) => invoke<void>("set_setting", { key, value }),

  // 出货清单：已录勾选（按 单号 + 入库号 + 商品名 认行）
  setPackingCheck: (waybill: string, inbound: string, name: string, checked: boolean) =>
    invoke<void>("set_packing_check", { waybill, inbound, name, checked }),
  listPackingChecks: () => invoke<PackingCheckDto[]>("list_packing_checks"),
  clearPackingChecks: () => invoke<void>("clear_packing_checks"),
};




