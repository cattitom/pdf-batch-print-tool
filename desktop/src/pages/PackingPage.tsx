import { useEffect, useMemo, useRef, useState } from "react";
import { StatsModal, type CustomerStat } from "./packing/StatsModal";
import { MobileSheetModal } from "./packing/MobileSheetModal";
import { ShipImagesModal, type ShipImageResult } from "./packing/ShipImagesModal";
import { LogModal } from "./packing/LogModal";
import {
  useCloudSync,
  KEY_KDOCS_AUTO_SYNC,
  KEY_KDOCS_LAST,
  KEY_KDOCS_SHEET,
  KEY_KDOCS_SHEETS,
} from "./packing/useCloudSync";
import { open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import {
  Alert,
  Button,
  Card,
  Checkbox,
  Drawer,
  Flex,
  Input,
  InputNumber,
  Popconfirm,
  Popover,
  Select,
  Space,
  Table,
  Tooltip,
  Typography,
} from "antd";
import {
  AppstoreOutlined,
  CloseOutlined,
  CloudSyncOutlined,
  CopyOutlined,
  DeleteOutlined,
  FileTextOutlined,
  FilterFilled,
  FolderOpenOutlined,
  HistoryOutlined,
  ImportOutlined,
  InboxOutlined,
  LoginOutlined,
  MobileOutlined,
  PieChartOutlined,
  PushpinOutlined,
  ReloadOutlined,
  SaveOutlined,
  SearchOutlined,
  UnorderedListOutlined,
} from "@ant-design/icons";
import { api, errorHint, errorText, type MobileInfoDto, type MobileRowDto } from "../api";
import type { PackingItemDto } from "../bindings/PackingItemDto";
import type { PackingSheetDto } from "../bindings/PackingSheetDto";

const { Text } = Typography;

/** 表格行 = 原始数据 + 在 rows 里的下标（编辑要改回原来那一行）。 */
type PackingRow = PackingItemDto & { __idx: number };

/** 折叠占位行：连续已录的一大段，中间用一行「已录 N 条」占位。 */
type FoldRow = {
  __fold: true;
  __foldKey: string;
  __foldCount: number;
  seq: number;
  name: string;
  qty: number;
  qtyRaw: string;
  inbound: string;
  destination: string;
  arrival: string;
  customer: string;
  waybill: string;
  checked: boolean;
  issues: string[];
};

type TableRow = PackingRow | FoldRow;

const isFold = (r: TableRow): r is FoldRow => "__fold" in r;

/** 列筛选面板：勾选立即生效，不用点「确定」；带搜索框。 */
function ColumnFilter({
  options,
  value,
  onChange,
}: {
  options: { text: string; value: string }[];
  value: string[] | null;
  onChange: (v: string[] | null) => void;
}) {
  const [q, setQ] = useState("");
  const picked = value ?? [];
  const shown = q ? options.filter((o) => o.text.toLowerCase().includes(q.toLowerCase())) : options;
  return (
    <div style={{ padding: 8, width: 236 }} onClick={(e) => e.stopPropagation()}>
      {options.length > 8 && (
        <Input
          size="small"
          allowClear
          placeholder="搜索"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          style={{ marginBottom: 6 }}
        />
      )}
      <div style={{ maxHeight: 220, overflowY: "auto" }}>
        <Checkbox.Group
          value={picked}
          onChange={(v) => onChange((v as string[]).length ? (v as string[]) : null)}
          style={{ display: "flex", flexDirection: "column", gap: 2 }}
        >
          {shown.map((o) => (
            <Checkbox key={o.value} value={o.value} style={{ marginInlineStart: 0 }}>
              {o.text}
            </Checkbox>
          ))}
        </Checkbox.Group>
      </div>
      <div style={{ marginTop: 6, textAlign: "right" }}>
        <Button size="small" type="link" onClick={() => onChange(null)}>
          清空
        </Button>
      </div>
    </div>
  );
}


/**
 * 出货清单：粘贴 Excel 表格 / 拖 xlsx·csv / 拖截图，自动拆成「目的地 + 单号」分组，
 * 结果可逐格改、可勾选，最后导出 Markdown 清单。
 */

const ISSUE_TEXT: Record<string, string> = {
  name: "商品名空",
  qty: "数量没读到",
  inbound: "入库号空",
  inbound_sci: "入库号像数值",
  inbound_inherit: "入库号沿用上行",
  dest_inherit: "目的地沿用上行",
  mark_text: "标记列含其它字",
  dup_mark: "去重后保留",
};

const IMAGE_EXT = /\.(png|jpe?g|webp|bmp|gif)$/i;

/** 真正要人工处理的：会整行标黄、计入「差异」、被「仅看差异」筛出来。
 *  「入库号空」= 客户还没做箱唛、「数量 0」= 来了还没打包，都是正常状态，不算问题。 */
const PROBLEM_ISSUES = new Set(["name", "inbound_sci"]);

/** 只是提示、不标黄：合并单元格沿用上一行属于正常情况 */
const NOTE_ISSUES = new Set(["inbound_inherit"]);

/** 备注列里要显示的全部提示（含沿用这种说明） */
const SHOWN_ISSUES = new Set([...PROBLEM_ISSUES, ...NOTE_ISSUES]);

/** 上次用文件选择器打开的目录（本机设置里）。 */
const KEY_LAST_DIR = "packing_last_dir";
/** 上次解析的文件全路径（下次启动可以自动打开）。 */
const KEY_LAST_FILE = "packing_last_file";
/** 启动时是否自动打开上次的文件："0" = 关，其余 = 开。 */
const KEY_AUTO_OPEN = "packing_auto_open";
/** 上次选中的单号（JSON 数组），下次打开还筛着那几票。 */
const KEY_PICKED = "packing_picked_waybills";
/** 钉住的货（行指纹数组），侧边栏用。 */
const KEY_PINNED = "packing_pinned";
/** 本地手改过的单号（JSON：`入库号|商品名` → 单号）。原表没写单号的行，改完能记住，不写回原表。 */
const KEY_WAYBILL_PATCH = "packing_waybill_patch";
/** 本次要出货的货：从截图里认出来的入库号（JSON 数组）。手机端按它只显示这批。 */
const KEY_TASKS = "packing_task_inbounds";
/** 任务号：按单号分组的 `[{ id: "T1", waybill, rows, boxes }]`，手机端按它认领。 */
const KEY_TASK_META = "packing_tasks";

/** 一个任务 = 一票（单号），手机端领取后去货架找。 */
type ShipTask = { id: string; waybill: string; rows: number; boxes: number };

function parseTasks(raw?: string): ShipTask[] {
  if (!raw) return [];
  try {
    const v: unknown = JSON.parse(raw);
    if (!Array.isArray(v)) return [];
    return v
      .filter((t): t is ShipTask => !!t && typeof t === "object" && typeof (t as ShipTask).id === "string")
      .map((t) => ({
        id: t.id,
        waybill: String(t.waybill ?? ""),
        rows: Number(t.rows) || 0,
        boxes: Number(t.boxes) || 0,
      }));
  } catch {
    return [];
  }
}

/** 解析「入库号数组」这种设置值，坏数据当空。 */
function parseCodeList(raw?: string): string[] {
  if (!raw) return [];
  try {
    const v: unknown = JSON.parse(raw);
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string" && !!x.trim()) : [];
  } catch {
    return [];
  }
}

/** 单号覆盖的键：入库号 + 商品名。刻意不含单号本身，改完还能认回来。 */
const waybillPatchKey = (r: { inbound: string; name: string }) =>
  `${r.inbound.trim()}|${r.name.trim()}`;

function parseWaybillPatch(raw?: string): Record<string, string> {
  if (!raw) return {};
  try {
    const v: unknown = JSON.parse(raw);
    if (v && typeof v === "object" && !Array.isArray(v)) {
      const out: Record<string, string> = {};
      for (const [k, val] of Object.entries(v as Record<string, unknown>)) {
        if (typeof val === "string" && val.trim()) out[k] = val.trim();
      }
      return out;
    }
  } catch {
    // 坏数据当没有
  }
  return {};
}

/** 既没入库号也没单号：客户还没给箱唛，货还不能发——灰着、不让标已出，但照常算未出/进手机/进导出。 */
const rowBlocked = (r: PackingItemDto) => !r.inbound.trim() && !r.waybill.trim();

/** 一行货物的稳定指纹：同一票、同入库号、同商品名，重新解析后还能认回来。 */
const rowKey = (r: PackingItemDto) =>
  `${r.waybill.trim()}|${r.inbound.trim()}|${r.name.trim()}`;

/** 按单号分块（出货是按票出的），块头带箱数和款数。 */
function toMarkdown(rows: PackingItemDto[]): string {
  const groups = new Map<string, PackingItemDto[]>();
  for (const row of rows) {
    const key = row.waybill.trim() || "未识别单号";
    const list = groups.get(key);
    if (list) list.push(row);
    else groups.set(key, [row]);
  }
  if (groups.size === 0) return "";

  const out: string[] = ["# 出货清单", ""];
  for (const [waybill, list] of groups) {
    const boxes = list.reduce((n, r) => n + r.qty, 0);
    out.push(`## 单号 ${waybill} · ${boxes} 箱 · ${list.length} 款`, "");
    for (const row of list) {
      const box = row.checked ? "x" : " ";
      out.push(
        `- [${box}] ${row.name} | 客户：${row.customer || "-"} | ${row.qty} 箱 | 入库号：${row.inbound}`
      );
    }
    out.push("");
  }
  return out.join("\n").trimEnd() + "\n";
}

/** 一票一段的「豆包粘贴稿」：按单号分块，粘给豆包就能生成可勾选的飞书待办清单。 */
function toDoubao(rows: PackingItemDto[]): { text: string; tickets: number; pieces: number } {
  const order: string[] = [];
  const groups = new Map<string, PackingItemDto[]>();
  for (const row of rows) {
    const wb = row.waybill.trim();
    // 没单号的行分不出票，粘给豆包也只会变成一锅粥
    if (!wb) continue;
    const list = groups.get(wb);
    if (list) list.push(row);
    else {
      groups.set(wb, [row]);
      order.push(wb);
    }
  }

  const blocks: string[] = [];
  let pieces = 0;
  order.forEach((wb, i) => {
    const list = groups.get(wb)!;
    const total = list.reduce((n, r) => n + r.qty, 0);
    pieces += total;
    const cust = list.map((r) => r.customer.trim()).find(Boolean) || "—";
    blocks.push(
      [
        `━━━ 第 ${i + 1} 张 · 单号 ${wb} · ${total} 件 · ${list.length} 行 ━━━`,
        `帮我新建一篇飞书文档，标题：入库核对清单 | 单号 ${wb}（${total}件）`,
        `客户：${cust} · ${list.length} 行 · ${total} 件`,
        "正文用待办清单（可勾选的那种），一条一行：",
        "",
        ...list.map((r) => `${r.name} ｜ 数量 ${r.qty} ｜ 入库号 ${r.inbound.trim() || "—"}`),
        "",
      ].join("\n")
    );
  });

  return { text: blocks.join("\n"), tickets: order.length, pieces };
}

/** 包一层「用户改过的行」：编辑后重新算 md，不回后端。 */
export default function PackingPage({
  pendingDrop,
  active = true,
}: {
  pendingDrop?: { paths: string[]; nonce: number } | null;
  /** 当前是不是停在这一页：切走时把手机出货面板收起（服务不停） */
  active?: boolean;
}) {
  const [rows, setRows] = useState<PackingItemDto[]>([]);
  const [sheet, setSheet] = useState<PackingSheetDto | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedPath, setSavedPath] = useState<string | null>(null);
  const [copyHint, setCopyHint] = useState<string | null>(null);
  /** 结果来自截图 OCR：必须提示人工核对（系统 OCR 会读丢个别格子） */
  const [fromOcr, setFromOcr] = useState(false);
  /** 单号筛选：空数组 = 全部；可多选 */
  const [pickedWaybills, setPickedWaybills] = useState<string[]>([]);
  /** 单号筛选是否已经从设置里读出来了（读出来之前别回写，否则会覆盖成空） */
  const pickedLoaded = useRef(false);
  /** 日志弹窗内容；null = 不显示 */
  const [logText, setLogText] = useState<string | null>(null);
  const [logBusy, setLogBusy] = useState(false);


  /** 出货状态记忆：指纹 → 是否已出。启动时从数据库读一次，点一下逐条写回。 */
  const checkedRef = useRef<Record<string, boolean>>({});
  /** 本地手改的单号（`入库号|商品名` → 单号），解析完铺表时套上去。 */
  const waybillPatchRef = useRef<Record<string, string>>({});
  /** 手机出货的轮询里要读最新 rows，避免闭包拿到旧表 */
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  /** 启动时是否自动打开上次的文件（默认开） */
  const [autoOpen, setAutoOpen] = useState(true);
  /** 右侧 Markdown 清单抽屉 */
  const [mdOpen, setMdOpen] = useState(false);
  /** 看板模式：按「未出 / 已出」两列铺卡片，点卡片切换 */
  const [boardMode, setBoardMode] = useState(false);
  /** 客户件数统计弹窗 */
  const [statsOpen, setStatsOpen] = useState(false);
  /** 顶部搜索词（模糊匹配商品/客户/入库号/单号） */
  const [q, setQ] = useState("");
  /** 勾了「有更新自动换表」就静默应用，否则只亮提示 */
  const [autoSync, setAutoSync] = useState(false);
  /** 右下角浮条提示（云端同步等，2.6s 后自动消失） */
  const [scanHint, setScanHint] = useState<string | null>(null);
  /** 只看有问题的行 */
  const [onlyIssues, setOnlyIssues] = useState(false);
  /** 多选的行指纹（批量标记 / 复制用） */
  const [selected, setSelected] = useState<string[]>([]);
  /** 手机出货：局域网服务信息 + 弹窗 */
  const [mobileOpen, setMobileOpen] = useState(false);
  /** 本次出货任务：截图里认出来的入库号 */
  const [taskCodes, setTaskCodes] = useState<string[]>([]);
  /** 导入出货截图弹窗：开没开 / 待识别的图 / 识别中 / 上次识别结果 */
  const [shipImagesOpen, setShipImagesOpen] = useState(false);
  /** 拖放回调里要读「弹窗开没开」，用 ref 免得闭包拿到旧值 */
  const shipOpenRef = useRef(false);
  const [shipPaths, setShipPaths] = useState<string[]>([]);
  const [shipBusy, setShipBusy] = useState(false);
  const [shipResult, setShipResult] = useState<ShipImageResult | null>(null);
  shipOpenRef.current = shipImagesOpen;
  /** 只看任务行 */
  const [taskOnly, setTaskOnly] = useState(false);
  /** 任务号（按单号分组），识别成功后生成，手机端领取用 */
  const [shipTasks, setShipTasks] = useState<ShipTask[]>([]);
  const [mobileInfo, setMobileInfo] = useState<MobileInfoDto | null>(null);
  /** 钉住的行指纹（表格上方的钉住条） */
  const [pinned, setPinned] = useState<string[]>([]);
  /** 钉住条是否展开 */
  const [pinOpen, setPinOpen] = useState(true);
  /** 钉住条目里勾选的那些（用于批量复制） */
  const [pinSelected, setPinSelected] = useState<string[]>([]);
  /** 钉住的目标行（按指纹在 rows 里查） */
  const pinnedRows = useMemo(() => {
    const byKey = new Map<string, PackingRow>();
    rows.forEach((r, i) => byKey.set(rowKey(r), { ...r, __idx: i }));
    return pinned.map((k) => byKey.get(k)).filter((r): r is PackingRow => !!r);
  }, [pinned, rows]);

  const md = useMemo(() => toMarkdown(rows), [rows]);
  const doubao = useMemo(() => toDoubao(rows), [rows]);
  const checkedCount = rows.filter((r) => r.checked).length;
  const totalQty = rows.reduce((n, r) => n + (Number.isFinite(r.qty) ? r.qty : 0), 0);
  const issueCount = rows.filter((r) => r.issues.length > 0).length;

  /** 按单号分票，值是该组在 rows 里的下标（组内保持原表顺序）。 */
  const groups = useMemo(() => {
    const map = new Map<string, number[]>();
    rows.forEach((r, i) => {
      const key = r.waybill.trim();
      const list = map.get(key);
      if (list) list.push(i);
      else map.set(key, [i]);
    });
    return [...map.entries()];
  }, [rows]);

  /** 渲染顺序：同单号挨在一起，组内保持原表顺序（编辑仍用原始下标）。 */
  const order = useMemo(() => {
    const idx = rows.map((_, i) => i);
    idx.sort((a, b) => {
      const wa = rows[a].waybill.trim();
      const wb = rows[b].waybill.trim();
      if (wa === wb) return a - b;
      if (!wa) return 1;
      if (!wb) return -1;
      return wa < wb ? -1 : 1;
    });
    return idx;
  }, [rows]);

  /** 筛选后的行序（空数组 = 全部）。空单号用 __none__ 占位，避免和「全部」的空值撞车。 */
  const shownOrder = useMemo(
    () =>
      pickedWaybills.length
        ? order.filter((i) => pickedWaybills.includes(rows[i].waybill.trim() || "__none__"))
        : order,
    [order, pickedWaybills, rows]
  );
  const shownBoxes = shownOrder.reduce((n, i) => n + rows[i].qty, 0);
  const shownChecked = shownOrder.filter((i) => rows[i].checked).length;

  /** 顶部统计：筛选时只算选中的票 */
  const stats = pickedWaybills.length
    ? { tickets: pickedWaybills.length, rows: shownOrder.length, boxes: shownBoxes, checked: shownChecked, filtered: true }
    : { tickets: groups.length, rows: rows.length, boxes: totalQty, checked: checkedCount, filtered: false };

  /** Excel 式列筛选：每个字段一组勾选值，null/空 = 不筛 */
  const [colFilters, setColFilters] = useState<Record<string, string[] | null>>({});
  const checkedFilter = colFilters.checked ?? null;
  const kw = q.trim().toLowerCase();
  const taskSet = useMemo(() => new Set(taskCodes.map((c) => c.trim())), [taskCodes]);
  /** 单号 → 任务号（T1/T2…） */
  const taskIdByWaybill = useMemo(
    () => new Map(shipTasks.map((t) => [t.waybill, t.id])),
    [shipTasks]
  );
  const visibleOrder = shownOrder.filter((i) => {
    const r = rows[i];
    if (taskOnly && !taskSet.has(r.inbound.trim())) return false;
    if (onlyIssues && !r.issues.some((c) => PROBLEM_ISSUES.has(c))) return false;
    if (kw) {
      const matched =
        r.name.toLowerCase().includes(kw) ||
        r.customer.toLowerCase().includes(kw) ||
        r.destination.toLowerCase().includes(kw) ||
        r.inbound.toLowerCase().includes(kw) ||
        r.waybill.toLowerCase().includes(kw);
      if (!matched) return false;
    }
    const hit = (key: string, value: string) => {
      const picked = colFilters[key];
      return !picked || picked.length === 0 || picked.includes(value);
    };
    return (
      hit("checked", r.checked ? "done" : "todo") &&
      hit("customer", r.customer || "(空)") &&
      hit("name", r.name) &&
      hit("destination", r.destination || "(空)") &&
      hit("inbound", r.inbound || "(空)") &&
      hit("waybill", r.waybill || "(空)")
    );
  });
  const uncheckedInView = shownOrder.filter((i) => !rows[i].checked).length;
  const onlyTodo = !!checkedFilter && checkedFilter.includes("todo") && !checkedFilter.includes("done");
  /** 有提示的行数（差异筛选按钮上的数字） */
  const issueRows = shownOrder.filter((i) => rows[i].issues.some((c) => PROBLEM_ISSUES.has(c))).length;

  /** 客户件数统计：跟着当前筛选走（单号票筛 / 搜索 / 列筛选都会算进去） */
  const customerStats = useMemo<CustomerStat[]>(() => {
    const m = new Map<string, CustomerStat>();
    for (const i of visibleOrder) {
      const r = rows[i];
      const key = (r.customer || "").trim() || "(空)";
      let e = m.get(key);
      if (!e) {
        e = { customer: key, boxes: 0, rows: 0, done: 0, left: 0, tickets: new Set<string>() };
        m.set(key, e);
      }
      e.boxes += r.qty;
      e.rows += 1;
      if (r.checked) e.done += r.qty;
      else e.left += r.qty;
      const t = r.waybill.trim();
      if (t) e.tickets.add(t);
    }
    return [...m.values()].sort((a, b) => b.boxes - a.boxes || a.customer.localeCompare(b.customer));
  }, [visibleOrder, rows]);
  const statBoxes = customerStats.reduce((n, c) => n + c.boxes, 0);

  /** 折叠已录：连续已录超过 head+tail+1 条时，中间那段收成一行占位 */
  const [foldChecked, setFoldChecked] = useState(false);
  const [expandedFolds, setExpandedFolds] = useState<string[]>([]);

  /** Table 的数据源：按单号排好序，带上原始下标；需要时把长串已录折叠掉。 */
  const data: TableRow[] = useMemo(() => {
    const out: TableRow[] = [];
    const HEAD = 2;
    const TAIL = 2;
    let i = 0;
    while (i < visibleOrder.length) {
      const idx = visibleOrder[i];
      if (!rows[idx].checked || !foldChecked) {
        out.push({ ...rows[idx], __idx: idx });
        i += 1;
        continue;
      }
      let j = i;
      while (j < visibleOrder.length && rows[visibleOrder[j]].checked) j += 1;
      const block = visibleOrder.slice(i, j);
      const key = `fold-${block[0]}`;
      if (block.length > HEAD + TAIL + 1 && !expandedFolds.includes(key)) {
        block.slice(0, HEAD).forEach((k) => out.push({ ...rows[k], __idx: k }));
        out.push({
          __fold: true,
          __foldKey: key,
          __foldCount: block.length - HEAD - TAIL,
          seq: 0,
          name: "",
          qty: 0,
          qtyRaw: "",
          inbound: "",
          destination: "",
          arrival: "",
          customer: "",
          waybill: rows[block[0]].waybill,
          checked: false,
          issues: [],
        });
        block.slice(-TAIL).forEach((k) => out.push({ ...rows[k], __idx: k }));
      } else {
        block.forEach((k) => out.push({ ...rows[k], __idx: k }));
      }
      i = j;
    }
    return out;
  }, [visibleOrder, rows, foldChecked, expandedFolds]);

  // 筛选条件一变就清空多选，避免批量操作作用到看不见的行
  useEffect(() => {
    setSelected([]);
  }, [q, pickedWaybills, onlyIssues, foldChecked]);

  /** 看板两列：未出 / 已出（跟当前单号筛选一致） */
  const pendingRows = visibleOrder
    .filter((i) => !rows[i].checked)
    .map((i) => ({ ...rows[i], __idx: i }));
  const doneRows = visibleOrder
    .filter((i) => rows[i].checked)
    .map((i) => ({ ...rows[i], __idx: i }));

  /** 每票的录入进度，按件数算：总数、已录、待录 = 总数 − 已录。 */
  const progress = groups.map(([waybill, idx]) => {
    const total = idx.reduce((n, i) => n + rows[i].qty, 0);
    const done = idx
      .filter((i) => rows[i].checked)
      .reduce((n, i) => n + rows[i].qty, 0);
    return {
      key: waybill || "__none__",
      waybill: waybill || "未识别单号",
      total,
      done,
      left: total - done,
    };
  });

  /** 已勾选行的件数合计（全部卡片用）。 */
  const doneQty = rows.filter((r) => r.checked).reduce((n, r) => n + r.qty, 0);

  const toggleWaybill = (key: string) =>
    setPickedWaybills((prev) =>
      prev.includes(key) ? prev.filter((x) => x !== key) : [...prev, key]
    );

  /** 上次点过的单号（Shift 连选用） */
  const lastPickRef = useRef<string | null>(null);

  /** 点单号 chip：普通点切换，Shift 点从上一次到这一次整段加进来。 */
  const pickWaybill = (key: string, shift: boolean) => {
    if (shift && lastPickRef.current) {
      const keys = progress.map((p) => p.key);
      const a = keys.indexOf(lastPickRef.current);
      const b = keys.indexOf(key);
      if (a >= 0 && b >= 0) {
        const range = keys.slice(Math.min(a, b), Math.max(a, b) + 1);
        setPickedWaybills((prev) => [...new Set([...prev, ...range])]);
        lastPickRef.current = key;
        return;
      }
    }
    lastPickRef.current = key;
    toggleWaybill(key);
  };

  useEffect(() => {
    if (!scanHint) return;
    const t = setTimeout(() => setScanHint(null), 2600);
    return () => clearTimeout(t);
  }, [scanHint]);

  const toggleSelect = (key: string) =>
    setSelected((prev) => (prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key]));

  /** 选中行批量标记已出（只作用于当前看得见的行）。 */
  const markSelectedDone = () => {
    const targets = data.filter(
      (d): d is PackingRow =>
        !isFold(d) && selected.includes(rowKey(d)) && !d.checked && !rowBlocked(d)
    );
    targets.forEach((d) => patch(d.__idx, { checked: true }));
    setSelected([]);
  };

  const copySelected = async () => {
    const list = rows.filter((r) => selected.includes(rowKey(r)));
    if (list.length === 0) return;
    const text = list
      .map((r) => [r.customer, r.name, String(r.qty), r.inbound, r.waybill].join("\t"))
      .join("\n");
    await copyText(text);
    setCopyHint(`已复制 ${list.length} 条`);
    setTimeout(() => setCopyHint(null), 1800);
  };

  const monoStyle = { fontFamily: "Consolas, monospace" };
  /** 列头 = 标题 + 漏斗，点漏斗在原位弹筛选面板（不跳位置）。 */
  const filterTitle = (label: string, key: string, options: { text: string; value: string }[]) => {
    const active = !!colFilters[key]?.length;
    return (
      <Space size={4}>
        <span>{label}</span>
        <Popover
          trigger="click"
          placement="bottomRight"
          arrow={false}
          styles={{ content: { padding: 0 } }}
          content={
            <ColumnFilter
              options={options}
              value={colFilters[key] ?? null}
              onChange={(v) => setColFilters((prev) => ({ ...prev, [key]: v }))}
            />
          }
        >
          <FilterFilled
            style={{ fontSize: 11, cursor: "pointer", color: active ? "#5c7fa3" : "#c9c4b9" }}
          />
        </Popover>
      </Space>
    );
  };

  /** 每列的可选值（去重排序），给 Excel 式筛选用 */
  const optionsOf = (pick: (r: PackingItemDto) => string, emptyLabel = "(空)") => {
    const set = new Set<string>();
    rows.forEach((r) => set.add(pick(r) || emptyLabel));
    return [...set].sort().map((v) => ({ text: v, value: v }));
  };
  const custOptions = optionsOf((r) => r.customer);
  const nameOptions = optionsOf((r) => r.name);
  const destOptions = optionsOf((r) => r.destination);
  const inboundOptions = optionsOf((r) => r.inbound);
  const waybillOptions = optionsOf((r) => r.waybill);

  /** 当前页可勾选的行（折叠占位行不算） */
  const selectableKeys = data.filter((d) => !isFold(d)).map((d) => rowKey(d));
  const allSelected = selectableKeys.length > 0 && selectableKeys.every((k) => selected.includes(k));
  const someSelected = selectableKeys.some((k) => selected.includes(k));


  const columns = [
    {
      title: (
        <Checkbox
          checked={allSelected}
          indeterminate={someSelected && !allSelected}
          onChange={(e) => setSelected(e.target.checked ? selectableKeys : [])}
          title="全选当前列表"
        />
      ),
      key: "sel",
      width: 40,
      align: "center" as const,
      render: (_: unknown, r: TableRow) =>
        isFold(r) ? null : (
          <Checkbox
            checked={selected.includes(rowKey(r))}
            onChange={() => toggleSelect(rowKey(r))}
          />
        ),
    },
    {
      // 本次出货任务：从截图导入时打勾，手机端默认只看这批。放最左边，扫一眼就知道哪几行要出
      title: "任务",
      key: "task",
      width: 60,
      align: "center" as const,
      render: (_: unknown, r: TableRow) => {
        if (isFold(r)) return null;
        if (!taskSet.has(r.inbound.trim())) return <span className="cell-dash">—</span>;
        const id = taskIdByWaybill.get(r.waybill.trim() || "未识别单号");
        return (
          <span className="task-mark" title={`本次任务 ${id ?? ""}（按单号分组）`}>
            {id ?? "✓"}
          </span>
        );
      },
    },
    {
      title: filterTitle("客户名称", "customer", custOptions),
      dataIndex: "customer",
      width: 108,
      render: (v: string, r: TableRow) =>
        isFold(r) ? null : (
          <Input
            variant="borderless"
            value={v}
            onChange={(e) => patch(r.__idx, { customer: e.target.value })}
          />
        ),
    },
    {
      title: filterTitle("商品名称", "name", nameOptions),
      dataIndex: "name",
      width: 190,
      render: (v: string, r: TableRow) => {
        if (isFold(r)) {
          return (
            <Button
              type="link"
              size="small"
              className="fold-btn"
              onClick={() => setExpandedFolds((prev) => [...prev, r.__foldKey])}
            >
              ⋯ 已出 {r.__foldCount} 条，点击展开 ⋯
            </Button>
          );
        }
        return (
          <Input
            variant="borderless"
            value={v}
            onChange={(e) => patch(r.__idx, { name: e.target.value })}
          />
        );
      },
    },
    {
      title: "箱数",
      dataIndex: "qty",
      width: 64,
      align: "center" as const,
      render: (v: number, r: TableRow) =>
        isFold(r) ? null : (
          <InputNumber
            variant="borderless"
            min={0}
            value={v}
            onChange={(x) => patch(r.__idx, { qty: Math.max(0, Number(x) || 0) })}
            style={{ width: "100%" }}
          />
        ),
    },
    {
      title: filterTitle("出货详情", "checked", [
          { text: "未出", value: "todo" },
          { text: "已出", value: "done" },
        ]),
      dataIndex: "checked",
      width: 92,
      render: (v: boolean, r: TableRow) =>
        isFold(r) ? null : (
          <button
            className={`ship-state${v ? " on" : ""}${rowBlocked(r) ? " blocked" : ""}`}
            disabled={!v && rowBlocked(r)}
            title={
              !v && rowBlocked(r)
                ? "没有入库号也没有单号，不能出货"
                : v
                  ? "点一下改回未出"
                  : "点一下标记已出"
            }
            onClick={() => {
              if (!v && rowBlocked(r)) return;
              patch(r.__idx, { checked: !v });
            }}
          >
            {v ? "已出" : "未出"}
          </button>
        ),
    },
    {
      // 原表的「到货情况」，插在出货详情与备注之间（2026-10-31 用户要求）
      title: "到货情况",
      dataIndex: "arrival",
      width: 104,
      render: (v: string, r: TableRow) =>
        isFold(r) ? null : (
          <Input
            variant="borderless"
            value={v}
            onChange={(e) => patch(r.__idx, { arrival: e.target.value })}
          />
        ),
    },
    {
      title: "备注",
      key: "issues",
      width: 118,
      render: (_: unknown, r: TableRow) => {
        if (isFold(r)) return null;
        const shown = r.issues.filter((c) => SHOWN_ISSUES.has(c));
        if (shown.length === 0) return <span className="cell-dash">—</span>;
        const text = ISSUE_TEXT[shown[0]] ?? shown[0];
        const isNote = shown.every((c) => NOTE_ISSUES.has(c));
        return (
          <Tooltip title={shown.map((c) => ISSUE_TEXT[c] ?? c).join("、")}>
            <span className={`issue-tag${isNote ? " note" : ""}`}>
              {text}
              {shown.length > 1 ? ` +${shown.length - 1}` : ""}
            </span>
          </Tooltip>
        );
      },
    },
    {
      // 跟云表格里的列序一致：收货仓紧挨在入库号左边
      title: filterTitle("收货仓", "destination", destOptions),
      dataIndex: "destination",
      width: 104,
      render: (v: string, r: TableRow) =>
        isFold(r) ? null : (
          <Input
            variant="borderless"
            value={v}
            onChange={(e) => patch(r.__idx, { destination: e.target.value })}
          />
        ),
    },
    {
      title: filterTitle("入库号", "inbound", inboundOptions),
      dataIndex: "inbound",
      width: 150,
      render: (v: string, r: TableRow) =>
        isFold(r) ? null : (
          <Space size={4}>
            <Input
              variant="borderless"
              value={v}
              style={monoStyle}
              onChange={(e) => patch(r.__idx, { inbound: e.target.value })}
            />
            {r.issues.includes("inbound_inherit") && (
              <Tooltip title="原表这里是合并单元格，按上一行沿用的，核对一下">
                <span className="inherit-mark">↑沿用</span>
              </Tooltip>
            )}
          </Space>
        ),
    },
    {
      title: filterTitle("单号", "waybill", waybillOptions),
      dataIndex: "waybill",
      width: 160,
      render: (v: string, r: TableRow) =>
        isFold(r) ? null : (
          <Input
            variant="borderless"
            className="wb-cell"
            value={v}
            style={monoStyle}
            onChange={(e) => setWaybill(r.__idx, e.target.value)}
          />
        ),
    },
    {
      title: (
        <Tooltip title="点行末图钉，把常看的货钉到表格上方">
          <PushpinOutlined style={{ color: "#adb2b8" }} />
        </Tooltip>
      ),
      key: "ops",
      width: 72,
      render: (_: unknown, r: TableRow) => {
        if (isFold(r)) return null;
        const key = rowKey(r);
        const isPinned = pinned.includes(key);
        return (
          <Space size={0}>
            <Button
              type="text"
              size="small"
              icon={<PushpinOutlined />}
              title={isPinned ? "取消钉住" : "钉到上方"}
              style={{ color: isPinned ? "#5c7fa3" : "#c9c4b9" }}
              onClick={() =>
                setPinned((prev) =>
                  prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key]
                )
              }
            />
            <Popconfirm
              title="删掉这一行？"
              description="只从当前清单里去掉，原 Excel 不动"
              okText="删除"
              okButtonProps={{ danger: true }}
              cancelText="取消"
              onConfirm={() => setRows((prev) => prev.filter((_, j) => j !== r.__idx))}
            >
              <Button type="text" danger size="small" icon={<DeleteOutlined />} title="删除这一行" />
            </Popconfirm>
          </Space>
        );
      },
    },
  ];

  const applySheet = (s: PackingSheetDto) => {
    // 原表里「产品名称 → 入库号」刷红字的就是已出货：这是原表作者亲手标的，比本地点出来的可信，
    // 所以红字的行直接用解析结果（本地旧记录不覆盖它）；其余行再把之前勾过的状态按指纹认回来。
    const items = s.items.map((it) => {
      // 原表没写单号的行，这里先套上本地手改的那一个（测试 / 客户事后补单号都用得上）
      const local = waybillPatchRef.current[waybillPatchKey(it)];
      const row = local ? { ...it, waybill: local } : it;
      if (row.checked) return row;
      // 单号被本地改过时，两种指纹都认：改号之前点过的「已出」不该丢
      const saved =
        checkedRef.current[rowKey(row)] ??
        (local ? checkedRef.current[rowKey(it)] : undefined);
      return saved === undefined ? row : { ...row, checked: saved };
    });
    setSheet(s);
    setRows(items);
    setSelected([]);
    setOnlyIssues(false);
    // 上次筛的单号如果这份表里没有，自动去掉，免得一打开就是空表
    const present = new Set(items.map((it) => it.waybill.trim() || "__none__"));
    setPickedWaybills((prev) => {
      const kept = prev.filter((k) => present.has(k));
      return kept.length === prev.length ? prev : kept;
    });
  };

  /** 云表格同步（状态 + 版本号缓存 + 定时检查都在 hook 里）。表的落地由 onApplied 交给页面。 */
  const {
    syncing,
    cloudSheet,
    cloudSheets,
    cloudUpdate,
    setCloudUpdate,
    setCloudSheet,
    setCloudSheets,
    sync,
    openCached,
  } = useCloudSync({
    autoSync,
    onApplied: (sheet) => {
      applySheet(sheet);
      setFromOcr(false);
      setError(null);
      setSavedPath(null);
    },
    onHint: setScanHint,
    onError: (e) => setError(errorText(e) + errorHint(e)),
    errorText,
    hasExternalFile: () => savedPath !== null,
    hasRows: () => rowsRef.current.length > 0,
  });

  /** 解析一个本地路径（文件选择器和「自动打开上次」共用）。 */
  const loadFromPath = async (path: string, silent = false) => {
    const isImage = IMAGE_EXT.test(path);
    try {
      setBusy(true);
      const s = isImage ? await api.parsePackingImage(path) : await api.parsePackingFile(path);
      applySheet(s);
      setFromOcr(isImage);
      setError(null);
      setSavedPath(null);
      api.setSetting(KEY_LAST_FILE, path).catch(() => {
        // 记不住路径不影响这次解析
      });
    } catch (e) {
      if (!silent) setError(errorText(e) + errorHint(e));
    } finally {
      setBusy(false);
    }
  };

  /** 进页面：先把上次同步的表铺上（不用点），再后台问一句云端有没有新版。
   *  没表可铺（第一次用、缓存被清了）就直接拉一次铺上。 */
  useEffect(() => {
    let alive = true;
    void (async () => {
      let s: Record<string, string> = {};
      try {
        s = await api.allSettings();
      } catch {
        // 读不到设置就当「有更新自动换表」是关的
      }
      if (!alive) return;
      waybillPatchRef.current = parseWaybillPatch(s[KEY_WAYBILL_PATCH]);
      setTaskCodes(parseCodeList(s[KEY_TASKS]));
      setShipTasks(parseTasks(s[KEY_TASK_META]));
      setAutoSync(s[KEY_KDOCS_AUTO_SYNC] === "1");
      // 上面那个「启动时打开上次文件」要是会打开文件，就别用云缓存抢它的位置
      const localFile = s[KEY_AUTO_OPEN] !== "0" && !!s[KEY_LAST_FILE];
      const lastPath = localFile ? "" : s[KEY_KDOCS_LAST];
      let restored = false;
      if (lastPath) {
        // 先把「已录」记忆读出来，铺表时才能把勾选认回来（跟另一个挂载 effect 读的是同一份）
        try {
          const list = await api.listPackingChecks();
          const m: Record<string, boolean> = {};
          for (const c of list) m[`${c.waybill}|${c.inbound}|${c.name}`] = c.checked;
          checkedRef.current = m;
        } catch {
          // 读不到就按表里自带的来
        }
        try {
          // .xls 缓存要按上次那张表恢复（表按月排，不指定就会跳到最新那张）
          const wantSheet = (s[KEY_KDOCS_SHEET] || "").trim();
          const sheet = /\.xls$/i.test(lastPath)
            ? await api.parsePackingXls(lastPath, wantSheet || null)
            : await api.parsePackingFile(lastPath);
          if (!alive) return;
          applySheet(sheet);
          setFromOcr(false);
          restored = true;
          setCloudSheet(s[KEY_KDOCS_SHEET] || null);
          try {
            const list: unknown = JSON.parse(s[KEY_KDOCS_SHEETS] || "[]");
            if (Array.isArray(list)) {
              setCloudSheets(list.filter((x): x is string => typeof x === "string"));
            }
          } catch {
            // 表名列表坏了不影响用表
          }
          const name = (s[KEY_KDOCS_SHEET] || "").trim();
          setScanHint(
            `已载入上次同步的云表格${name ? `「${name}」` : ""}：${sheet.items.length} 条待办`,
          );
        } catch {
          // 缓存文件被清了 / 读不了：下面直接重新拉一次
        }
      }
      if (!alive) return;
      void sync(undefined, { background: true, applyAnyway: !restored });
    })();
    return () => {
      alive = false;
    };
    // 只在进页面时跑一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** 文件选择器：图片走 OCR，xlsx / csv / txt 直接解析；打开完记住这个目录。 */
  const pickFile = async () => {
    let defaultPath: string | undefined;
    try {
      const saved = await api.allSettings();
      defaultPath = saved[KEY_LAST_DIR] || undefined;
    } catch {
      // 读不到设置就不带默认目录
    }
    const picked = await openFileDialog({
      multiple: false,
      directory: false,
      defaultPath,
      filters: [
        {
          name: "表格 / 截图",
          extensions: ["xlsx", "xls", "csv", "txt", "png", "jpg", "jpeg", "webp", "bmp"],
        },
      ],
    });
    if (typeof picked !== "string" || !picked) return;

    const dir = picked.replace(/[\\/][^\\/]+$/, "");
    if (dir && dir !== picked) {
      api.setSetting(KEY_LAST_DIR, dir).catch(() => {
        // 记不住目录不影响这次解析
      });
    }
    await loadFromPath(picked);
  };

  /** 重新解析上次打开过的文件：文件更新了不用再选一遍。 */
  const refreshLast = async () => {
    let last = "";
    try {
      const saved = await api.allSettings();
      last = saved[KEY_LAST_FILE] ?? "";
    } catch {
      // 读不到设置就当没有记录
    }
    if (!last) {
      setError("还没有打开过文件，先点「选择文件」");
      return;
    }
    await loadFromPath(last);
  };

  /** 清空这次的结果（不动「已录」记忆）。 */
  const clearAll = () => {
    setSheet(null);
    setRows([]);
    setSelected([]);
    setOnlyIssues(false);
    setError(null);
    setSavedPath(null);
    setFromOcr(false);
  };

  /** 挂载时：先读「已录」记忆，再看是否自动打开上次的文件。 */
  useEffect(() => {
    (async () => {
      try {
        const list = await api.listPackingChecks();
        const m: Record<string, boolean> = {};
        for (const c of list) {
          m[`${c.waybill}|${c.inbound}|${c.name}`] = c.checked;
        }
        checkedRef.current = m;
      } catch {
        // 读不到就没有记忆，按表格自带的标记来
      }

      let saved: Record<string, string> = {};
      try {
        saved = await api.allSettings();
      } catch {
        // 读不到设置就按默认（自动打开）
      }
      const on = saved[KEY_AUTO_OPEN] !== "0";
      setAutoOpen(on);

      // 恢复上次筛的单号（解析完会自动滤掉这份表里没有的）
      try {
        const arr = JSON.parse(saved[KEY_PICKED] ?? "[]");
        if (Array.isArray(arr)) {
          setPickedWaybills(arr.filter((x): x is string => typeof x === "string"));
        }
      } catch {
        // 存坏了就当没筛过
      }
      pickedLoaded.current = true;

      try {
        const arr = JSON.parse(saved[KEY_PINNED] ?? "[]");
        if (Array.isArray(arr)) setPinned(arr.filter((x): x is string => typeof x === "string"));
      } catch {
        // 存坏了就当没钉过
      }

      const last = saved[KEY_LAST_FILE];
      if (on && last) {
        await loadFromPath(last, true);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** 单号筛选变了就记下来（读设置之前不回写，免得覆盖成空）。 */
  useEffect(() => {
    if (!pickedLoaded.current) return;
    api.setSetting(KEY_PICKED, JSON.stringify(pickedWaybills)).catch(() => {
      // 记不住不影响当前筛选
    });
  }, [pickedWaybills]);

  /** 钉住的货变了就记下来。 */
  useEffect(() => {
    api.setSetting(KEY_PINNED, JSON.stringify(pinned)).catch(() => {
      // 记不住不影响当前使用
    });
  }, [pinned]);

  /** 拖进来的文件：图片走系统 OCR，xlsx / csv / txt 直接解析。 */
  useEffect(() => {
    const paths = pendingDrop?.paths ?? [];
    if (paths.length === 0) return;
    // 导入出货截图的弹窗开着：拖进来的图归它，别当成整张表去解析
    if (shipOpenRef.current) {
      const imgs = paths.filter((p) => IMAGE_EXT.test(p));
      if (imgs.length > 0) {
        setShipPaths((prev) => [...prev, ...imgs.filter((p) => !prev.includes(p))]);
        return;
      }
    }
    const path = paths[0];
    const isImage = IMAGE_EXT.test(path);
    let cancelled = false;
    (async () => {
      try {
        setBusy(true);
        const s = isImage ? await api.parsePackingImage(path) : await api.parsePackingFile(path);
        if (cancelled) return;
        applySheet(s);
        setFromOcr(isImage);
        setError(null);
        setSavedPath(null);
        api.setSetting(KEY_LAST_FILE, path).catch(() => {
          // 记不住路径不影响这次解析
        });
      } catch (e) {
        if (!cancelled) setError(errorText(e) + errorHint(e));
      } finally {
        if (!cancelled) setBusy(false);
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingDrop?.nonce]);

  const patch = (i: number, p: Partial<PackingItemDto>) => {
    setRows((prev) => prev.map((r, j) => (j === i ? { ...r, ...p } : r)));
    // 勾选状态按指纹记进数据库，下次打开同一份表还能认回来
    if (p.checked !== undefined) {
      const row = rows[i];
      if (row) {
        checkedRef.current[rowKey({ ...row, ...p })] = p.checked;
        api
          .setPackingCheck(row.waybill.trim(), row.inbound.trim(), row.name.trim(), p.checked)
          .catch(() => {
            // 写失败下次重勾一次就行
          });
        // 手机端服务在跑就顺手告诉它，省得整表 sync 覆盖手机刚勾的状态
        // （面板收起也算在跑，不能用 mobileOpen 判断）
        if (mobileInfo) {
          api
            .mobileSetCheck(row.waybill.trim(), row.inbound.trim(), row.name.trim(), p.checked)
            .catch(() => {});
        }
      }
    }
    setSavedPath(null);
  };

  /** 改单号：同时记进数据库（`入库号|商品名` → 单号），重新解析 / 重启还能认回来。
   *  清空这一格就删掉覆盖，回到原表的空单号。 */
  const setWaybill = (i: number, value: string) => {
    const before = rowsRef.current[i];
    patch(i, { waybill: value });
    if (!before) return;
    const key = waybillPatchKey(before);
    const next = { ...waybillPatchRef.current };
    const v = value.trim();
    if (v) next[key] = v;
    else delete next[key];
    waybillPatchRef.current = next;
    api.setSetting(KEY_WAYBILL_PATCH, JSON.stringify(next)).catch(() => {
      // 记不住不影响这次改动
    });
  };

  /** 往剪贴板写文本（WebView 里 clipboard 可能没权限，回退到 textarea） */
  const copyText = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
    }
  };

  /** 复制选中的钉住条目：制表符分隔，贴 Excel 能自动分列 */
  const copyPinned = async (keys: string[]) => {
    const list = pinnedRows.filter((r) => keys.includes(rowKey(r)));
    if (list.length === 0) return;
    const text = list
      .map((r) => [r.customer, r.name, String(r.qty), r.inbound, r.waybill].join("\t"))
      .join("\n");
    await copyText(text);
    setCopyHint(`已复制 ${list.length} 条`);
    setTimeout(() => setCopyHint(null), 1800);
  };

  const copyMd = async () => {
    if (!md) return;
    try {
      await navigator.clipboard.writeText(md);
      setCopyHint("已复制");
    } catch {
      // WebView 里 clipboard 权限可能没有：退回到选中 textarea
      const ta = document.createElement("textarea");
      ta.value = md;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
      setCopyHint("已复制");
    }
    setTimeout(() => setCopyHint(null), 1800);
  };

  const saveMd = async () => {
    if (!md) return;
    try {
      const day = new Date().toISOString().slice(0, 10);
      const out = await saveFileDialog({
        defaultPath: `装箱单待办_${sheet?.waybill || day}.md`,
        filters: [{ name: "Markdown", extensions: ["md"] }],
      });
      if (!out) return;
      await api.saveTextFile(out, md);
      setSavedPath(out);
      setError(null);
    } catch (e) {
      setError(errorText(e) + errorHint(e));
    }
  };

  /** 导出「豆包粘贴稿」：按票一段，粘给豆包生成飞书待办清单 */
  const exportDoubao = async () => {
    if (doubao.tickets === 0) {
      setError("当前表里没有带单号的票，导不出清单");
      return;
    }
    try {
      const day = new Date().toISOString().slice(0, 10);
      const who = (cloudSheet || "").trim() || day;
      const out = await saveFileDialog({
        defaultPath: `豆包清单_${who}.txt`,
        filters: [{ name: "文本", extensions: ["txt"] }],
      });
      if (!out) return;
      await api.saveTextFile(out, doubao.text);
      setSavedPath(out);
      setScanHint(`已导出 ${doubao.tickets} 票 · ${doubao.pieces} 件，可以粘给豆包了`);
      setError(null);
    } catch (e) {
      setError(errorText(e) + errorHint(e));
    }
  };

  const openLog = async () => {
    setLogBusy(true);
    try {
      setLogText(await api.readLogTail(500));
    } catch (e) {
      setLogText(`读取日志失败：${errorText(e)}${errorHint(e)}`);
    } finally {
      setLogBusy(false);
    }
  };

  const copyLog = async () => {
    if (!logText) return;
    try {
      await navigator.clipboard.writeText(logText);
      setCopyHint("日志已复制");
      setTimeout(() => setCopyHint(null), 1800);
    } catch {
      setError("复制失败，可以点「打开日志目录」直接看文件");
    }
  };

  /** 客户件数统计复制成 TSV，粘到 Excel 直接拆列 */
  const copyStats = async () => {
    const head = ["客户", "票数", "款数", "总件数", "已出", "未出"];
    const lines = [head.join("\t")];
    for (const c of customerStats) {
      lines.push([c.customer, c.tickets.size, c.rows, c.boxes, c.done, c.left].join("\t"));
    }
    lines.push(
      [
        "合计",
        customerStats.reduce((n, c) => n + c.tickets.size, 0),
        customerStats.reduce((n, c) => n + c.rows, 0),
        statBoxes,
        customerStats.reduce((n, c) => n + c.done, 0),
        customerStats.reduce((n, c) => n + c.left, 0),
      ].join("\t")
    );
    try {
      await copyText(lines.join("\n"));
      setCopyHint(`已复制 ${customerStats.length} 个客户的件数`);
      setTimeout(() => setCopyHint(null), 1800);
    } catch {
      setError("复制失败，可以直接截图或手抄");
    }
  };

  const mobilePayload = (): MobileRowDto[] =>
    rowsRef.current.map((r) => ({
      waybill: r.waybill,
      inbound: r.inbound,
      name: r.name,
      customer: r.customer,
      qty: r.qty,
      checked: r.checked,
      issues: r.issues,
      blocked: rowBlocked(r),
      task: taskSet.has(r.inbound.trim()),
      taskId: taskIdByWaybill.get(r.waybill.trim() || "未识别单号") ?? "",
      claimedBy: "",
    }));

  /** 只把手机服务拉起来，不碰面板；silent = 启动时自动开，失败不吵人 */
  const startMobileService = async (silent = false): Promise<boolean> => {
    if (mobileInfo) return true;
    try {
      const info = await api.mobileStart(mobilePayload());
      setMobileInfo(info);
      return true;
    } catch (e) {
      if (!silent) setError(errorText(e));
      return false;
    }
  };

  /** 点按钮：确保服务在跑，然后把面板拉出来（服务早就起好了也不重启） */
  const openMobile = async () => {
    setError(null);
    if (await startMobileService()) setMobileOpen(true);
  };

  const stopMobile = async () => {
    setMobileOpen(false);
    setMobileInfo(null);
    try {
      await api.mobileStop();
    } catch {
      // 服务已经停了就当成功
    }
  };

  /** 软件一启动就把手机服务挂起来（面板不弹）；用户点过「停止手机端」就不再自动拉起 */
  const autoMobileRef = useRef(false);
  useEffect(() => {
    if (autoMobileRef.current) return;
    autoMobileRef.current = true;
    void startMobileService(true);
    // 只在挂载时跑一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** 切到别的页签就收起面板（服务继续跑，手机那边不受影响） */
  useEffect(() => {
    if (!active) setMobileOpen(false);
  }, [active]);

  const copyMobileUrl = async () => {
    if (!mobileInfo) return;
    try {
      await navigator.clipboard.writeText(mobileInfo.url);
      setCopyHint("链接已复制");
      setTimeout(() => setCopyHint(null), 1800);
    } catch {
      setError("复制失败，直接手输上面的地址");
    }
  };

  /** 打开「导入出货截图」弹窗（拖图 / 选图都在里面） */
  const openShipImages = () => {
    if (rows.length === 0) {
      setError("先把表铺出来（同步 / 本地缓存 / 选择文件），再导入出货截图");
      return;
    }
    // 每次打开都是新的一批：别把上次的图和结果留在窗口里
    setShipPaths([]);
    setShipResult(null);
    setShipImagesOpen(true);
  };

  /** 弹窗里点「选文件」：多选图片，追加到待识别列表 */
  const pickShipImages = async () => {
    try {
      const picked = await openFileDialog({
        multiple: true,
        directory: false,
        filters: [{ name: "出货截图", extensions: ["png", "jpg", "jpeg", "webp", "bmp"] }],
      });
      const list = Array.isArray(picked) ? picked : typeof picked === "string" && picked ? [picked] : [];
      if (list.length === 0) return;
      setShipPaths((prev) => [...prev, ...list.filter((p) => !prev.includes(p))]);
    } catch (e) {
      setError(errorText(e));
    }
  };

  /** 识别这批截图：认入库号 → 匹配当前表 → 打任务；对不上的当场报出来 */
  const runShipImages = async () => {
    const list = shipPaths;
    if (list.length === 0) return;
    setShipBusy(true);
    try {
      const r = await api.parseShipImages(list);
      const rowsNow = rowsRef.current;
      const known = new Set(rowsNow.map((x) => x.inbound.trim()));
      const matched = r.codes.filter((c) => known.has(c));
      const unknown = r.codes.filter((c) => !known.has(c));
      // 按单号分组成任务：一个单号一个 task，编号 T1、T2…
      // 没有单号的行不建任务 —— 手机上是按票领取的，挂到「未识别单号」那一堆里既没法领也容易领错
      const byWb = new Map<string, { rows: number; boxes: number }>();
      let noWaybill = 0;
      for (const code of matched) {
        const item = rowsNow.find((x) => x.inbound.trim() === code);
        if (!item) continue;
        const wb = item.waybill.trim();
        if (!wb) {
          noWaybill += 1;
          continue;
        }
        const agg = byWb.get(wb) ?? { rows: 0, boxes: 0 };
        agg.rows += 1;
        agg.boxes += item.qty;
        byWb.set(wb, agg);
      }
      const tasks: ShipTask[] = [...byWb.entries()].map(([wb, agg], i) => ({
        id: `T${i + 1}`,
        waybill: wb,
        rows: agg.rows,
        boxes: agg.boxes,
      }));
      setTaskCodes(matched);
      setShipTasks(tasks);
      setTaskOnly(true);
      setShipResult({ total: r.codes.length, matched: matched.length, unknown, failed: r.failed });
      api.setSetting(KEY_TASKS, JSON.stringify(matched)).catch(() => {});
      api.setSetting(KEY_TASK_META, JSON.stringify(tasks)).catch(() => {});
      setScanHint(
        matched.length > 0
          ? `本批 ${tasks.length} 个任务 / ${matched.length} 行${
              noWaybill > 0 ? `（${noWaybill} 行没单号，不生成任务）` : ""
            }，手机端点「任务」就能领`
          : "这批图没匹配到任何行，检查一下图里的入库号",
      );
      // 认出来了就收窗：结果进表格 + 浮条提示；一行都没匹配上才留着窗口看「原表没有」
      if (matched.length > 0) {
        setShipImagesOpen(false);
        setShipPaths([]);
      }
    } catch (e) {
      setError(errorText(e));
    } finally {
      setShipBusy(false);
    }
  };

  /** 清掉本次任务，回到看整张表 */
  const clearTasks = () => {
    setTaskCodes([]);
    setShipTasks([]);
    setTaskOnly(false);
    api.setSetting(KEY_TASKS, "[]").catch(() => {});
    api.setSetting(KEY_TASK_META, "[]").catch(() => {});
  };

  /** 打开 WPS 登录窗口：脚本随程序发（data/kdocs/tools/kdocs-login.bat），不用自己找 */
  const openKdocsLogin = async () => {
    try {
      await api.kdocsLoginOpen();
    } catch (e) {
      setError(errorText(e));
    }
  };

  /** 一键放行防火墙：后端弹一次 UAC 跑 netsh，结果既显示在弹窗里，也走右下角浮条 */
  const allowFirewall = async () => {
    try {
      const msg = await api.mobileAllowFirewall();
      setScanHint(msg);
      return msg;
    } catch (e) {
      const msg = errorText(e);
      setScanHint(msg);
      throw new Error(msg);
    }
  };

  /** 换新口令：让已经存过链接的手机失效，重新扫码 */
  const resetMobile = async () => {
    if (!confirm("换新口令后，已经存过链接的手机要重新扫一次。继续？")) return;
    try {
      const info = await api.mobileResetToken();
      setMobileInfo(info);
      setCopyHint("口令已换");
      setTimeout(() => setCopyHint(null), 1800);
    } catch (e) {
      setError(errorText(e));
    }
  };

  /** 表结构变了（换表、改名字/数量）才推给手机；勾选由两边各自轮询 */
  const mobileSig = rows.map((r) => `${r.waybill}|${r.inbound}|${r.name}|${r.qty}`).join("~");
  useEffect(() => {
    if (!mobileInfo) return;
    const timer = setTimeout(() => {
      api.mobileSync(mobilePayload()).catch(() => {});
    }, 300);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mobileSig, mobileOpen]);

  /** 手机勾的变化 → 桌面端落库（和本地勾选走同一个 patch） */
  useEffect(() => {
    if (!mobileInfo) return;
    let cancelled = false;
    const pull = async () => {
      try {
        const remote = await api.mobileRows();
        if (cancelled) return;
        for (const m of remote) {
          const idx = rowsRef.current.findIndex(
            (r) => r.waybill === m.waybill && r.inbound === m.inbound && r.name === m.name
          );
          if (idx >= 0 && rowsRef.current[idx].checked !== m.checked) {
            patch(idx, { checked: m.checked });
          }
        }
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
  }, [mobileOpen]);

  return (
    <div className="page wide packing-page">
      <header className="page-head">
        <div>
          <h1 className="page-title">出货清单</h1>
          <p className="page-sub">拖入 xlsx、csv 或截图，按「目的地 + 单号」拆成出货待办</p>
        </div>
        <div className="page-actions">
          <button
            className="btn"
            onClick={() => sync()}
            disabled={busy || syncing}
            title="借 kdocs-login.bat 打开的登录窗口，把 WPS 云表格的最新来货登记拉下来直接解析"
          >
            <CloudSyncOutlined />
            {syncing ? "同步中…" : "从云表格同步"}
          </button>
          <button
            className="btn"
            onClick={() => void openKdocsLogin()}
            disabled={busy || syncing}
            title="打开 WPS 云表格登录窗口：扫码登录一次，之后同步会自动复用登录态"
          >
            <LoginOutlined />
            WPS 登录
          </button>
          <button
            className="btn"
            onClick={() => void openCached()}
            disabled={busy || syncing}
            title="不联网：直接解析上次同步下载的 .xls 缓存（测试省得每次去云端拉）"
          >
            <HistoryOutlined />
            本地缓存
          </button>
          {cloudSheets.length > 0 && (
            <Select
              size="small"
              style={{ width: 132 }}
              value={cloudSheet ?? undefined}
              onChange={(v) => sync(v)}
              disabled={syncing}
              placeholder="换一张表"
              options={cloudSheets.map((s) => ({ label: s.trim(), value: s }))}
            />
          )}
          <button
            className="btn"
            onClick={openMobile}
            disabled={rows.length === 0}
            title="手机连同一个 Wi-Fi，扫码就能勾选"
          >
            <MobileOutlined />
            手机出货
          </button>
          <button className="btn" onClick={openLog} disabled={logBusy}>
            <FileTextOutlined />
            日志
          </button>
          <button
            className="btn"
            onClick={() => setStatsOpen(true)}
            disabled={rows.length === 0}
            title="按客户汇总件数（跟着当前筛选走）"
          >
            <PieChartOutlined />
            统计
          </button>
          <button
            className="btn"
            onClick={exportDoubao}
            disabled={rows.length === 0}
            title="按单号一票一段，导出能直接粘给豆包生成飞书待办清单的文本"
          >
            <CopyOutlined />
            豆包清单
          </button>
          <button
            className="btn"
            onClick={openShipImages}
            disabled={busy || rows.length === 0}
            title="选几张「本次要出货」的截图，认出里面的入库号并匹配成任务（手机端只看这批）"
          >
            <ImportOutlined />
            导入出货图
          </button>
          {taskCodes.length > 0 && (
            <button
              className={`btn${taskOnly ? " primary" : ""}`}
              onClick={() => setTaskOnly((v) => !v)}
              title="只看本次任务的行"
            >
              只看任务 {taskCodes.length}
            </button>
          )}
          {taskCodes.length > 0 && (
            <button className="btn" onClick={clearTasks} title="清掉本次任务">
              清任务
            </button>
          )}
          <button className="btn primary" onClick={pickFile} disabled={busy}>
            <FolderOpenOutlined />
            {busy ? "解析中…" : "选择文件"}
          </button>
        </div>
      </header>

      {scanHint && (
        <div className="toast" role="status">
          {scanHint}
        </div>
      )}

      <div className="packing-layout">
      <div className="packing-main">
    <div className="stack">
      <div className="card">
        <div className="card-body tight" style={{ padding: "10px 12px" }}>
          <div className="toolbar">
            <Input
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder="搜索商品 / 客户 / 入库号 / 单号"
              prefix={<SearchOutlined style={{ color: "#9aa3b2" }} />}
              allowClear
              style={{ width: 280 }}
            />
            <Button icon={<ReloadOutlined />} onClick={refreshLast} disabled={busy}>
              刷新
            </Button>
            <Button onClick={clearAll} disabled={rows.length === 0}>
              清空
            </Button>
            <div style={{ flex: 1 }} />
            <Checkbox
              checked={autoOpen}
              onChange={(e) => {
                const on = e.target.checked;
                setAutoOpen(on);
                api.setSetting(KEY_AUTO_OPEN, on ? "1" : "0").catch(() => {
                  // 记不住就下次按默认来
                });
              }}
            >
              启动时打开上次文件
            </Checkbox>
            <Checkbox
              checked={autoSync}
              onChange={(e) => {
                const on = e.target.checked;
                setAutoSync(on);
                api.setSetting(KEY_KDOCS_AUTO_SYNC, on ? "1" : "0").catch(() => {
                  // 记不住就下次按默认来（关）
                });
                // 刚勾上就把云端最新的一张铺上去，省得还要再点一次
                if (on) void sync(undefined, { background: true, applyAnyway: true });
              }}
            >
              有更新自动换表
            </Checkbox>
            <Button
              icon={<AppstoreOutlined />}
              type={boardMode ? "primary" : "default"}
              onClick={() => setBoardMode((v) => !v)}
              disabled={rows.length === 0}
            >
              看板
            </Button>
            <Button
              icon={<UnorderedListOutlined />}
              onClick={() => setMdOpen(true)}
              disabled={rows.length === 0}
            >
              清单
            </Button>
            {pinnedRows.length > 0 && !pinOpen && (
              <Button icon={<PushpinOutlined />} onClick={() => setPinOpen(true)}>
                钉住 {pinnedRows.length}
              </Button>
            )}
          </div>
        </div>
      </div>

      {pinnedRows.length > 0 && pinOpen && (
        <div className="card pin-strip">
          <div className="card-head">
            <span className="card-title">
              <PushpinOutlined />
              钉住
            </span>
            <span className="card-sub">{pinnedRows.length} 条 · 表格行末的图钉加进来</span>
            <div style={{ flex: 1 }} />
            <Checkbox
              checked={pinSelected.length === pinnedRows.length}
              indeterminate={pinSelected.length > 0 && pinSelected.length < pinnedRows.length}
              onChange={(e) =>
                setPinSelected(e.target.checked ? pinnedRows.map((r) => rowKey(r)) : [])
              }
            >
              全选
            </Checkbox>
            <button
              className="btn sm"
              disabled={pinSelected.length === 0}
              onClick={() => copyPinned(pinSelected)}
            >
              <CopyOutlined />
              复制{pinSelected.length ? ` ${pinSelected.length}` : ""}
            </button>
            <button className="btn sm" onClick={() => setPinOpen(false)} title="收起钉住条">
              收起
            </button>
          </div>
          <div className="pin-strip-body">
            {pinnedRows.map((r) => (
              <div key={rowKey(r)} className="pin-chip">
                <Checkbox
                  checked={pinSelected.includes(rowKey(r))}
                  onChange={(e) =>
                    setPinSelected((prev) =>
                      e.target.checked ? [...prev, rowKey(r)] : prev.filter((k) => k !== rowKey(r))
                    )
                  }
                />
                <div
                  className="pin-chip-main"
                  title={`点击复制这一条\n${r.name} · ${r.customer} · ${r.inbound} · ${r.waybill}`}
                  onClick={() => copyPinned([rowKey(r)])}
                >
                  <div className="pin-chip-top">
                    <span className="pin-chip-name">{r.name}</span>
                    <span className="pin-chip-qty">{r.qty} 件</span>
                  </div>
                  <div className="pin-chip-meta">
                    {r.customer || "—"} · <span className="mono">{r.inbound}</span>
                  </div>
                </div>
                <button
                  className={`pin-flag${r.checked ? " on" : ""}${rowBlocked(r) ? " blocked" : ""}`}
                  disabled={!r.checked && rowBlocked(r)}
                  title={
                    !r.checked && rowBlocked(r)
                      ? "没有入库号也没有单号，不能出货"
                      : r.checked
                        ? "点一下改回未出"
                        : "点一下标记已出"
                  }
                  onClick={() => {
                    if (!r.checked && rowBlocked(r)) return;
                    patch(r.__idx, { checked: !r.checked });
                  }}
                >
                  {r.checked ? "已出" : "未出"}
                </button>
                <button
                  className="icon-btn"
                  title="取消钉住"
                  onClick={() => setPinned((prev) => prev.filter((k) => k !== rowKey(r)))}
                >
                  <CloseOutlined />
                </button>
              </div>
            ))}
          </div>
        </div>
      )}

      {rows.length === 0 && (
        <div className="empty lg" onClick={pickFile}>
          <div className="empty-icon">
            <InboxOutlined />
          </div>
          <div className="empty-title">把表格或截图拖进来</div>
          <div className="empty-sub">
            支持 xlsx / csv / txt / PNG / JPG · 也可以点这里选文件 · 解析结果按单号分组、可逐条勾选
          </div>
          <div className="empty-actions">
            <button
              className="btn primary"
              onClick={(e) => {
                e.stopPropagation();
                pickFile();
              }}
            >
              <FolderOpenOutlined />
              选择表格
            </button>
          </div>
        </div>
      )}

      {fromOcr && rows.length > 0 && (
        <Alert
          type="warning"
          showIcon
          message={
            sheet?.engine === "paddle"
              ? "这结果来自 PaddleOCR（离线运行）"
              : "这结果来自 Windows 自带 OCR"
          }
          description={
            sheet?.engine === "paddle"
              ? "文字和数字基本都能读到，仍建议扫一眼标黄行（多为合并单元格的入库号）。"
              : "没装 PaddleOCR 模型时会读丢个别格子，标黄行必须对照原图改一下。"
          }
        />
      )}

      {sheet?.warnings?.length ? (
        <Alert type="warning" showIcon message={sheet.warnings.join("；")} />
      ) : null}

      {rows.length > 0 && (
        <Card
          size="small"
          title={
            <Space size={12}>
              <span>明细</span>
              <Text type="secondary" style={{ fontSize: 12, fontWeight: 400 }}>
                {stats.tickets} 票 · {stats.boxes} 件
              </Text>
              {sheet?.expectedQty != null && (
                <Text
                  type={sheet.expectedQty === totalQty ? "success" : "warning"}
                  style={{ fontSize: 12, fontWeight: 400 }}
                >
                  表里合计 {sheet.expectedQty} 件
                  {sheet.expectedQty === totalQty ? "（一致）" : "（不符）"}
                </Text>
              )}
              {issueCount > 0 && (
                <Text type="warning" style={{ fontSize: 12, fontWeight: 400 }}>
                  {issueRows} 行待确认
                </Text>
              )}
            </Space>
          }
          extra={
            <Space size={12}>
              {boardMode && (
                <Checkbox
                  checked={onlyTodo}
                  onChange={(e) =>
                    setColFilters((prev) => ({ ...prev, checked: e.target.checked ? ["todo"] : null }))
                  }
                >
                  只看未录{uncheckedInView ? `（${uncheckedInView}）` : ""}
                </Checkbox>
              )}
              <button
                className={`btn sm${onlyIssues ? " primary" : ""}`}
                disabled={issueRows === 0}
                onClick={() => setOnlyIssues((v) => !v)}
                title="只看有问题的行：缺入库号、数量没读到等"
              >
                仅看差异{issueRows > 0 ? ` ${issueRows}` : ""}
              </button>
              <Checkbox
                checked={foldChecked}
                onChange={(e) => {
                  setFoldChecked(e.target.checked);
                  setExpandedFolds([]);
                }}
              >
                折叠已出
              </Checkbox>
            </Space>
          }
        >
          <div className="chips-head">
            <span className="chips-title">按单号</span>
            <span className="chips-sum">
              {pickedWaybills.length > 0
                ? `已选 ${pickedWaybills.length} 票 · ${shownBoxes} 件`
                : `未筛选 · 共 ${progress.length} 票 · ${totalQty} 件`}
            </span>
            <span style={{ flex: 1 }} />
            <button
              className="link-btn"
              onClick={() => setPickedWaybills(progress.map((p) => p.key))}
              disabled={progress.length === 0}
              title="把这一批单号全部选上，再点掉不要的"
            >
              全选
            </button>
            <button
              className="link-btn"
              onClick={() => setPickedWaybills([])}
              disabled={pickedWaybills.length === 0}
            >
              清空
            </button>
          </div>
          <div className="waybill-chips">
            {progress.map((p) => {
              const on = pickedWaybills.includes(p.key);
              return (
                <button
                  key={p.key}
                  className={`wchip${on ? " on" : ""}`}
                  onClick={(e) => pickWaybill(p.key, e.shiftKey)}
                  title={on ? "点击取消这一票；Shift 点另一票可连选" : "点击加入筛选；Shift 点另一票可连选"}
                  aria-pressed={on}
                >
                  <span className="wchip-k">
                    <span className="mono">{p.waybill}</span>
                    <i className="wchip-check" aria-hidden="true" />
                  </span>
                  <span className="wchip-v">{p.total} 件</span>
                  <span className="wchip-m">
                    已出 <b className="ok">{p.done}</b> / 未出{" "}
                    <b className={p.left ? "warn" : "ok"}>{p.left}</b>
                  </span>
                </button>
              );
            })}
          </div>

          {selected.length > 0 && (
            <div className="batch-bar">
              <span>
                已选 <b>{selected.length}</b> 行
              </span>
              <button className="btn sm" onClick={markSelectedDone}>
                标记已出
              </button>
              <button className="btn sm" onClick={copySelected}>
                复制
              </button>
              <button className="btn sm" onClick={() => setSelected([])}>
                清除
              </button>
              <span className="batch-hint">只作用于当前列表里的行</span>
            </div>
          )}

{boardMode ? (
            <Flex gap={12} style={{ flex: 1, minHeight: 0 }}>
              {[
                { title: "未出", list: pendingRows, tone: "#a67c52" },
                { title: "已出", list: doneRows, tone: "#4f7d63" },
              ].map((col) => (
                <div key={col.title} className="board-col">
                  <div className="board-col-head">
                    <span style={{ color: col.tone }}>{col.title}</span>
                    <span className="board-col-count">{col.list.length} 条</span>
                  </div>
                  <div className="board-col-body">
                    {col.list.map((r) => (
                      <div
                        key={r.__idx}
                        className={`board-card ${col.title === "已出" ? "done" : ""}${
                          rowBlocked(r) ? " blocked" : ""
                        }`}
                        title={rowBlocked(r) && col.title !== "已出" ? "没有入库号也没有单号，不能出货" : undefined}
                        onClick={() => {
                          if (col.title !== "已出" && rowBlocked(r)) return;
                          patch(r.__idx, { checked: col.title !== "已出" });
                        }}
                      >
                        <div className="board-card-cust">{r.customer || "—"}</div>
                        <div className="board-card-row">
                          <span className="board-card-name">{r.name}</span>
                          <span className="board-card-qty">{r.qty} 件</span>
                          <span
                            className="board-card-inbound"
                            style={{ fontFamily: "Consolas, monospace" }}
                          >
                            {r.inbound}
                          </span>
                        </div>
                      </div>
                    ))}
                    {col.list.length === 0 && <div className="board-empty">这里空着</div>}
                  </div>
                </div>
              ))}
            </Flex>
          ) : (
            <Table<TableRow>
              size="small"
              className="packing-table"
              rowKey={(r) => (isFold(r) ? r.__foldKey : String(r.__idx))}
              dataSource={data}
              columns={columns}
              pagination={{
                // 用 defaultPageSize 而不是 pageSize：传 pageSize 等于把它变成受控值，
                // 没有配套 onChange 时用户选 10/100 条会被立刻按回 50（2026-10-31 用户反馈「选不了」）
                defaultPageSize: 50,
                showSizeChanger: true,
                // 合计 / 已出 / 未出 / 差异都并进分页器左侧这一行，
                // 否则表格下面要再压一行统计，两行挤在一起（2026-10-31 用户反馈）
                showTotal: (total) =>
                  `共 ${total} 款 · 已出 ${doneQty} 件 · 未出 ${totalQty - doneQty} 件${
                    issueRows > 0 ? ` · 差异 ${issueRows} 行` : ""
                  }`,
                size: "small",
              }}
              // 表体高度交给 CSS flex 撑满（打包成 Tauri 后窗口高度不固定，
              // 写死 calc 常量会在工具栏换行时算错）。见 styles/packing.css 末尾「一屏布局」
              scroll={{ x: 1100, y: "100%" }}
              rowClassName={(r) => {
                if (isFold(r)) return "packing-fold-row";
                if (rowBlocked(r)) return "packing-row-blocked";
                return r.issues.some((c) => PROBLEM_ISSUES.has(c)) ? "packing-row-warn" : "";
              }}
              onRow={(r) => {
                if (isFold(r)) return {};
                const shown = r.issues.filter((c) => SHOWN_ISSUES.has(c));
                return shown.length
                  ? { title: shown.map((c) => ISSUE_TEXT[c] ?? c).join("、") }
                  : {};
              }}
            />
          )}
        </Card>
      )}

      {shipImagesOpen && (
        <ShipImagesModal
          paths={shipPaths}
          busy={shipBusy}
          result={shipResult}
          onPick={() => void pickShipImages()}
          onRemove={(p) => setShipPaths((prev) => prev.filter((x) => x !== p))}
          onRun={() => void runShipImages()}
          onClose={() => setShipImagesOpen(false)}
        />
      )}

      {mobileOpen && mobileInfo && (
        <MobileSheetModal
          info={mobileInfo}
          onCopy={copyText}
          onCopyUrl={copyMobileUrl}
          onReset={resetMobile}
          onAllowFirewall={allowFirewall}
          onClose={() => setMobileOpen(false)}
          onStop={stopMobile}
        />
      )}

      <Drawer
        title="Markdown 清单"
        placement="right"
        size={560}
        open={mdOpen}
        onClose={() => setMdOpen(false)}
        styles={{ body: { padding: 12 } }}
        extra={
          <Space>
            <Button size="small" icon={<CopyOutlined />} onClick={copyMd}>
              {copyHint ?? "复制"}
            </Button>
            <Button size="small" type="primary" icon={<SaveOutlined />} onClick={saveMd}>
              另存为 .md
            </Button>
          </Space>
        }
      >
        <Input.TextArea
          value={md}
          readOnly
          style={{
            height: "100%",
            fontFamily: "Consolas, monospace",
            fontSize: 12.5,
            resize: "none",
          }}
        />
      </Drawer>

      {cloudUpdate && (
        <div className="banner warn">
          <span>
            ☁️ 云表格有新数据{cloudUpdate ? `（版本 ${cloudUpdate}）` : ""}
            {cloudSheet ? `·「${cloudSheet.trim()}」` : ""}，要换最新吗？
          </span>
          <button
            className="btn sm"
            onClick={() => void sync(cloudSheet ?? undefined)}
            disabled={syncing}
          >
            {syncing ? "同步中…" : "换最新"}
          </button>
          <button className="btn sm banner-btn" onClick={() => setCloudUpdate(null)}>
            先这样
          </button>
        </div>
      )}
      {savedPath && <div className="banner ok">✅ 已保存：{savedPath}</div>}
      {error && <div className="banner err">❌ {error}</div>}

      {statsOpen && (
        <StatsModal
          stats={customerStats}
          total={statBoxes}
          filtered={stats.filtered}
          shownRows={shownOrder.length}
          onCopy={copyStats}
          onClose={() => setStatsOpen(false)}
        />
      )}

      {logText !== null && (
        <LogModal
          text={logText}
          busy={logBusy}
          onRefresh={openLog}
          onCopy={copyLog}
          onText={setLogText}
          onError={setError}
          onClose={() => setLogText(null)}
        />
      )}
    </div>
      </div>
    </div>
    </div>
  );
}























