import { useEffect, useMemo, useState } from "react";

/** 箱规计划：两个文本框粘贴参考/实际数据，自动解析并生成对照表（含模拟行）
 *  实重货 → 模拟实重（可选：是否模拟尺寸）；抛货 → 模拟抛货（可选：是否模拟重量） */

interface TextState {
  refText: string;
  newText: string;
}

const DEFAULT_REF = `杨总
唇线笔，144/件
52×52×37/6000=16.67467
实 19.1，共（2 件）`;

const DEFAULT_NEW = `杨总
唇线笔，160/件
45×45×45/6000=15.1875
实 19.5，共（2 件）`;

const STORE_KEY = "boxplan-text-v2";

/* ---------- 解析 ---------- */

interface ParsedBlock {
  client: string;
  product: string;
  pcs: number | null; // 数量/件
  l: number | null;
  w: number | null;
  h: number | null;
  divisor: number | null;
  vol: number | null; // 体积重（若原文给出）
  weight: number | null; // 实重
  count: number | null; // 共 N 件
}

const EMPTY_BLOCK: ParsedBlock = {
  client: "",
  product: "",
  pcs: null,
  l: null,
  w: null,
  h: null,
  divisor: null,
  vol: null,
  weight: null,
  count: null,
};

export function parseBlock(text: string): ParsedBlock {
  const res: ParsedBlock = { ...EMPTY_BLOCK };
  const lines = text
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter(Boolean);
  const matched = new Set<number>();

  lines.forEach((line, i) => {
    if (res.l === null) {
      const m = line.match(
        /(\d+(?:\.\d+)?)\s*[×xX*]\s*(\d+(?:\.\d+)?)\s*[×xX*]\s*(\d+(?:\.\d+)?)\s*\/\s*(\d+(?:\.\d+)?)\s*=\s*(\d+(?:\.\d+)?)/
      );
      if (m) {
        res.l = +m[1];
        res.w = +m[2];
        res.h = +m[3];
        res.divisor = +m[4];
        res.vol = +m[5];
        matched.add(i);
        return;
      }
    }
    if (res.weight === null) {
      const m = line.match(/实\s*(\d+(?:\.\d+)?)\s*[，,]?\s*共\s*[（(]?\s*(\d+)\s*件/);
      if (m) {
        res.weight = +m[1];
        res.count = +m[2];
        matched.add(i);
        return;
      }
    }
    if (res.pcs === null) {
      const m = line.match(/[，,]\s*(\d+(?:\.\d+)?)\s*\/\s*件/);
      if (m) {
        res.pcs = +m[1];
        res.product = line.slice(0, line.indexOf(m[0])).trim().replace(/[，,]\s*$/, "");
        matched.add(i);
        return;
      }
    }
  });

  const first = lines.find((_, i) => !matched.has(i));
  res.client = first ?? "";
  return res;
}

/* ---------- 整数尺寸拟合（移植自 box-dimension-fit 技能） ---------- */

interface BoxCand {
  l: number;
  w: number;
  h: number;
  result: number; // 体积重
  dev: number; // 比例偏差 0~1
}

function ratioNorm(t: [number, number, number]): [number, number, number] {
  const g = Math.cbrt(t[0] * t[1] * t[2]);
  return [t[0] / g, t[1] / g, t[2] / g].sort((a, b) => a - b) as [number, number, number];
}

export function solveBox(
  base: [number, number, number],
  divisor: number,
  target: number,
  count = 5
): BoxCand[] {
  const v0 = base[0] * base[1] * base[2];
  const targetV = target * divisor;
  const scale = Math.cbrt(targetV / v0);
  const ideal = base.map((b) => b * scale);

  let win = 0.6;
  let lo = 1;
  let hi = 2;
  for (;;) {
    lo = Math.max(1, Math.floor(Math.min(...ideal) * (1 - win)));
    hi = Math.ceil(Math.max(...ideal) * (1 + win));
    if (hi - lo + 1 <= 110 || win < 0.05) break;
    win *= 0.7;
  }

  const baseNorm = ratioNorm(base);
  const cands: [number, number, number, number, number][] = []; // product, dev, l, w, h
  for (let a = lo; a <= hi; a++) {
    for (let b = a; b <= hi; b++) {
      for (let c = b; c <= hi; c++) {
        const p = a * b * c;
        if (p <= targetV) continue;
        const n = ratioNorm([a, b, c]);
        const dev = Math.max(
          Math.abs(n[0] - baseNorm[0]) / baseNorm[0],
          Math.abs(n[1] - baseNorm[1]) / baseNorm[1],
          Math.abs(n[2] - baseNorm[2]) / baseNorm[2]
        );
        cands.push([p, dev, a, b, c]);
      }
    }
  }
  cands.sort((x, y) => x[0] - y[0]);

  // 帕累托前沿：乘积越大、比例偏差越小
  const front: typeof cands = [];
  let best = Infinity;
  for (const row of cands) {
    if (row[1] < best - 1e-12) {
      front.push(row);
      best = row[1];
    }
  }
  if (front.length === 0) return [];

  // 均匀取样
  const idxs = new Set<number>();
  if (count >= front.length) {
    for (let i = 0; i < front.length; i++) idxs.add(i);
  } else {
    for (let i = 0; i < count; i++) idxs.add(Math.round((i * (front.length - 1)) / (count - 1)));
  }
  return [...idxs]
    .sort((a, b) => a - b)
    .map((i) => {
      const [p, dev, l, w, h] = front[i];
      return { l, w, h, result: p / divisor, dev };
    });
}

/* ---------- 格式化 ---------- */

/** 保留最多 5 位小数并去掉尾随 0：11.39600 -> "11.396" */
const fmt = (x: number, nd = 5): string => {
  if (!Number.isFinite(x)) return "-";
  const s = x.toFixed(nd).replace(/\.?0+$/, "");
  return s === "" ? "0" : s;
};

const fmtWeight = (x: number): string => (Number.isFinite(x) ? String(Number(x.toFixed(2))) : "-");

/** 周长 =（最小值+中间值）×2+最大值 */
const girth = (a: number | null, b: number | null, c: number | null): number | null => {
  if (a == null || b == null || c == null) return null;
  const [x, y, z] = [a, b, c].sort((m, n) => m - n);
  return (x + y) * 2 + z;
};

/* ---------- 页面 ---------- */

export default function BoxPlanPage() {
  const [texts, setTexts] = useState<TextState>(() => {
    try {
      const raw = localStorage.getItem(STORE_KEY);
      if (raw) {
        const o = JSON.parse(raw);
        return { refText: o.refText ?? DEFAULT_REF, newText: o.newText ?? DEFAULT_NEW };
      }
    } catch {
      /* 忽略 */
    }
    return { refText: DEFAULT_REF, newText: DEFAULT_NEW };
  });
  const [extra, setExtra] = useState(false); // 实重货→是否模拟尺寸；抛货→是否模拟重量
  const [pickedIdx, setPickedIdx] = useState(0);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    try {
      localStorage.setItem(STORE_KEY, JSON.stringify(texts));
    } catch {
      /* 忽略 */
    }
  }, [texts]);

  const ref = useMemo(() => parseBlock(texts.refText), [texts.refText]);
  const cur = useMemo(() => parseBlock(texts.newText), [texts.newText]);

  const calc = useMemo(() => {
    const refVol =
      ref.vol ?? (ref.l && ref.w && ref.h && ref.divisor ? (ref.l * ref.w * ref.h) / ref.divisor : NaN);
    const newVol =
      cur.vol ?? (cur.l && cur.w && cur.h && cur.divisor ? (cur.l * cur.w * cur.h) / cur.divisor : NaN);
    const simWRaw =
      ref.pcs && cur.pcs && ref.weight != null ? (cur.pcs / ref.pcs) * ref.weight : NaN;
    // 物理限制：模拟出来的重量必须是 0.05 的倍数（向上进位）
    const simW = Number.isFinite(simWRaw)
      ? Math.round(Math.ceil(Math.round(simWRaw * 1000) / 1000 / 0.05) * 0.05 * 100) / 100
      : NaN;
    // 模拟抛货：模拟体积重 = 数量比 × 参考体积重
    const simVolTarget =
      ref.pcs && cur.pcs && Number.isFinite(refVol) ? (cur.pcs / ref.pcs) * refVol : NaN;
    // 货类按参考数据判定：参考实重 > 参考体积重 → 实重货
    const goodsType =
      ref.weight != null && Number.isFinite(refVol) ? (ref.weight > refVol ? "实重货" : "抛货") : null;
    const ok = Number.isFinite(refVol) && Number.isFinite(newVol) && Number.isFinite(simW);
    return { refVol, newVol, simW, simWRaw, simVolTarget, goodsType, ok, divisor: cur.divisor ?? 6000 };
  }, [ref, cur]);

  // 实重货 → 模拟实重；抛货 → 模拟抛货。货类变化时重置可选项
  const goodsType = calc.goodsType ?? "实重货";
  const fitDims = goodsType === "抛货" || extra; // 抛货总是凑尺寸；实重货勾选后才凑
  // 凑尺寸目标（两种货类一致）：模拟体积重 = 参考体积重 × 数量比，即 模拟体积重/参考体积重 = 实际数量/参考数量
  const fitTarget = calc.simVolTarget;
  useEffect(() => {
    setExtra(false);
  }, [goodsType]);

  const cands = useMemo(() => {
    if (!calc.ok || !fitDims) return [];
    if (ref.l == null || ref.w == null || ref.h == null) return [];
    return solveBox([ref.l, ref.w, ref.h], calc.divisor, fitTarget, 5);
  }, [calc, ref, fitDims, fitTarget]);

  useEffect(() => {
    if (cands.length === 0) {
      setPickedIdx(0);
      return;
    }
    const i = cands.findIndex((c) => c.dev <= 0.05);
    setPickedIdx(i >= 0 ? i : 0);
  }, [cands]);

  // 模拟行数据：未勾选模拟尺寸 → 尺寸与实际相同；否则取选中的整数尺寸方案
  const simCand = fitDims && cands.length > 0 ? cands[Math.min(pickedIdx, cands.length - 1)] : null;
  const simDims = simCand
    ? { l: String(simCand.l), w: String(simCand.w), h: String(simCand.h) }
    : { l: cur.l, w: cur.w, h: cur.h };
  const simVol = simCand ? simCand.result : calc.newVol;
  const simGirth = simCand ? girth(simCand.l, simCand.w, simCand.h) : girth(cur.l, cur.w, cur.h);
  // 模拟行实重：实重货=模拟实重；抛货默认实际实重，勾选模拟重量后=模拟实重
  const simWeight = goodsType === "实重货" || extra ? calc.simW : cur.weight;

  // ---- 输出文本 ----
  const output = useMemo(() => {
    if (!calc.ok) return "";
    const head = `${cur.client || ref.client || "客户"}\n${cur.product || "商品"}，${cur.pcs ?? "?"}/件`;
    const dims = simCand
      ? `${simCand.l}×${simCand.w}×${simCand.h}`
      : `${cur.l ?? "?"}×${cur.w ?? "?"}×${cur.h ?? "?"}`;
    const weightLine =
      goodsType === "实重货" || extra
        ? `模拟实 ${fmtWeight(calc.simW)}，共（${cur.count ?? "?"} 件）`
        : `实 ${fmtWeight(cur.weight ?? NaN)}，共（${cur.count ?? "?"} 件）`;
    return `${head}\n${dims}/${calc.divisor}=${fmt(simVol)}\n${weightLine}`;
  }, [cur, ref, calc, simCand, simVol, goodsType, extra]);

  const copyOut = async () => {
    try {
      await navigator.clipboard.writeText(output);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* 忽略 */
    }
  };

  const numOrDash = (v: number | null): string => (v == null ? "-" : fmt(v));

  // ---- 双向同步：表格改单元格 → 整块按标准格式重写回输入框 ----
  const blockToText = (b: ParsedBlock): string => {
    const vol =
      b.vol ??
      (b.l != null && b.w != null && b.h != null
        ? (b.l * b.w * b.h) / (b.divisor ?? 6000)
        : NaN);
    return [
      b.client || "客户",
      `${b.product || "商品"}，${b.pcs ?? "?"}/件`,
      `${b.l ?? "?"}×${b.w ?? "?"}×${b.h ?? "?"}/${b.divisor ?? 6000}=${
        Number.isFinite(vol) ? fmt(vol) : "?"
      }`,
      `实 ${b.weight ?? "?"}，共（${b.count ?? "?"} 件）`,
    ].join("\n");
  };

  const patchRef = (p: Partial<ParsedBlock>) => {
    const b: ParsedBlock = { ...ref, ...p };
    if (b.divisor == null) b.divisor = 6000;
    if (b.l != null && b.w != null && b.h != null) b.vol = (b.l * b.w * b.h) / b.divisor;
    setTexts((t) => ({ ...t, refText: blockToText(b) }));
  };

  const patchCur = (p: Partial<ParsedBlock>) => {
    const b: ParsedBlock = { ...cur, ...p };
    if (b.divisor == null) b.divisor = 6000;
    if (b.l != null && b.w != null && b.h != null) b.vol = (b.l * b.w * b.h) / b.divisor;
    setTexts((t) => ({ ...t, newText: blockToText(b) }));
  };

  // 表格数字单元格（带上下调节按钮；尺寸/数量步长 1，实重步长 0.05）
  const cellInput = (v: number | null, on: (n: number | null) => void, step = 1) => {
    const parse = (s: string): number | null => {
      const t = s.trim();
      if (t === "") return null;
      const n = Number(t);
      return Number.isFinite(n) ? n : null;
    };
    const bump = (d: number) => {
      const base = v ?? 0;
      on(Math.round((base + d) * 100) / 100);
    };
    return (
      <div className="stepper">
        <input
          value={v ?? ""}
          onChange={(e) => on(parse(e.target.value))}
        />
        <div className="stepper-btns">
          <button type="button" title={`+${step}`} onClick={() => bump(step)}>
            ▲
          </button>
          <button type="button" title={`-${step}`} onClick={() => bump(-step)}>
            ▼
          </button>
        </div>
      </div>
    );
  };

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1 className="page-title">箱规计划</h1>
          <p className="page-sub">粘贴参考与实际数据，自动算周长、体积重和模拟箱规</p>
        </div>
      </header>

      {/* 数据录入：两个粘贴框 */}
      <div className="form-grid boxplan-inputs">
        <div className="field grow">
          <label>参考数据（粘贴原始格式，自动解析）</label>
          <textarea
            className="boxplan-textarea mono"
            value={texts.refText}
            onChange={(e) => setTexts((t) => ({ ...t, refText: e.target.value }))}
            spellCheck={false}
          />
        </div>
        <div className="field grow">
          <label>实际数据（粘贴原始格式，自动解析）</label>
          <textarea
            className="boxplan-textarea mono"
            value={texts.newText}
            onChange={(e) => setTexts((t) => ({ ...t, newText: e.target.value }))}
            spellCheck={false}
          />
        </div>
      </div>

      {/* 对照表（含模拟行） */}
      <div className="form-grid">
        <div className="boxplan-title">对照表</div>
        <div className="boxplan-table-wrap">
        <table className="hist-table boxplan-input-table">
          <thead>
            <tr>
              <th>数据</th>
              <th>数量/件</th>
              <th>长 cm</th>
              <th>宽 cm</th>
              <th>高 cm</th>
              <th>周长 cm</th>
              <th>体积重</th>
              <th>实重 kg</th>
              <th>单件重量</th>
              <th>单件体积重</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td className="boxplan-tag">参考</td>
              <td>{cellInput(ref.pcs, (n) => patchRef({ pcs: n }))}</td>
              <td>{cellInput(ref.l, (n) => patchRef({ l: n }))}</td>
              <td>{cellInput(ref.w, (n) => patchRef({ w: n }))}</td>
              <td>{cellInput(ref.h, (n) => patchRef({ h: n }))}</td>
              <td className="mono">{fmt(girth(ref.l, ref.w, ref.h) ?? NaN)}</td>
              <td className="mono">{fmt(calc.refVol)}</td>
              <td>{cellInput(ref.weight, (n) => patchRef({ weight: n }), 0.05)}</td>
              <td className="mono">
                {fmt(ref.weight != null && ref.pcs ? ref.weight / ref.pcs : NaN)}
              </td>
              <td className="mono">
                {fmt(ref.pcs && Number.isFinite(calc.refVol) ? calc.refVol / ref.pcs : NaN)}
              </td>
            </tr>
            <tr>
              <td className="boxplan-tag">实际</td>
              <td>{cellInput(cur.pcs, (n) => patchCur({ pcs: n }))}</td>
              <td>{cellInput(cur.l, (n) => patchCur({ l: n }))}</td>
              <td>{cellInput(cur.w, (n) => patchCur({ w: n }))}</td>
              <td>{cellInput(cur.h, (n) => patchCur({ h: n }))}</td>
              <td className="mono">{fmt(girth(cur.l, cur.w, cur.h) ?? NaN)}</td>
              <td className="mono">{fmt(calc.newVol)}</td>
              <td>{cellInput(cur.weight, (n) => patchCur({ weight: n }), 0.05)}</td>
              <td className="mono">
                {fmt(cur.weight != null && cur.pcs ? cur.weight / cur.pcs : NaN)}
              </td>
              <td className="mono">
                {fmt(cur.pcs && Number.isFinite(calc.newVol) ? calc.newVol / cur.pcs : NaN)}
              </td>
            </tr>
            <tr className="boxplan-simrow">
              <td className="boxplan-tag">
                模拟
                <br />
                <span className="boxplan-mode-tag">
                  {goodsType === "抛货" ? "抛货" : "实重"}
                </span>
              </td>
              <td className="mono">{numOrDash(cur.pcs)}</td>
              <td className="mono">{simDims.l ?? "-"}</td>
              <td className="mono">{simDims.w ?? "-"}</td>
              <td className="mono">{simDims.h ?? "-"}</td>
              <td className="mono">{fmt(simGirth ?? NaN)}</td>
              <td className="mono">{fmt(simVol)}</td>
              <td className="mono">
                <b>{fmtWeight(simWeight ?? NaN)}</b>
              </td>
              <td className="mono">
                {fmt(simWeight != null && cur.pcs ? simWeight / cur.pcs : NaN)}
              </td>
              <td className="mono">
                {fmt(cur.pcs && Number.isFinite(simVol) ? simVol / cur.pcs : NaN)}
              </td>
            </tr>
          </tbody>
        </table>
        </div>
      </div>

      {/* 模拟 */}
      <div className="form-grid boxplan-sim">
        <div className="boxplan-title">模拟</div>
        <div className="muted boxplan-inline">
          判定（按参考数据）：<b>{calc.goodsType ?? "-"}</b>
          {goodsType === "抛货" ? " → 按「模拟抛货」" : " → 按「模拟实重」"}
        </div>
        <div className="muted boxplan-inline mono">
          {goodsType !== "抛货" && (
            <>
              模拟实重 = {numOrDash(cur.pcs)}/{numOrDash(ref.pcs)} × {numOrDash(ref.weight)} ={" "}
              {fmt(calc.simWRaw)} → 按 0.05 进位 = <b>{fmtWeight(calc.simW)}</b>
              {fitDims && <br />}
            </>
          )}
          {fitDims && (
            <>
              模拟体积重 = {numOrDash(cur.pcs)}/{numOrDash(ref.pcs)} × {fmt(calc.refVol)} ={" "}
              <b>{fmt(calc.simVolTarget)}</b>（模拟体积重/参考体积重 = 实际数量/参考数量）
            </>
          )}
        </div>
        <label className="setting-toggle">
          <input type="checkbox" checked={extra} onChange={(e) => setExtra(e.target.checked)} />
          {goodsType === "抛货"
            ? "是否模拟重量（实重也按数量比放大：模拟实重 = 数量比 × 参考实重）"
            : "是否模拟尺寸（模拟体积重 = 参考体积重 × 数量比，按参考箱比例凑整）"}
        </label>

        {fitDims && cands.length > 0 && (
          <table className="hist-table boxplan-table">
            <thead>
              <tr>
                <th>选择</th>
                <th>长×宽×高</th>
                <th>体积重</th>
                <th>模拟体积重</th>
                <th>差值</th>
                <th>比例偏差</th>
              </tr>
            </thead>
            <tbody>
              {cands.map((c, i) => (
                <tr
                  key={`${c.l}-${c.w}-${c.h}`}
                  className={i === pickedIdx ? "boxplan-picked" : ""}
                  onClick={() => setPickedIdx(i)}
                  style={{ cursor: "pointer" }}
                >
                  <td>
                    <input type="radio" checked={i === pickedIdx} onChange={() => setPickedIdx(i)} />
                  </td>
                  <td>
                    {c.l}×{c.w}×{c.h}
                    {i === cands.findIndex((x) => x.dev <= 0.05) && (
                      <span className="boxplan-rec">推荐</span>
                    )}
                  </td>
                  <td>{fmt(c.result)}</td>
                  <td>{fmt(fitTarget)}</td>
                  <td>
                    {c.result - fitTarget >= 0 ? "+" : ""}
                    {fmt(c.result - fitTarget)}
                  </td>
                  <td>{(c.dev * 100).toFixed(1)}%</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {fitDims && cands.length === 0 && calc.ok && (
          <div className="banner err">没有找到满足条件的整数组合，请检查参考箱尺寸。</div>
        )}

        {output && (
          <div className="boxplan-out-wrap">
            <div className="preview-title">输出（按箱规格式，可直接复制）</div>
            <pre className="boxplan-out mono">{output}</pre>
            <div className="toolbar">
              <button className="btn primary sm" onClick={copyOut}>
                {copied ? "✓ 已复制" : "复制"}
              </button>
              <span className="muted">
                {goodsType === "抛货"
                  ? extra
                    ? "模拟抛货：凑整尺寸盖住模拟体积重，实重已按数量比放大"
                    : "模拟抛货：凑整尺寸盖住模拟体积重，实重不变"
                  : extra
                    ? "模拟实重+模拟尺寸：模拟体积重/参考体积重 = 实际数量/参考数量"
                    : "模拟实重：尺寸未改动，实重替换为模拟值"}
              </span>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
