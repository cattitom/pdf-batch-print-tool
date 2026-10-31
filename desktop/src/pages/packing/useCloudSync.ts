import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import type { PackingSheetDto } from "../../bindings/PackingSheetDto";

/** 有更新就直接换表（"1"）；默认关 = 只提示，等你点。 */
export const KEY_KDOCS_AUTO_SYNC = "kdocs_auto_sync";
/** 上次同步下载的 .xls 路径：启动时先把它解析出来，表格立刻有内容，不用先点同步。 */
export const KEY_KDOCS_LAST = "kdocs_last_xlsx";
/** 上次同步用的表名、云端版本号、这本表里所有表名（JSON 数组）。 */
export const KEY_KDOCS_SHEET = "kdocs_last_sheet";
export const KEY_KDOCS_FVER = "kdocs_last_fver";
export const KEY_KDOCS_SHEETS = "kdocs_last_sheets";

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
};

/**
 * 云表格同步：后端借已登录的 Chrome（窗口没开就自己起无界面）拉最新来货登记 → 解析成待办。
 * 负责同步状态、版本号缓存写回、10 分钟一次的后台检查；表的落地交给页面。
 */
export function useCloudSync(opts: Options): CloudSync {
  const [syncing, setSyncing] = useState(false);
  const [cloudSheet, setCloudSheet] = useState<string | null>(null);
  const [cloudSheets, setCloudSheets] = useState<string[]>([]);
  const [cloudUpdate, setCloudUpdate] = useState<string | null>(null);

  /** 同一个时刻只允许一个同步在跑（后台轮询撞上手动点击时直接跳过）。 */
  const busyRef = useRef(false);
  /** 后台检查失败只提示一次，别每 10 分钟刷一遍。 */
  const failRef = useRef(false);
  /** 回调放进 ref：sync 保持稳定引用，定时器不用跟着重挂。 */
  const optsRef = useRef(opts);
  optsRef.current = opts;

  const sync = useCallback(async (sheetName?: string, o?: SyncOpts) => {
    const background = o?.background ?? false;
    const applyAnyway = o?.applyAnyway ?? false;
    if (background && busyRef.current) return; // 别叠着拉
    busyRef.current = true;
    setSyncing(true);
    try {
      const r = await api.syncKdocsPacking(sheetName ?? null);
      setCloudSheet(r.sheet);
      setCloudSheets(r.sheets);
      // 记下这份表：下次启动先把它解析出来，进门就有内容，不用先点同步
      api.setSetting(KEY_KDOCS_SHEET, r.sheet).catch(() => {});
      api.setSetting(KEY_KDOCS_FVER, r.fver).catch(() => {});
      api.setSetting(KEY_KDOCS_LAST, r.path).catch(() => {});
      api.setSetting(KEY_KDOCS_SHEETS, JSON.stringify(r.sheets)).catch(() => {});
      const cur = optsRef.current;
      if (background && !applyAnyway) {
        if (!r.changed) return; // 云端没改，别打扰
        if (cur.hasExternalFile() || !cur.autoSync) {
          // 手上这张是你自己挑的文件，或者你没让「有更新就换」：只提示
          setCloudUpdate(r.fver);
          cur.onHint(
            `云表格有新数据${r.fver ? `（版本 ${r.fver}）` : ""}，点上面的提示换最新`,
          );
          return;
        }
      }
      cur.onApplied(r.packing);
      setCloudUpdate(null);
      const n = r.packing.items.length;
      cur.onHint(
        n === 0
          ? `云表格「${r.sheet.trim()}」里没有能用的出货行，换一张表试试`
          : `已从云表格同步「${r.sheet.trim()}」：${n} 条待办`,
      );
    } catch (e) {
      const cur = optsRef.current;
      if (background) {
        if (!failRef.current) {
          failRef.current = true;
          cur.onHint(`云表格检查失败：${cur.errorText(e)}`);
        }
        return;
      }
      cur.onError(e);
    } finally {
      busyRef.current = false;
      setSyncing(false);
    }
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
  };
}
