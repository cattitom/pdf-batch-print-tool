import { useEffect, useMemo, useRef, useState } from "react";
import { confirm as confirmDialog, open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
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
  Space,
  Table,
  Tooltip,
  Typography,
} from "antd";
import {
  AppstoreOutlined,
  CloseOutlined,
  CopyOutlined,
  DeleteOutlined,
  FileTextOutlined,
  FilterFilled,
  FolderOpenOutlined,
  PushpinOutlined,
  ReloadOutlined,
  SaveOutlined,
  SearchOutlined,
  UnorderedListOutlined,
} from "@ant-design/icons";
import { api, errorHint, errorText } from "../api";
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
 * 录货清单：粘贴 Excel 表格 / 拖 xlsx·csv / 拖截图，自动拆成「目的地 + 单号」分组，
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

/** 只有这几种提示值得占界面：目的地已经不看了，dest_inherit 之类的就不显示。 */
const SHOWN_ISSUES = new Set(["name", "qty", "inbound", "inbound_inherit", "inbound_sci"]);

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

/** 一行货物的稳定指纹：同一票、同入库号、同商品名，重新解析后还能认回来。 */
const rowKey = (r: PackingItemDto) =>
  `${r.waybill.trim()}|${r.inbound.trim()}|${r.name.trim()}`;

/** 按单号分块（录货是按票收的），块头带箱数和款数。 */
function toMarkdown(rows: PackingItemDto[]): string {
  const groups = new Map<string, PackingItemDto[]>();
  for (const row of rows) {
    const key = row.waybill.trim() || "未识别单号";
    const list = groups.get(key);
    if (list) list.push(row);
    else groups.set(key, [row]);
  }
  if (groups.size === 0) return "";

  const out: string[] = ["# 录货清单", ""];
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

/** 日志行结构化：`2026-10-28 08:24:26.745 INFO  desktop_lib: 消息` */
interface LogRow {
  time: string;
  level: string;
  target: string;
  message: string;
}

// target 里可能带 `::`（如 tao::platform_impl::…），所以取第一个「: 」作为分隔
const LOG_LINE =
  /^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3})\s+(\w+)\s+(.*?):\s([\s\S]*)$/;

function parseLog(text: string): LogRow[] {
  const rows: LogRow[] = [];
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const m = line.match(LOG_LINE);
    if (m) {
      rows.push({ time: m[1], level: m[2], target: m[3], message: m[4] });
    } else if (rows.length > 0) {
      // 续行（消息里带换行时）挂回上一条
      rows[rows.length - 1].message += `\n${line}`;
    }
  }
  return rows;
}

/** 包一层「用户改过的行」：编辑后重新算 md，不回后端。 */
export default function PackingPage({
  pendingDrop,
}: {
  pendingDrop?: { paths: string[]; nonce: number } | null;
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
  const [logView, setLogView] = useState<"structured" | "raw">("structured");
  const [logQuery, setLogQuery] = useState("");
  const [logLevel, setLogLevel] = useState("全部");


  /** 已勾选记忆：指纹 → 是否已录。启动时从数据库读一次，勾选时逐条写回。 */
  const checkedRef = useRef<Record<string, boolean>>({});
  /** 启动时是否自动打开上次的文件（默认开） */
  const [autoOpen, setAutoOpen] = useState(true);
  /** 右侧 Markdown 清单抽屉 */
  const [mdOpen, setMdOpen] = useState(false);
  /** 看板模式：按「待录 / 已录」两列铺卡片，点卡片切换 */
  const [boardMode, setBoardMode] = useState(false);
  /** 顶部搜索词（模糊匹配商品/客户/入库号/单号） */
  const [q, setQ] = useState("");
  /** 钉住的行指纹（右侧边栏） */
  const [pinned, setPinned] = useState<string[]>([]);
  /** 钉住条目里勾选的那些（用于批量复制） */
  const [pinSelected, setPinSelected] = useState<string[]>([]);
  /** 钉住的目标行（按指纹在 rows 里查） */
  const pinnedRows = useMemo(() => {
    const byKey = new Map<string, PackingRow>();
    rows.forEach((r, i) => byKey.set(rowKey(r), { ...r, __idx: i }));
    return pinned.map((k) => byKey.get(k)).filter((r): r is PackingRow => !!r);
  }, [pinned, rows]);

  const md = useMemo(() => toMarkdown(rows), [rows]);
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
  const visibleOrder = shownOrder.filter((i) => {
    const r = rows[i];
    if (kw) {
      const matched =
        r.name.toLowerCase().includes(kw) ||
        r.customer.toLowerCase().includes(kw) ||
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
      hit("inbound", r.inbound || "(空)") &&
      hit("waybill", r.waybill || "(空)")
    );
  });
  const uncheckedInView = shownOrder.filter((i) => !rows[i].checked).length;
  const onlyTodo = !!checkedFilter && checkedFilter.includes("todo") && !checkedFilter.includes("done");

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

  /** 看板两列：待录 / 已录（跟当前单号筛选一致） */
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
  const inboundOptions = optionsOf((r) => r.inbound);
  const waybillOptions = optionsOf((r) => r.waybill);


  const columns = [
    {
      title: filterTitle("客户名称", "customer", custOptions),
      dataIndex: "customer",
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
      render: (v: string, r: TableRow) => {
        if (isFold(r)) {
          return (
            <Button
              type="link"
              size="small"
              className="fold-btn"
              onClick={() => setExpandedFolds((prev) => [...prev, r.__foldKey])}
            >
              ⋯ 已录 {r.__foldCount} 条，点击展开 ⋯
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
      width: 90,
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
      title: filterTitle("已录", "checked", [
          { text: "待录", value: "todo" },
          { text: "已录", value: "done" },
        ]),
      dataIndex: "checked",
      render: (v: boolean, r: TableRow) =>
        isFold(r) ? null : (
          <Checkbox checked={v} onChange={(e) => patch(r.__idx, { checked: e.target.checked })} />
        ),
    },
    {
      title: filterTitle("入库号", "inbound", inboundOptions),
      dataIndex: "inbound",
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
      render: (v: string, r: TableRow) =>
        isFold(r) ? null : (
          <Input
            variant="borderless"
            value={v}
            style={monoStyle}
            onChange={(e) => patch(r.__idx, { waybill: e.target.value })}
          />
        ),
    },
    {
      title: "",
      key: "ops",
      width: 84,
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
              title={isPinned ? "取消钉住" : "钉到侧边栏"}
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

  const logRows = useMemo(() => parseLog(logText ?? ""), [logText]);
  const filteredLog = useMemo(() => {
    const q = logQuery.trim().toLowerCase();
    return logRows.filter((r) => {
      if (logLevel !== "全部" && r.level !== logLevel) return false;
      if (!q) return true;
      return (
        r.message.toLowerCase().includes(q) ||
        r.target.toLowerCase().includes(q) ||
        r.level.toLowerCase().includes(q)
      );
    });
  }, [logRows, logQuery, logLevel]);

  const applySheet = (s: PackingSheetDto) => {
    // 重新解析同一份表时，把之前勾过的「已录」按指纹认回来
    const items = s.items.map((it) => {
      const saved = checkedRef.current[rowKey(it)];
      return saved === undefined ? it : { ...it, checked: saved };
    });
    setSheet(s);
    setRows(items);
    // 上次筛的单号如果这份表里没有，自动去掉，免得一打开就是空表
    const present = new Set(items.map((it) => it.waybill.trim() || "__none__"));
    setPickedWaybills((prev) => {
      const kept = prev.filter((k) => present.has(k));
      return kept.length === prev.length ? prev : kept;
    });
  };

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
    const path = pendingDrop?.paths?.[0];
    if (!path) return;
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
      }
    }
    setSavedPath(null);
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

  return (
    <div className="packing-layout">
      <aside className="pin-sidebar">
        <div className="pin-sidebar-head">
          <PushpinOutlined />
          <span>钉住</span>
          {pinnedRows.length > 0 && <span className="pin-sidebar-count">{pinnedRows.length}</span>}
        </div>
        <div className="pin-sidebar-body">
          {pinnedRows.length === 0 ? (
            <div className="pin-empty">表格里点图钉<br />常看的货会留在这儿</div>
          ) : (
            pinnedRows.map((r) => (
              <div key={rowKey(r)} className="pin-item">
                <div className="pin-cust">
                  <Checkbox
                    checked={pinSelected.includes(rowKey(r))}
                    onChange={(e) =>
                      setPinSelected((prev) =>
                        e.target.checked
                          ? [...prev, rowKey(r)]
                          : prev.filter((k) => k !== rowKey(r))
                      )
                    }
                  />
                  <span className="pin-cust-name">{r.customer || "—"}</span>
                  <Button
                    type="text"
                    size="small"
                    icon={<CopyOutlined />}
                    title="复制这一条"
                    onClick={() => copyPinned([rowKey(r)])}
                  />
                  <Button
                    type="text"
                    size="small"
                    icon={<CloseOutlined />}
                    title="取消钉住"
                    onClick={() => setPinned((prev) => prev.filter((k) => k !== rowKey(r)))}
                  />
                </div>
                <div className="pin-row">
                  <span className="pin-name">{r.name}</span>
                  <span className="pin-qty">{r.qty} 件</span>
                </div>
                <div className="pin-code">
                  {r.inbound} · {r.waybill}
                </div>
                <Checkbox
                  checked={r.checked}
                  onChange={(e) => patch(r.__idx, { checked: e.target.checked })}
                  style={{ marginTop: 4, fontSize: 12 }}
                >
                  已录
                </Checkbox>
              </div>
            ))
          )}
        </div>
        {pinnedRows.length > 0 && (
          <div className="pin-sidebar-foot">
            <Checkbox
              checked={pinSelected.length === pinnedRows.length && pinnedRows.length > 0}
              indeterminate={pinSelected.length > 0 && pinSelected.length < pinnedRows.length}
              onChange={(e) =>
                setPinSelected(e.target.checked ? pinnedRows.map((r) => rowKey(r)) : [])
              }
            >
              全选
            </Checkbox>
            <Button
              size="small"
              type="primary"
              icon={<CopyOutlined />}
              disabled={pinSelected.length === 0}
              onClick={() => copyPinned(pinSelected)}
            >
              复制{pinSelected.length ? ` ${pinSelected.length}` : ""}
            </Button>
          </div>
        )}
      </aside>
      <div className="packing-main">
    <Flex vertical gap={16} className="page">
      <Flex align="center" gap={12} wrap>
        <Text type="secondary">
          选择或拖入 xlsx、csv、截图{busy ? " · 解析中…" : ""}
        </Text>
        <div style={{ flex: 1 }} />
        <Button type="primary" icon={<FolderOpenOutlined />} onClick={pickFile} loading={busy}>
          选择文件
        </Button>
        <Input
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder="搜索商品 / 客户 / 入库号 / 单号"
          prefix={<SearchOutlined style={{ color: "#b9b3a8" }} />}
          allowClear
          style={{ width: 260 }}
        />
        <Button icon={<ReloadOutlined />} onClick={refreshLast} disabled={busy}>
          刷新
        </Button>
        <Button onClick={clearAll} disabled={rows.length === 0}>
          清空
        </Button>
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
        <Button icon={<FileTextOutlined />} onClick={openLog} loading={logBusy}>
          日志
        </Button>
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
      </Flex>

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
                  {issueCount} 行待确认
                </Text>
              )}
            </Space>
          }
          extra={
            <Space size={16}>
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
              <Checkbox
                checked={foldChecked}
                onChange={(e) => {
                  setFoldChecked(e.target.checked);
                  setExpandedFolds([]);
                }}
              >
                折叠已录
              </Checkbox>
            </Space>
          }
        >
          <Flex gap={10} wrap style={{ marginBottom: 14 }}>
            <Card
              size="small"
              onClick={() => setPickedWaybills([])}
              style={{
                width: 160,
                cursor: "pointer",
                borderColor: pickedWaybills.length === 0 ? "#5c7fa3" : undefined,
                background: pickedWaybills.length === 0 ? "#eef2f6" : undefined,
              }}
              styles={{ body: { padding: "8px 10px" } }}
            >
              <div style={{ fontSize: 11, color: "#8d897f" }}>全部</div>
              <div style={{ fontSize: 16, fontWeight: 600, margin: "2px 0" }}>
                总数 {totalQty} 件
              </div>
              <div style={{ fontSize: 11.5, color: "#767268" }}>
                已录 <b style={{ color: "#4f7d63" }}>{doneQty}</b> / 待录{" "}
                <b style={{ color: totalQty - doneQty ? "#a67c52" : "#4f7d63" }}>
                  {totalQty - doneQty}
                </b>
              </div>
            </Card>
            {progress.map((p) => {
              const on = pickedWaybills.includes(p.key);
              return (
                <Card
                  key={p.key}
                  size="small"
                  onClick={() => toggleWaybill(p.key)}
                  style={{
                    width: 160,
                    cursor: "pointer",
                    borderColor: on ? "#5c7fa3" : undefined,
                    background: on ? "#eef2f6" : undefined,
                  }}
                  styles={{ body: { padding: "8px 10px" } }}
                >
                  <div
                    style={{
                      fontSize: 11,
                      color: "#8d897f",
                      fontFamily: "Consolas, monospace",
                    }}
                  >
                    {p.waybill}
                  </div>
                  <div style={{ fontSize: 16, fontWeight: 600, margin: "2px 0" }}>
                    总数 {p.total} 件
                  </div>
                  <div style={{ fontSize: 11.5, color: "#767268" }}>
                    已录 <b style={{ color: "#4f7d63" }}>{p.done}</b> / 待录{" "}
                    <b style={{ color: p.left ? "#a67c52" : "#4f7d63" }}>{p.left}</b>
                  </div>
                </Card>
              );
            })}
          </Flex>

{boardMode ? (
            <Flex gap={12} style={{ height: 560 }}>
              {[
                { title: "待录", list: pendingRows, tone: "#a67c52" },
                { title: "已录", list: doneRows, tone: "#4f7d63" },
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
                        className={`board-card ${col.title === "已录" ? "done" : ""}`}
                        onClick={() => patch(r.__idx, { checked: col.title !== "已录" })}
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
              rowKey={(r) => (isFold(r) ? r.__foldKey : String(r.__idx))}
              dataSource={data}
              columns={columns}
              pagination={{
                pageSize: 50,
                showSizeChanger: true,
                showTotal: (t) => `共 ${t} 款`,
                size: "small",
              }}
              scroll={{ y: 520 }}
              rowClassName={(r) => {
                if (isFold(r)) return "packing-fold-row";
                return r.issues.some((c) => SHOWN_ISSUES.has(c)) ? "packing-row-warn" : "";
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
          <Flex gap={8} style={{ marginTop: 12 }} align="center">
            <div style={{ flex: 1 }} />
            <Text type="secondary">
              已勾选 {doneQty} 件 · 待录 {totalQty - doneQty} 件
            </Text>
          </Flex>
        </Card>
      )}

      <Drawer
        title="Markdown 清单"
        placement="right"
        width={560}
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

      {savedPath && <div className="banner ok">✅ 已保存：{savedPath}</div>}
      {error && <div className="banner err">❌ {error}</div>}

      {logText !== null && (
        <div className="modal-mask" onClick={() => setLogText(null)}>
          <div className="modal packing-log-modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-head">
              <span>
                📄 日志 <span className="muted">· {logRows.length} 条</span>
              </span>
              <span className="spacer" />
              <div className="log-view-toggle">
                <button
                  className={`btn sm ${logView === "structured" ? "primary" : ""}`}
                  onClick={() => setLogView("structured")}
                >
                  结构化
                </button>
                <button
                  className={`btn sm ${logView === "raw" ? "primary" : ""}`}
                  onClick={() => setLogView("raw")}
                >
                  原始
                </button>
              </div>
              <button className="btn sm" onClick={openLog} disabled={logBusy}>
                刷新
              </button>
              <button
                className="btn sm danger"
                onClick={async () => {
                  const ok = await confirmDialog("清空当前日志？清空后无法恢复。", {
                    title: "清空日志",
                    kind: "warning",
                  });
                  if (!ok) return;
                  try {
                    await api.clearLog();
                    setLogText(await api.readLogTail(500));
                  } catch (e) {
                    setError(errorText(e) + errorHint(e));
                  }
                }}
              >
                清空
              </button>
              <button className="btn sm" onClick={copyLog}>
                复制
              </button>
              <button
                className="btn sm"
                onClick={async () => {
                  try {
                    await revealItemInDir(await api.logDir());
                  } catch (e) {
                    setError(errorText(e));
                  }
                }}
              >
                打开目录
              </button>
              <button className="btn sm" onClick={() => setLogText(null)}>
                关闭
              </button>
            </div>

            {logView === "structured" && (
              <div className="log-filterbar">
                <input
                  className="log-search"
                  placeholder="搜索关键词（如「网格」「OCR」「ERROR」）"
                  value={logQuery}
                  onChange={(e) => setLogQuery(e.target.value)}
                />
                {["全部", "INFO", "WARN", "ERROR", "DEBUG"].map((lv) => (
                  <button
                    key={lv}
                    className={`btn sm ${logLevel === lv ? "primary" : ""}`}
                    onClick={() => setLogLevel(lv)}
                  >
                    {lv}
                  </button>
                ))}
                <span className="spacer" />
                <span className="muted">
                  显示 {filteredLog.length} / {logRows.length} 条
                </span>
              </div>
            )}

            <div className="modal-body">
              {logView === "raw" ? (
                <pre className="packing-log">{logText || "（日志为空）"}</pre>
              ) : filteredLog.length === 0 ? (
                <div className="packing-log-empty">没有匹配的日志</div>
              ) : (
                <div className="log-scroll">
                  <table className="hist-table log-table">
                    <thead>
                      <tr>
                        <th style={{ width: 92 }}>时间</th>
                        <th style={{ width: 62 }}>级别</th>
                        <th style={{ width: 140 }}>模块</th>
                        <th>内容</th>
                      </tr>
                    </thead>
                    <tbody>
                      {filteredLog.map((r, i) => (
                        <tr key={i}>
                          <td className="t" title={r.time}>
                            {r.time.slice(11)}
                          </td>
                          <td>
                            <span className={`log-lvl ${r.level}`}>{r.level}</span>
                          </td>
                          <td className="muted">{r.target}</td>
                          <td className="m">{r.message}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </div>
          </div>
        </div>
      )}
    </Flex>
      </div>
    </div>
  );
}
