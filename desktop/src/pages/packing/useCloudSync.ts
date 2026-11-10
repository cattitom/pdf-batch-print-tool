import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import type { KdocsSyncDto } from "../../bindings/KdocsSyncDto";
import type { PackingSheetDto } from "../../bindings/PackingSheetDto";

/** 手动刷新始终可用；"1" 启用动态心跳。 */
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

/** 稳定时降频，失败退避；手机/桌面活跃时更快感知变化。 */
export function heartbeatDelay(active: boolean, unchanged: number, failures: number): number {
  if (failures > 0) return Math.min(15 * 60_000, 60_000 * 2 ** Math.min(failures - 1, 4));
  if (!active) return 10 * 60_000;
  return unchanged === 0 ? 60_000 : unchanged === 1 ? 2 * 60_000 : 5 * 60_000;
}

type SyncOpts = {
  refreshRemote?: boolean;
  throwOnError?: boolean;
  /** 启动 / 定时检查用：版本号没变就什么都不做；变了默认只亮提示。 */
  background?: boolean;
  /** 手里还没有表（第一次用、缓存丢了）时直接铺上去。 */
  applyAnyway?: boolean;
};

type Options = {
  /** 「有更新自动换表」勾没勾（页面 state，hook 只读）。 */
  autoSync: boolean;
  mobileActive?: () => Promise<boolean>;
  /** 拿到新表：页面负责 applySheet + 清掉「来自 OCR / 上次文件」这些标记。 */
  onApplied: (sheet: PackingSheetDto) => void | boolean | Promise<void | boolean>;
  /** 保存出货、编辑表格时暂缓换表，防止冲掉正在进行的操作。 */
  canApply?: () => boolean;
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
 * 负责同步状态、版本号缓存写回和动态心跳；表的落地交给页面。
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
  const heartbeatRef = useRef({ lastAttempt: Date.now(), unchanged: 0, failures: 0 });
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
    if (busyRef.current) {
      if (o?.throwOnError) throw new Error("电脑正在同步，请稍后重试");
      return;
    }
    busyRef.current = true;
    heartbeatRef.current.lastAttempt = Date.now();
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
      const r = !o?.refreshRemote && !background && pending && (!wanted || wanted === pending.sheet)
        ? pending : await api.syncKdocsPacking(wanted ?? null, !!o?.refreshRemote || background || sheetName === undefined);
      const changedVersion = r.fver !== applied.fver || r.sheet !== applied.sheet || r.path !== applied.path;
      heartbeatRef.current.unchanged = changedVersion ? 0 : heartbeatRef.current.unchanged + 1;
      heartbeatRef.current.failures = 0;
      failRef.current = false;
      const cur = optsRef.current;
      if (background) {
        const changed = r.path !== applied.path || r.sheet !== applied.sheet || r.fver !== applied.fver;
        if (!changed && !applyAnyway) return;
        if (cur.hasExternalFile() || (!applyAnyway && !cur.autoSync) || cur.canApply?.() === false) {
          pendingRef.current = r;
          setCloudUpdate(r.fver || "待确认");
          cur.onHint(cur.autoSync && !cur.hasExternalFile() ? "云表格有新数据，当前操作完成后自动更新" : `云表格有新数据${r.fver ? `（版本 ${r.fver}）` : ""}，确认后才会替换当前表格`);
          return;
        }
      }
      const cache = { sheet: r.sheet, sheets: r.sheets, fver: r.fver, path: r.path };
      // 仅确认应用后才更新当前版本的缓存元数据。
      await api.setSetting(KEY_KDOCS_APPLIED, JSON.stringify(cache));
      try {
        if (await cur.onApplied(r.packing) === false) throw new Error("清单未应用，保留原版本并等待重试");
      } catch (error) {
        pendingRef.current = r;
        await api.setSetting(KEY_KDOCS_APPLIED, JSON.stringify(applied));
        throw error;
      }
      appliedRef.current = cache;
      setCloudSheet(r.sheet);
      setCloudSheets(r.sheets);
      pendingRef.current = null;
      setCloudUpdate(null);
      const n = r.packing.items.length;
      cur.onHint(n === 0 ? `云表格「${r.sheet.trim()}」里没有能用的出货行，换一张表试试`
        : `已从云表格同步「${r.sheet.trim()}」：${n} 条待办`);
    } catch (e) {
      heartbeatRef.current.failures += 1;
      const cur = optsRef.current;
      if (!background) cur.onError(e);
      else if (!failRef.current) {
        failRef.current = true;
        cur.onHint(`云表格检查失败：${cur.errorText(e)}`);
      }
      if (o?.throwOnError) throw e;
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
      if (await optsRef.current.onApplied(sheet) === false) throw new Error("缓存清单未能应用");
      setCloudSheet(cache.sheet || null);
      setCloudSheets(cache.sheets);
      optsRef.current.onHint(`已用本地缓存「${cache.sheet}」：${sheet.items.length} 条待办（没联网）`);
    } catch (e) { optsRef.current.onError(e); }
    finally { busyRef.current = false; setSyncing(false); }
  }, []);

  /** 单次定时器等待请求结束再调度，避免重叠；活动唤醒也遵守最小间隔。 */
  useEffect(() => {
    if (!opts.autoSync) return;
    let cancelled = false, timer: number | undefined, localActivity = Date.now(), scheduling = false;
    const schedule = async () => {
      if (cancelled || scheduling) return;
      scheduling = true;
      try {
        const mobileActive = await optsRef.current.mobileActive?.().catch(() => false) || false;
        if (cancelled) return;
        const active = mobileActive || (!document.hidden && Date.now() - localActivity < 2 * 60_000);
        const state = heartbeatRef.current;
        const delay = heartbeatDelay(active, state.unchanged, state.failures);
        window.clearTimeout(timer);
        timer = window.setTimeout(async () => {
          if (cancelled) return;
          // 手动刷新、网络恢复等可能已检查过，不在短时间内重复访问云端。
          const current = heartbeatRef.current;
          if (Date.now() - current.lastAttempt >= heartbeatDelay(active, current.unchanged, current.failures)) {
            if (!optsRef.current.hasExternalFile()) await sync(undefined, { background: true, applyAnyway: !optsRef.current.hasRows() });
            else current.lastAttempt = Date.now();
          }
          void schedule();
        }, Math.max(busyRef.current ? 5000 : 1000, state.lastAttempt + delay - Date.now()));
      } finally { scheduling = false; }
    };
    const activity = () => { localActivity = Date.now(); void schedule(); };
    const visible = () => { if (!document.hidden) activity(); else void schedule(); };
    void schedule();
    // 只读取本机手机服务的活跃时间；此计时器不会访问云文档。
    const monitor = window.setInterval(() => void schedule(), 15000);
    window.addEventListener("online", activity);
    window.addEventListener("pointerdown", activity);
    window.addEventListener("keydown", activity);
    document.addEventListener("visibilitychange", visible);
    return () => {
      cancelled = true; window.clearTimeout(timer); window.clearInterval(monitor);
      window.removeEventListener("online", activity); window.removeEventListener("pointerdown", activity); window.removeEventListener("keydown", activity);
      document.removeEventListener("visibilitychange", visible);
    };
  }, [sync, opts.autoSync]);

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
