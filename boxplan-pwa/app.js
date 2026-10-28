/* 箱规计划 PWA —— 纯前端实现，数据存 localStorage */
"use strict";

/* ---------- 常量与默认值 ---------- */
const STORE_KEY = "boxplan-pwa-v1";
const DEFAULT_REF = "杨总\n唇线笔，144/件\n52×52×37/6000=16.67467\n实 19.1，共（2 件）";
const DEFAULT_NEW = "杨总\n唇线笔，160/件\n45×45×45/6000=15.1875\n实 19.5，共（2 件）";

/* ---------- 状态 ---------- */
let S = { refText: DEFAULT_REF, newText: DEFAULT_NEW, extra: false, pickedIdx: 0 };
try {
  const raw = localStorage.getItem(STORE_KEY);
  if (raw) {
    const o = JSON.parse(raw);
    S = {
      refText: typeof o.refText === "string" ? o.refText : DEFAULT_REF,
      newText: typeof o.newText === "string" ? o.newText : DEFAULT_NEW,
      extra: !!o.extra,
      pickedIdx: o.pickedIdx | 0,
    };
  }
} catch (e) {
  /* 忽略 */
}
function save() {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(S));
  } catch (e) {
    /* 忽略 */
  }
}

/* ---------- 工具 ---------- */
const fmt = (x, nd = 5) => {
  if (!Number.isFinite(x)) return "-";
  const s = x.toFixed(nd).replace(/\.?0+$/, "");
  return s === "" ? "0" : s;
};
const fmtWeight = (x) => (Number.isFinite(x) ? String(Number(x.toFixed(2))) : "-");
/** 周长 =（最小值+中间值）×2+最大值 */
const girth = (a, b, c) => {
  if (a == null || b == null || c == null) return null;
  const s = [a, b, c].sort((m, n) => m - n);
  return (s[0] + s[1]) * 2 + s[2];
};
/** 模拟重量必须是 0.05 的倍数（向上进位，物理限制） */
const snap005 = (x) => Math.round(Math.ceil(Math.round(x * 1000) / 1000 / 0.05) * 0.05 * 100) / 100;
const numOrDash = (v) => (v == null ? "-" : fmt(v));

/* ---------- 解析 ---------- */
function parseBlock(text) {
  const res = {
    client: "", product: "", pcs: null, l: null, w: null, h: null,
    divisor: null, vol: null, weight: null, count: null,
  };
  const lines = String(text).split(/\r?\n/).map((s) => s.trim()).filter(Boolean);
  const matched = new Set();
  lines.forEach((line, i) => {
    if (res.l === null) {
      const m = line.match(
        /(\d+(?:\.\d+)?)\s*[×xX*]\s*(\d+(?:\.\d+)?)\s*[×xX*]\s*(\d+(?:\.\d+)?)\s*\/\s*(\d+(?:\.\d+)?)\s*=\s*(\d+(?:\.\d+)?)/
      );
      if (m) {
        res.l = +m[1]; res.w = +m[2]; res.h = +m[3]; res.divisor = +m[4]; res.vol = +m[5];
        matched.add(i);
        return;
      }
    }
    if (res.weight === null) {
      const m = line.match(/实\s*(\d+(?:\.\d+)?)\s*[，,]?\s*共\s*[（(]?\s*(\d+)\s*件/);
      if (m) {
        res.weight = +m[1]; res.count = +m[2];
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

/** 解析块 → 标准格式文本（表格编辑后反向同步用） */
function blockToText(b) {
  const vol =
    b.vol ??
    (b.l != null && b.w != null && b.h != null ? (b.l * b.w * b.h) / (b.divisor ?? 6000) : NaN);
  return [
    b.client || "客户",
    `${b.product || "商品"}，${b.pcs ?? "?"}/件`,
    `${b.l ?? "?"}×${b.w ?? "?"}×${b.h ?? "?"}/${b.divisor ?? 6000}=${
      Number.isFinite(vol) ? fmt(vol) : "?"
    }`,
    `实 ${b.weight ?? "?"}，共（${b.count ?? "?"} 件）`,
  ].join("\n");
}

/* ---------- 整数尺寸拟合（移植自 box-dimension-fit） ---------- */
function ratioNorm(t) {
  const g = Math.cbrt(t[0] * t[1] * t[2]);
  return [t[0] / g, t[1] / g, t[2] / g].sort((a, b) => a - b);
}
function solveBox(base, divisor, target, count = 5) {
  const v0 = base[0] * base[1] * base[2];
  const targetV = target * divisor;
  const scale = Math.cbrt(targetV / v0);
  const ideal = base.map((b) => b * scale);
  let win = 0.6, lo = 1, hi = 2;
  for (;;) {
    lo = Math.max(1, Math.floor(Math.min(...ideal) * (1 - win)));
    hi = Math.ceil(Math.max(...ideal) * (1 + win));
    if (hi - lo + 1 <= 110 || win < 0.05) break;
    win *= 0.7;
  }
  const baseNorm = ratioNorm(base);
  const cands = [];
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
  const front = [];
  let best = Infinity;
  for (const row of cands) {
    if (row[1] < best - 1e-12) {
      front.push(row);
      best = row[1];
    }
  }
  if (front.length === 0) return [];
  const idxs = new Set();
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

/* ---------- 计算 ---------- */
function compute() {
  const ref = parseBlock(S.refText);
  const cur = parseBlock(S.newText);
  const refVol =
    ref.vol ?? (ref.l != null && ref.w != null && ref.h != null && ref.divisor
      ? (ref.l * ref.w * ref.h) / ref.divisor : NaN);
  const newVol =
    cur.vol ?? (cur.l != null && cur.w != null && cur.h != null && cur.divisor
      ? (cur.l * cur.w * cur.h) / cur.divisor : NaN);
  const simWRaw = ref.pcs && cur.pcs && ref.weight != null ? (cur.pcs / ref.pcs) * ref.weight : NaN;
  const simW = Number.isFinite(simWRaw) ? snap005(simWRaw) : NaN;
  const simVolTarget = ref.pcs && cur.pcs && Number.isFinite(refVol) ? (cur.pcs / ref.pcs) * refVol : NaN;
  const goodsType =
    ref.weight != null && Number.isFinite(refVol) ? (ref.weight > refVol ? "实重货" : "抛货") : null;
  const ok = Number.isFinite(refVol) && Number.isFinite(newVol) && Number.isFinite(simW);
  const divisor = cur.divisor ?? 6000;
  // 货类变化时重置可选项
  if (goodsType !== compute.lastGoods) {
    S.extra = false;
    compute.lastGoods = goodsType;
    save();
  }
  const fitDims = goodsType === "抛货" || S.extra;
  const fitTarget = simVolTarget;
  const cands = ok && fitDims && ref.l != null && ref.w != null && ref.h != null
    ? solveBox([ref.l, ref.w, ref.h], divisor, fitTarget, 5)
    : [];
  if (S.pickedIdx >= cands.length) S.pickedIdx = 0;
  const recIdx = cands.findIndex((c) => c.dev <= 0.05);
  const picked = fitDims && cands.length ? cands[Math.min(S.pickedIdx, cands.length - 1)] : null;
  const simDims = picked
    ? { l: picked.l, w: picked.w, h: picked.h }
    : { l: cur.l, w: cur.w, h: cur.h };
  const simVol = picked ? picked.result : newVol;
  const simWeight = goodsType === "实重货" || S.extra ? simW : cur.weight;
  const simGirth = picked ? girth(picked.l, picked.w, picked.h) : girth(cur.l, cur.w, cur.h);
  let output = "";
  if (ok) {
    const head = `${cur.client || ref.client || "客户"}\n${cur.product || "商品"}，${cur.pcs ?? "?"}/件`;
    const dims = picked
      ? `${picked.l}×${picked.w}×${picked.h}`
      : `${cur.l ?? "?"}×${cur.w ?? "?"}×${cur.h ?? "?"}`;
    const weightLine =
      goodsType === "实重货" || S.extra
        ? `模拟实 ${fmtWeight(simW)}，共（${cur.count ?? "?"} 件）`
        : `实 ${fmtWeight(cur.weight ?? NaN)}，共（${cur.count ?? "?"} 件）`;
    output = `${head}\n${dims}/${divisor}=${fmt(simVol)}\n${weightLine}`;
  }
  return {
    ref, cur, refVol, newVol, simWRaw, simW, simVolTarget, goodsType, ok, divisor,
    fitDims, fitTarget, cands, recIdx, picked, simDims, simVol, simWeight, simGirth, output,
  };
}

/* ---------- 渲染 ---------- */
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));

function stepperHtml(k, v, step) {
  return `<div class="stepper"><input class="ed" data-k="${k}" inputmode="decimal" value="${
    v ?? ""
  }"><div class="stepper-btns"><button type="button" data-k="${k}" data-d="${step}" title="+${step}">▲</button><button type="button" data-k="${k}" data-d="-${step}" title="-${step}">▼</button></div></div>`;
}

function rowsHtml(R) {
  const { ref, cur, refVol, newVol, simVol, simWeight, simGirth, simDims } = R;
  const cell = (k, v, step) => `<td>${stepperHtml(k, v, step)}</td>`;
  const perW = (w, p) => fmt(w != null && p ? w / p : NaN);
  const perV = (vol, p) => fmt(p && Number.isFinite(vol) ? vol / p : NaN);
  return `
  <tr>
    <td class="tag">参考</td>
    ${cell("ref.pcs", ref.pcs, 1)}
    ${cell("ref.l", ref.l, 1)}${cell("ref.w", ref.w, 1)}${cell("ref.h", ref.h, 1)}
    <td class="mono" id="g_ref">${fmt(girth(ref.l, ref.w, ref.h) ?? NaN)}</td>
    <td class="mono" id="v_ref">${fmt(refVol)}</td>
    ${cell("ref.weight", ref.weight, 0.05)}
    <td class="mono" id="uw_ref">${perW(ref.weight, ref.pcs)}</td>
    <td class="mono" id="uv_ref">${perV(refVol, ref.pcs)}</td>
  </tr>
  <tr>
    <td class="tag">实际</td>
    ${cell("cur.pcs", cur.pcs, 1)}
    ${cell("cur.l", cur.l, 1)}${cell("cur.w", cur.w, 1)}${cell("cur.h", cur.h, 1)}
    <td class="mono" id="g_cur">${fmt(girth(cur.l, cur.w, cur.h) ?? NaN)}</td>
    <td class="mono" id="v_cur">${fmt(newVol)}</td>
    ${cell("cur.weight", cur.weight, 0.05)}
    <td class="mono" id="uw_cur">${perW(cur.weight, cur.pcs)}</td>
    <td class="mono" id="uv_cur">${perV(newVol, cur.pcs)}</td>
  </tr>
  <tr class="simrow">
    <td class="tag">模拟<br><span class="mode-tag">${R.goodsType === "抛货" ? "抛货" : "实重"}</span></td>
    <td class="mono" id="pcs_sim">${numOrDash(cur.pcs)}</td>
    <td class="mono" id="l_sim">${simDims.l ?? "-"}</td>
    <td class="mono" id="w2_sim">${simDims.w ?? "-"}</td>
    <td class="mono" id="h_sim">${simDims.h ?? "-"}</td>
    <td class="mono" id="g_sim">${fmt(simGirth ?? NaN)}</td>
    <td class="mono" id="v_sim">${fmt(simVol)}</td>
    <td class="mono" id="wt_sim">${fmtWeight(simWeight ?? NaN)}</td>
    <td class="mono" id="uw_sim">${fmt(simWeight != null && cur.pcs ? simWeight / cur.pcs : NaN)}</td>
    <td class="mono" id="uv_sim">${fmt(cur.pcs && Number.isFinite(simVol) ? simVol / cur.pcs : NaN)}</td>
  </tr>`;
}

function renderTable(R) {
  $("tableBody").innerHTML = rowsHtml(R);
}

function updateTableValues(R) {
  // 只改值不重建，保住输入焦点
  const map = {
    "ref.pcs": R.ref.pcs, "ref.l": R.ref.l, "ref.w": R.ref.w, "ref.h": R.ref.h, "ref.weight": R.ref.weight,
    "cur.pcs": R.cur.pcs, "cur.l": R.cur.l, "cur.w": R.cur.w, "cur.h": R.cur.h, "cur.weight": R.cur.weight,
  };
  document.querySelectorAll("#tableBody input.ed").forEach((inp) => {
    if (inp === document.activeElement) return;
    const v = map[inp.dataset.k];
    const s = v == null ? "" : String(v);
    if (inp.value !== s) inp.value = s;
  });
}

function renderSim(R) {
  const judge = $("judgeLine");
  judge.innerHTML = `判定（按参考数据）：<b>${esc(R.goodsType ?? "-")}</b>${
    R.goodsType === "抛货" ? " → 按「模拟抛货」" : " → 按「模拟实重」"
  }`;

  const f = $("formulaLine");
  const simVolLine = `模拟体积重 = ${numOrDash(R.cur.pcs)}/${numOrDash(R.ref.pcs)} × ${fmt(R.refVol)} = <b>${fmt(
    R.simVolTarget
  )}</b>（模拟体积重/参考体积重 = 实际数量/参考数量）`;
  if (R.goodsType === "抛货") {
    f.innerHTML = simVolLine;
  } else {
    f.innerHTML = `模拟实重 = ${numOrDash(R.cur.pcs)}/${numOrDash(R.ref.pcs)} × ${numOrDash(
      R.ref.weight
    )} = ${fmt(R.simWRaw)} → 按 0.05 进位 = <b>${fmtWeight(R.simW)}</b>${
      R.fitDims ? "<br>" + simVolLine : ""
    }`;
  }

  $("extraLabel").textContent =
    R.goodsType === "抛货"
      ? "是否模拟重量（实重也按数量比放大：模拟实重 = 数量比 × 参考实重）"
      : "是否模拟尺寸（模拟体积重 = 参考体积重 × 数量比，按参考箱比例凑整）";
  $("extraBox").checked = S.extra;

  const cw = $("candsWrap");
  if (R.fitDims && R.cands.length) {
    cw.innerHTML = `<div class="table-wrap"><table class="cands"><thead><tr>
      <th>选择</th><th>长×宽×高</th><th>体积重</th><th>模拟体积重</th><th>差值</th><th>比例偏差</th>
    </tr></thead><tbody>${R.cands
      .map(
        (c, i) => `<tr data-i="${i}" class="${i === S.pickedIdx ? "picked" : ""}">
      <td><input type="radio" name="cand" ${i === S.pickedIdx ? "checked" : ""}></td>
      <td>${c.l}×${c.w}×${c.h}${i === R.recIdx ? '<span class="rec">推荐</span>' : ""}</td>
      <td class="mono">${fmt(c.result)}</td>
      <td class="mono">${fmt(R.fitTarget)}</td>
      <td class="mono">${c.result - R.fitTarget >= 0 ? "+" : ""}${fmt(c.result - R.fitTarget)}</td>
      <td class="mono">${(c.dev * 100).toFixed(1)}%</td>
    </tr>`
      )
      .join("")}</tbody></table></div>`;
  } else if (R.fitDims && R.ok) {
    cw.innerHTML = `<div class="err">没有找到满足条件的整数组合，请检查参考箱尺寸。</div>`;
  } else {
    cw.innerHTML = "";
  }

  const ow = $("outWrap");
  if (R.output) {
    const hint =
      R.goodsType === "抛货"
        ? S.extra
          ? "模拟抛货：凑整尺寸盖住模拟体积重，实重已按数量比放大"
          : "模拟抛货：凑整尺寸盖住模拟体积重，实重不变"
        : S.extra
        ? "模拟实重+模拟尺寸：模拟体积重/参考体积重 = 实际数量/参考数量"
        : "模拟实重：尺寸未改动，实重替换为模拟值";
    ow.innerHTML = `<div class="out-wrap">
      <div class="muted">输出（按箱规格式，可直接复制）</div>
      <pre class="out mono" id="outPre">${esc(R.output)}</pre>
      <div><button class="btn" id="copyBtn">复制</button> <span class="hint">${hint}</span></div>
    </div>`;
  } else {
    ow.innerHTML = "";
  }
}

let lastOutput = "";
function updateUI(src) {
  const R = compute();
  if (!src || src === "ta") {
    renderTable(R);
  } else {
    updateTableValues(R);
    // 表格里的只读格也要刷新
    renderTableReadonly(R);
  }
  renderSim(R);
  lastOutput = R.output;
}

/** 部分刷新：只更新对照表里的只读单元格 */
function renderTableReadonly(R) {
  const cells = {
    "g_ref": girth(R.ref.l, R.ref.w, R.ref.h),
    "v_ref": R.refVol,
    "uw_ref": R.ref.weight != null && R.ref.pcs ? R.ref.weight / R.ref.pcs : NaN,
    "uv_ref": R.ref.pcs && Number.isFinite(R.refVol) ? R.refVol / R.ref.pcs : NaN,
    "g_cur": girth(R.cur.l, R.cur.w, R.cur.h),
    "v_cur": R.newVol,
    "uw_cur": R.cur.weight != null && R.cur.pcs ? R.cur.weight / R.cur.pcs : NaN,
    "uv_cur": R.cur.pcs && Number.isFinite(R.newVol) ? R.newVol / R.cur.pcs : NaN,
    "g_sim": R.simGirth,
    "v_sim": R.simVol,
    "w_sim": R.simWeight,
    "uw_sim": R.simWeight != null && R.cur.pcs ? R.simWeight / R.cur.pcs : NaN,
    "uv_sim": R.cur.pcs && Number.isFinite(R.simVol) ? R.simVol / R.cur.pcs : NaN,
  };
  for (const [id, v] of Object.entries(cells)) {
    const el = document.getElementById(id);
    if (el) el.textContent = fmt(v);
  }
  const dims = [R.simDims.l, R.simDims.w, R.simDims.h];
  ["l_sim", "w2_sim", "h_sim"].forEach((id, i) => {
    const el = document.getElementById(id);
    if (el) el.textContent = dims[i] ?? "-";
  });
  const pcsEl = document.getElementById("pcs_sim");
  if (pcsEl) pcsEl.textContent = numOrDash(R.cur.pcs);
  const wtEl = document.getElementById("wt_sim");
  if (wtEl) wtEl.textContent = fmtWeight(R.simWeight ?? NaN);
}

/* ---------- 表格只读格 id 已在 rowsHtml 中直接标注，无需覆写 ---------- */

/* ---------- 事件绑定 ---------- */
function bind() {
  $("refText").addEventListener("input", (e) => {
    S.refText = e.target.value;
    save();
    updateUI("ta");
  });
  $("newText").addEventListener("input", (e) => {
    S.newText = e.target.value;
    save();
    updateUI("ta");
  });
  $("extraBox").addEventListener("change", (e) => {
    S.extra = e.target.checked;
    S.pickedIdx = 0;
    save();
    updateUI("ta"); // 表格重建（模拟行变）
  });

  const tbody = $("tableBody");
  tbody.addEventListener("input", (e) => {
    const inp = e.target.closest("input.ed");
    if (!inp) return;
    const key = inp.dataset.k;
    const n = Number(inp.value.trim());
    patch(key, inp.value.trim() === "" || !Number.isFinite(n) ? null : n);
    updateUI(key); // 部分刷新，保焦点
  });
  tbody.addEventListener("click", (e) => {
    const btn = e.target.closest("button[data-k]");
    if (!btn) return;
    const key = btn.dataset.k;
    const d = parseFloat(btn.dataset.d);
    const cur = getField(key) ?? 0;
    patch(key, Math.round((cur + d) * 100) / 100);
    updateUI("ta"); // 全量重建（按钮点击无焦点问题）
  });

  $("candsWrap").addEventListener("change", (e) => {
    if (e.target.name !== "cand") return;
    const tr = e.target.closest("tr[data-i]");
    if (tr) {
      S.pickedIdx = +tr.dataset.i;
      save();
      updateUI(null);
    }
  });
  $("candsWrap").addEventListener("click", (e) => {
    const tr = e.target.closest("tr[data-i]");
    if (tr && !e.target.matches("input")) {
      S.pickedIdx = +tr.dataset.i;
      save();
      updateUI(null);
    }
  });
  $("outWrap").addEventListener("click", async (e) => {
    if (e.target.id !== "copyBtn") return;
    try {
      await navigator.clipboard.writeText(lastOutput);
      e.target.textContent = "✓ 已复制";
      setTimeout(() => {
        e.target.textContent = "复制";
      }, 1500);
    } catch (err) {
      /* 忽略 */
    }
  });
}

/* ---------- 字段读写 ---------- */
function getField(key) {
  const ref = parseBlock(S.refText);
  const cur = parseBlock(S.newText);
  const [blk, fld] = key.split(".");
  return (blk === "ref" ? ref : cur)[fld];
}
function patch(key, value) {
  const [blk, fld] = key.split(".");
  const base = parseBlock(blk === "ref" ? S.refText : S.newText);
  const b = { ...base, [fld]: value };
  if (b.divisor == null) b.divisor = 6000;
  if (b.l != null && b.w != null && b.h != null) {
    b.vol = (b.l * b.w * b.h) / b.divisor;
  }
  const text = blockToText(b);
  if (blk === "ref") S.refText = text;
  else S.newText = text;
  save();
  // 同步输入框（不在焦点）
  const ta = blk === "ref" ? $("refText") : $("newText");
  if (document.activeElement !== ta) ta.value = text;
}

/* ---------- PWA ---------- */
if ("serviceWorker" in navigator && location.protocol === "https:") {
  navigator.serviceWorker.register("./sw.js").catch(() => {});
}

/* ---------- 记录（存储抽象：当前为本地实现，可切换云端） ----------
 * PlanStore.list() / save(rec) / remove(id)
 * 本地后端：localStorage；若日后开通云服务，仅需替换此对象实现。 */
const PlanStore = (() => {
  const KEY = "boxplan-pwa-records";
  const read = () => {
    try {
      const a = JSON.parse(localStorage.getItem(KEY) || "[]");
      return Array.isArray(a) ? a : [];
    } catch (e) {
      return [];
    }
  };
  const write = (a) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(a));
    } catch (e) {
      /* 忽略 */
    }
  };
  return {
    list: read,
    save(rec) {
      const a = read();
      const id = "r" + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
      a.unshift({ id, savedAt: Date.now(), ...rec });
      write(a.slice(0, 50)); // 本地最多保留 50 条
      return a.find((x) => x.id === id);
    },
    remove(id) {
      write(read().filter((x) => x.id !== id));
    },
  };
})();

function renderRecords() {
  const list = PlanStore.list();
  const wrap = $("recList");
  if (!list.length) {
    wrap.innerHTML = `<div class="rec-empty">还没有保存的记录，填好数据后点「保存当前」。</div>`;
    return;
  }
  wrap.innerHTML = list
    .map(
      (r) => `<div class="rec-item">
      <div class="rec-main">
        <div class="rec-name">${esc(r.name || "未命名")}</div>
        <div class="rec-meta">${esc(r.summary || "")} · ${new Date(r.savedAt).toLocaleString("zh-CN")}</div>
      </div>
      <div class="rec-btns">
        <button class="primary" data-load="${r.id}">载入</button>
        <button class="del" data-del="${r.id}">删除</button>
      </div>
    </div>`
    )
    .join("");
}

function bindRecords() {
  $("saveRec").addEventListener("click", () => {
    const R = compute();
    if (!R.ok) return;
    const name = $("recName").value.trim() || `${R.cur.client || ""}${R.cur.product || "箱规"}`.trim();
    PlanStore.save({
      name,
      refText: S.refText,
      newText: S.newText,
      summary: `${R.goodsType ?? ""}｜模拟实 ${fmtWeight(R.simW)}`,
    });
    $("recName").value = "";
    renderRecords();
  });
  $("recList").addEventListener("click", (e) => {
    const loadId = e.target.dataset && e.target.dataset.load;
    const delId = e.target.dataset && e.target.dataset.del;
    if (loadId) {
      const rec = PlanStore.list().find((x) => x.id === loadId);
      if (!rec) return;
      S.refText = rec.refText;
      S.newText = rec.newText;
      S.extra = false;
      S.pickedIdx = 0;
      save();
      $("refText").value = S.refText;
      $("newText").value = S.newText;
      updateUI("ta");
    } else if (delId) {
      PlanStore.remove(delId);
      renderRecords();
    }
  });
}

/* ---------- 启动 ---------- */
function init() {
  $("refText").value = S.refText;
  $("newText").value = S.newText;
  compute.lastGoods = undefined;
  bind();
  bindRecords();
  renderRecords();
  updateUI(null);
}
init();
