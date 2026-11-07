import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import type { KdocsSyncDto } from "../../bindings/KdocsSyncDto";
import type { PackingSheetDto } from "../../bindings/PackingSheetDto";

/** 有更新就直接换表（"1"）；默认关 = 只提示，等你点。 */
export const KEY_KDOCS_AUTO_SYNC = "kdocs_auto_sync";
/** 上次同步下载的 .xls 路径：启动时先把它解析出来，表格立刻有内容，不用先点同步。 */
export const KEY_KDOCS_LAST = "kdocs_last_xlsx";
/** 上次同步用的表名、云端版本号、这本表里所有表名（JSON 数组）。 */
export const KEY_KDOCS_SHEET = "kdocs_last_sheet";
export const KEY_KDOCS_FVER = "kdocs_last_fver";
export const KEY_KDOCS_SHEETS = "kdocs_last_sheets";

/** 当前已应用版本一次写入，避免表名、版本和路径只保存了一半。 */
export const KEY_KDOCS_APPLIED = "kdocs_applied_cache";
type CacheState = { sheet: string; sheets: string[]; fver: string; path: string };
export function readCloudCache(settings: Record<string, string>): CacheState {
  try {
    const value = JSON.parse(settings[KEY_KDOCS_APPLIED] || "null");
    if (value && typeof value.sheet === "string" && typeof value.path === "string" && typeof value.fver === "string" &&
        Array.isArray(value.sheets) && value.sheets.every((s: unknown) => typeof s === "string")) return value;
  } catch { /* 兼容旧版缓存 */ }
  let sheets: string[] = [];
  try { const parsed: unknown = JSON.parse(settings[KEY_KDOCS_SHEETS] || "[]");
    if (Array.isArray(parsed)) sheets = parsed.filter((s): s is string => typeof s === "string");
  } catch { /* 旧列表损坏不影响按名称恢复 */ }
  return { sheet: settings[KEY_KDOCS_SHEET] || "", sheets,
    fver: settings[KEY_KDOCS_FVER] || "", path: settings[KEY_KDOCS_LAST] || "" };
}

/** 后台检查的间隔：10 分钟问一次版本号，没变后端连文件都不碰。 */
const CHECK_EVERY_MS = 10 * 60 * 1000;

type SyncOpts = {
  /** 启动 / 定时检查用：版本号没变就什么都不做；变了默认只亮提示。 */
  background?: boolean;
  /** 手里还没有表（第一次用、缓存丢了）时直接铺上去。 */
  applyAnyway?: boolean;
};

type Options = {
  /** 「有更新自动换表」勾没勾（页面 state，hook 只读）。 */
  autoSync: boolean;
  /** 拿到新表：页面负责 applySheet + 清掉「来自 OCR / 上次文件」这些标记。 */
  onApplied: (sheet: PackingSheetDto) => void;
  /** 顶部提示条文案。 */
  onHint: (msg: string) => void;
  /** 手动同步失败：页面负责 setError。 */
  onError: (e: unknown) => void;
  /** 错误文案（页面里的 errorText）。 */
  errorText: (e: unknown) => string;
  /** 手上这张表是不是用户自己选的文件（是就别偷偷换掉）。 */
  hasExternalFile: () => boolean;
  /** 现在表里有没有行（决定「没表就直接铺」）。 */
  hasRows: () => boolean;
};

export type CloudSync = {
  /** 同步中（按钮上转圈用）。 */
  syncing: boolean;
  /** 上次同步用的是哪张表，以及这本表里所有表名（换表下拉用）。 */
  cloudSheet: string | null;
  cloudSheets: string[];
  /** 云端有新版本但还没换表（存版本号）。 */
  cloudUpdate: string | null;
  setCloudUpdate: (v: string | null) => void;
  /** 启动恢复缓存时把「上次那张表 / 表名列表」灌回来。 */
  setCloudSheet: (v: string | null) => void;
  setCloudSheets: (v: string[]) => void;
  /** 从云表格同步；`background` 会被并发挡掉，`applyAnyway` 见上。 */
  sync: (sheetName?: string, opts?: SyncOpts) => Promise<void>;
  /** 不联网，直接解析上次下载的 .xls 缓存（测试 / 断网时用）。 */
  openCached: () => Promise<void>;
};

/**
 * 云表格同步：后端借已登录的 Chrome（窗口没开就自己起无界面）拉最新来货登记 → 解析成待办。
 * 负责同步状态、版本号缓存写回、10 分钟一次的后台检查；表的落地交给页面。
 */
export function useCloudSync(opts: Options): CloudSync {
  const [syncing, setSyncing] = useState(false);
  const [cloudSheet, updateCloudSheet] = useState<string | null>(null);
  const sheetRef = useRef<string | null>(null);
  const appliedRef = useRef<CacheState | null>(null);
  const pendingRef = useRef<KdocsSyncDto | null>(null);
  const setCloudSheet = useCallback((value: string | null) => {
    sheetRef.current = value;
    updateCloudSheet(value);
  }, []);
  const [cloudSheets, setCloudSheets] = useState<string[]>([]);
  const [cloudUpdate, setCloudUpdate] = useState<string | null>(null);

  /** 同一个时刻只允许一个同步在跑（后台轮询撞上手动点击时直接跳过）。 */
  const busyRef = useRef(false);
  /** 后台检查失败只提示一次，别每 10 分钟刷一遍。 */
  const failRef = useRef(false);
  /** 回调放进 ref：sync 保持稳定引用，定时器不用跟着重挂。 */
  const optsRef = useRef(opts);
  optsRef.current = opts;

  const ensureApplied = async () => {
    if (appliedRef.current) return appliedRef.current;
    const cache = readCloudCache(await api.allSettings());
    if (cache.path) {
      cache.path = await api.snapshotKdocsCache(cache.path);
      await api.setSetting(KEY_KDOCS_APPLIED, JSON.stringify(cache));
    }
    appliedRef.current = cache;
    if (!sheetRef.current) setCloudSheet(cache.sheet || null);
    return cache;
  };

  const sync = useCallback(async (sheetName?: string, o?: SyncOpts) => {
    const background = o?.background ?? false;
    const applyAnyway = o?.applyAnyway ?? false;
    if (busyRef.current) return;
    busyRef.current = true;
    setSyncing(true);
    try {
      let applied: CacheState;
      try { applied = await ensureApplied(); }
      catch (e) {
        // 缓存丢失时允许重新取数；读取设置失败则不悄悄覆盖用户选择。
        const settings = await api.allSettings();
        applied = readCloudCache(settings);
        appliedRef.current = applied;
        if (!sheetRef.current) setCloudSheet(applied.sheet || null);
        if (!applyAnyway && background && optsRef.current.hasRows()) throw e;
      }
      const wanted = sheetName ?? sheetRef.current ?? undefined;
      const pending = pendingRef.current;
      const r = !background && pending && (!wanted || wanted === pending.sheet)
        ? pending : await api.syncKdocsPacking(wanted ?? null, background || sheetName === undefined);
      failRef.current = false;
      const cur = optsRef.current;
      if (background) {
        const changed = r.path !== applied.path || r.sheet !== applied.sheet || r.fver !== applied.fver;
        if (!changed && !applyAnyway) return;
        if (cur.hasExternalFile() || (!applyAnyway && !cur.autoSync)) {
          pendingRef.current = r;
          setCloudUpdate(r.fver || "待确认");
          cur.onHint(`云表格有新数据${r.fver ? `（版本 ${r.fver}）` : ""}，确认后才会替换当前表格`);
          return;
        }
      }
      const cache = { sheet: r.sheet, sheets: r.sheets, fver: r.fver, path: r.path };
      // 仅确认应用后才更新当前版本的缓存元数据。
      await api.setSetting(KEY_KDOCS_APPLIED, JSON.stringify(cache));
      cur.onApplied(r.packing);
      appliedRef.current = cache;
      setCloudSheet(r.sheet);
      setCloudSheets(r.sheets);
      pendingRef.current = null;
      setCloudUpdate(null);
      const n = r.packing.items.length;
      cur.onHint(n === 0 ? `云表格「${r.sheet.trim()}」里没有能用的出货行，换一张表试试`
        : `已从云表格同步「${r.sheet.trim()}」：${n} 条待办`);
    } catch (e) {
      const cur = optsRef.current;
      if (!background) cur.onError(e);
      else if (!failRef.current) {
        failRef.current = true;
        cur.onHint(`云表格检查失败：${cur.errorText(e)}`);
      }
    } finally {
      busyRef.current = false;
      setSyncing(false);
    }
  }, []);

  const openCached = useCallback(async () => {
    if (busyRef.current) return;
    busyRef.current = true;
    setSyncing(true);
    try {
      const cache = await ensureApplied();
      if (!cache.path) throw new Error("还没有缓存文件：先点一次「从云表格同步」");
      const sheet = await api.parsePackingXls(cache.path, cache.sheet || null);
      optsRef.current.onApplied(sheet);
      setCloudSheet(cache.sheet || null);
      setCloudSheets(cache.sheets);
      optsRef.current.onHint(`已用本地缓存「${cache.sheet}」：${sheet.items.length} 条待办（没联网）`);
    } catch (e) { optsRef.current.onError(e); }
    finally { busyRef.current = false; setSyncing(false); }
  }, []);

  /** 定时后台问版本号：没变什么都不做，变了亮提示（换不换由「有更新自动换表」决定）。 */
  useEffect(() => {
    const id = window.setInterval(() => {
      void sync(undefined, {
        background: true,
        applyAnyway: !optsRef.current.hasRows(),
      });
    }, CHECK_EVERY_MS);
    return () => window.clearInterval(id);
  }, [sync]);

  return {
    syncing,
    cloudSheet,
    cloudSheets,
    cloudUpdate,
    setCloudUpdate,
    setCloudSheet,
    setCloudSheets,
    sync,
    openCached,
  };
}
