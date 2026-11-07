import { useEffect, useRef, useState, type MutableRefObject, type Dispatch, type SetStateAction } from "react";
import { api, errorHint, errorText, type MobileRowDto } from "../../api";
import type { PackingItemDto } from "../../bindings/PackingItemDto";
import { rowKey } from "./recordKey";

type Options = {
  rowsRef: MutableRefObject<PackingItemDto[]>;
  checkedRef: MutableRefObject<Record<string, boolean>>;
  mobileVersionsRef: MutableRefObject<MobileRowDto[]>;
  sheetGenerationRef: MutableRefObject<string>;
  setRows: Dispatch<SetStateAction<PackingItemDto[]>>;
  setSavedPath: Dispatch<SetStateAction<string | null>>;
  setError: Dispatch<SetStateAction<string | null>>;
  mobilePort?: number;
};

/** 出货保存、失败重试及手机快照读取集中在此，页面只负责操作入口。 */
export function usePackingChecks(options: Options) {
  const { rowsRef, checkedRef, mobileVersionsRef, sheetGenerationRef, setRows, setSavedPath, setError, mobilePort } = options;
  const [savingChecks, setSavingChecks] = useState(false);
  const [checkFailure, setCheckFailure] = useState<{
    changes: Array<{ waybill: string; inbound: string; name: string; checked: boolean }>;
    message: string;
    sourceId: string;
  } | null>(null);
  const savingChecksRef = useRef(false);
  const checkFailureRef = useRef(false);
  const applyMobileSnapshot = (remote: MobileRowDto[]) => {
    const current = remote.filter((r) => r.sourceId === sheetGenerationRef.current);
    mobileVersionsRef.current = current;
    const byKey = new Map(current.map((r) => [rowKey(r), r.checked]));
    const next = rowsRef.current.map((r) => {
      const checked = byKey.get(rowKey(r));
      if (checked === undefined) return r;
      checkedRef.current[rowKey(r)] = checked;
      return r.checked === checked ? r : { ...r, checked };
    });
    rowsRef.current = next;
    setRows(next);
  };

  // 先保存数据库，再更新界面和本地缓存；失败保留原状态，允许整批重试。
  const saveChecks = async (
    changes: Array<{ waybill: string; inbound: string; name: string; checked: boolean }>,
    retry = false,
    sourceId = sheetGenerationRef.current,
  ): Promise<boolean> => {
    if (!changes.length) return true;
    if (sourceId !== sheetGenerationRef.current) {
      setError("表格已切换，请放弃旧操作后在当前表重新勾选");
      return false;
    }
    if (checkFailureRef.current && !retry) {
      setError("请先重试保存或放弃上次失败的出货操作");
      return false;
    }
    if (savingChecksRef.current) {
      setError("正在保存出货状态，请稍后再操作");
      return false;
    }
    const normalized = changes.map((r) => ({
      waybill: r.waybill.trim(), inbound: r.inbound.trim(), name: r.name.trim(), checked: r.checked,
    }));
    if (normalized.some((r) => r.checked && (!r.inbound || !r.waybill))) {
      setError("入库号和单号必须都填写，才能出货");
      return false;
    }
    savingChecksRef.current = true;
    setSavingChecks(true);
    try {
      if (retry) mobileVersionsRef.current = await api.mobileRows();
      await api.setPackingChecks(normalized, mobileVersionsRef.current);
      const updates = new Map(normalized.map((r) => [rowKey(r), r.checked]));
      for (const r of normalized) checkedRef.current[rowKey(r)] = r.checked;
      const next = rowsRef.current.map((r) => {
        const checked = updates.get(rowKey({ ...r, waybill: r.waybill.trim(), inbound: r.inbound.trim(), name: r.name.trim() }));
        return checked === undefined ? r : { ...r, checked };
      });
      rowsRef.current = next;
      setRows(next);
      setSavedPath(null);
      setCheckFailure(null);
      checkFailureRef.current = false;
      // 后端已同时更新数据库与手机内存；读取新版本供下一次操作使用。
      applyMobileSnapshot(await api.mobileRows().catch(() => []));
      return true;
    } catch (e) {
      if ((e as { code?: string })?.code === "conflict") {
        applyMobileSnapshot(await api.mobileRows().catch(() => []));
      }
      checkFailureRef.current = true;
      setCheckFailure({ changes: normalized, sourceId, message: errorText(e) + errorHint(e) });
      return false;
    } finally {
      savingChecksRef.current = false;
      setSavingChecks(false);
    }
  };

  /** 手机勾选已在后端落库，轮询只读取最新状态。 */
  useEffect(() => {
    if (!mobilePort) return;
    let cancelled = false;
    const pull = async () => {
      try {
        const remote = await api.mobileRows();
        if (cancelled || savingChecksRef.current || checkFailureRef.current) return;
        // 手机操作已经持久化，桌面仅展示快照，不再回写旧状态。
        applyMobileSnapshot(remote);
      } catch {
        // 手机端断开不影响电脑端
      }
    };
    pull();
    const id = setInterval(pull, 1500);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mobilePort]);
  const discardFailure = () => { checkFailureRef.current = false; setCheckFailure(null); };
  return { savingChecks, checkFailure, savingChecksRef, saveChecks, applyMobileSnapshot, discardFailure };
}
