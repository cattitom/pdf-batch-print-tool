// 通过已登录的 Chrome（CDP 9222）把 WPS 云表格下载成本地 .xls。
//
// 为什么要借 Chrome：kdocs 的下载接口要登录 cookie，cookie 在浏览器里；我们从页面
// 上下文里 fetch 一次 `/api/v3/office/file/<fid>/download`，拿到 presigned 的
// download_url，再用 Node 直接下（那一步不需要 cookie）。
//
// usage: node kdocs-pull.mjs <outFile> [--doc=<kdocs 链接>] [--port=9222] [--force]
// stdout: 一行 JSON {ok, file, bytes, fid, title, docUrl, fver, skipped}
// 版本号（fver）没变且本地文件还在时**不下载**，直接 skipped=true 返回：
// 问一句版本号只有 989 字节，整本下载是 4.4MB。
// 失败时非零退出，stderr 给人看的中文原因。

import { existsSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { assertNodeRuntime } from "./node-runtime.mjs";
try { assertNodeRuntime(); } catch (error) { console.error(error.message); process.exit(2); }


const args = process.argv.slice(2);
const outFile = args.find((a) => !a.startsWith("--"));
const opt = (name, def) => {
  const hit = args.find((a) => a.startsWith(`--${name}=`));
  return hit ? hit.slice(name.length + 3) : def;
};

if (!outFile) {
  console.error("用法: node kdocs-pull.mjs <outFile> [--doc=<链接>] [--port=9222]");
  process.exit(2);
}

const port = Number(opt("port", "9222"));
let activePort = port;
const wantDoc = opt("doc", "");
// 兜底：浏览器里读不到 cookie 时用这个（来货登记（新））
const FALLBACK_FID = "489766365819";
// 没开浏览器时自己起无界面 Chrome，复用 kdocs-login.bat 那个登录过的配置目录
const DOC_URL = wantDoc || "https://www.kdocs.cn/l/cie4FmnjnBhW";
// Chrome 有两种装法：装给全机器（Program Files）和「仅为当前用户安装」（AppData\Local）。
// 只认 Program Files 会在用户级安装的机器上一直失败，所以按顺序都找一遍。
const CHROME_CANDIDATES = [
  process.env.KDOCS_CHROME,
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
  `${process.env.LOCALAPPDATA ?? ""}\\Google\\Chrome\\Application\\chrome.exe`,
].filter(Boolean);
const CHROME =
  CHROME_CANDIDATES.find((p) => existsSync(p)) ?? CHROME_CANDIDATES[1] ?? CHROME_CANDIDATES[0];
const PROFILE = `${process.env.LOCALAPPDATA ?? "."}\\kdocs-cdp-profile`;
const headlessPort = Number(opt("headless-port", "9333"));

function cdpTargets(p = activePort) {
  return fetch(`http://127.0.0.1:${p}/json/list`).then((r) => r.json());
}

/** 起一个无界面 Chrome（同一个 profile → 登录态还在），进程退出时顺手关掉。 */
function startHeadless() {
  const kid = spawn(
    CHROME,
    [
      "--headless=new",
      `--remote-debugging-port=${headlessPort}`,
      `--user-data-dir=${PROFILE}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-gpu",
      "--hide-scrollbars",
      "--window-size=1440,900",
      DOC_URL,
    ],
    { stdio: "ignore", windowsHide: true },
  );
  // 别让子进程把 Node 的事件循环挂住：下载成功后脚本要能自己退出
  kid.unref();
  kid.on("error", () => {
    // 起不来就让后面的轮询报错，别抛
  });
  process.on("exit", () => {
    try {
      kid.kill();
    } catch {
      // 已经退了
    }
  });
  return kid;
}

async function evaluate(target, expr) {
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  const pending = new Map();
  let id = 0;
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const mid = ++id;
      pending.set(mid, { resolve, reject });
      ws.send(JSON.stringify({ id: mid, method, params }));
    });
  ws.addEventListener("message", (ev) => {
    const m = JSON.parse(ev.data);
    const p = m.id && pending.get(m.id);
    if (!p) return;
    pending.delete(m.id);
    m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result);
  });
  try {
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve, { once: true });
      ws.addEventListener("error", () => reject(new Error("连不上 Chrome 调试端口")), { once: true });
    });
    await send("Runtime.enable");
    const r = await send("Runtime.evaluate", {
      expression: `(async () => { ${expr} })()`,
      awaitPromise: true,
      returnByValue: true,
    });
    if (r.exceptionDetails) {
      throw new Error(String(r.exceptionDetails.exception?.description ?? JSON.stringify(r.exceptionDetails)).slice(0, 300));
    }
    return r.result?.value;
  } finally {
    ws.close();
  }
}

let targets = null;
try {
  targets = await cdpTargets();
} catch {
  targets = null;
}

// 连不上就自己起一个无界面 Chrome（复用同一个登录过的 profile），同步完随手关掉
let selfStarted = false;
if (!targets && !args.includes("--no-headless")) {
  // 没装 Chrome 就直接说什么问题，别傻等 45 秒把桌面端拖住
  if (!existsSync(CHROME)) {
    console.error(
      "没装 Chrome（找过这些位置都没找到）：\n" +
        CHROME_CANDIDATES.map((p) => `  ${p}`).join("\n") +
        "\n云表格同步要借一个登录过 WPS 的 Chrome 取数：装好 Chrome 后点页头「WPS 登录」按钮（或双击 kdocs-login.bat）登录一次，再点同步。"
    );
    process.exit(3);
  }
  startHeadless();
  selfStarted = true;
  activePort = headlessPort;
  const hasDoc = () =>
    (targets ?? []).some((t) => t.type === "page" && /kdocs\.cn/.test(t.url));
  for (let i = 0; i < 40 && !hasDoc(); i++) {
    await new Promise((r) => setTimeout(r, 500));
    try {
      targets = await cdpTargets(headlessPort);
    } catch {
      targets = null;
    }
  }
  if (!hasDoc()) {
    // 起来了但停在登录页 ≠ 起不来：这两种情况给的话不一样
    const seen = (targets ?? []).filter((t) => t.type === "page").map((t) => t.url);
    if (seen.some((u) => /account\.kdocs\.cn|passport|\/login/.test(u))) {
      console.error(
        "无界面 Chrome 起来了，但停在 WPS 登录页——配置目录里的登录态过期了。\n" +
          "点「WPS 登录」按钮扫码登录一次（或双击 kdocs-login.bat），然后再点同步。"
      );
      process.exit(5);
    }
    console.error(
      `连不上 Chrome 调试端口 ${port}，无界面模式也没起来（端口 ${headlessPort}）。\n` +
        "最常见的原因：「WPS 登录」那个窗口还开着——同一个配置目录不能同时开两次。\n" +
        "把那个窗口关掉再点同步，或者干脆留着窗口、不用无界面模式。"
    );
    process.exit(3);
  }
}

const pages = targets.filter((t) => t.type === "page" && /kdocs\.cn/.test(t.url));
const target = wantDoc ? pages.find((p) => p.url.includes(wantDoc)) ?? pages[0] : pages[0];
if (!target) {
  console.error(
    "Chrome 开着，但没有 WPS 云表格标签页。\n" +
      "点「WPS 登录」按钮（或双击 kdocs-login.bat）会打开 https://www.kdocs.cn/l/cie4FmnjnBhW 并保持登录。"
  );
  process.exit(4);
}

// fid 从页面 cookie 里取（reuse_session_id=edit/489766365819），取不到用兜底值
const READ_META = `
  let ck = "";
  let denied = false;
  try { ck = document.cookie || ""; } catch { denied = true; }
  return {
    ok: /^https?:/.test(location.href) && !denied,
    denied,
    href: location.href,
    ready: document.readyState,
    fid: (ck.match(/reuse_session_id=edit\\/(\\d+)/) || [])[1] || "",
    title: document.title || "",
  };
`;

// 刚拉起 Chrome 时 /json/list 已经报出 kdocs 的 URL，但文档还是初始 about:blank，
// 这时读 document.cookie 会抛 SecurityError: Access is denied for this document
//（2026-10-31 用户点同步就撞上）。等到文档真正提交、拿到 kdocs 的源再往下走。
async function waitMeta(t, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    try {
      last = await evaluate(t, READ_META);
      if (last?.ok) return last;
    } catch {
      // 初始文档还没换掉，继续等
    }
    await new Promise((r) => setTimeout(r, 400));
  }
  return last;
}

let meta = await waitMeta(target);
if (!meta?.ok) {
  console.error(
    `云表格页面还没加载完（停在 ${meta?.href ?? "?"}）。\n` +
      "等一下再点同步；一直这样就把「WPS 登录」那个窗口刷新一次。"
  );
  process.exit(4);
}
if (selfStarted && !meta.fid) {
  // 无界面刚起来时页面还在跑登录态，等它一会儿再问
  for (let i = 0; i < 10 && !meta.fid; i++) {
    await new Promise((r) => setTimeout(r, 1000));
    meta = (await waitMeta(target, 3000)) || meta;
  }
}
// 页面没登录时 cookie 里没有 reuse_session_id
const fid = meta?.fid || FALLBACK_FID;

// 页面刚提交时可能马上又跳一次（登录态探测），fetch 会跟着失败，重试几次
let info = null;
for (let i = 0; i < 5; i++) {
  try {
    info = await evaluate(
      target,
      `
      const r = await fetch("/api/v3/office/file/${fid}/download?isblocks=false", { credentials: "include" });
      const text = await r.text();
      let j = null;
      try { j = JSON.parse(text); } catch {}
      return { status: r.status, isJson: !!j, url: j && j.download_url, size: j && j.file_size, fver: j && j.fver, head: text.slice(0, 120) };
    `
    );
  } catch {
    info = null;
  }
  if (info) break;
  await new Promise((r) => setTimeout(r, 800));
}
if (!info) {
  console.error("页面还在跳转，取不到下载地址，等一下再点同步。");
  process.exit(6);
}

if (info?.status === 401 || info?.status === 403 || !info?.isJson) {
  console.error(
    `WPS 云表格没登录（下载接口 HTTP ${info?.status ?? "?"}）。\n` +
      "在「WPS 登录」打开的窗口里扫码登录，登录完成后保持窗口开着再点同步。"
  );
  process.exit(5);
}
if (!info.url) {
  console.error("拿不到下载地址：" + JSON.stringify(info).slice(0, 300));
  process.exit(6);
}

// 版本号没变就不下整本（表格每次保存 fver 都会变）
const stampFile = `${outFile}.fver`;
const fver = info.fver == null ? "" : String(info.fver);
const cached = existsSync(stampFile) ? readFileSync(stampFile, "utf8").trim() : "";
if (!args.includes("--force") && fver && fver === cached && existsSync(outFile)) {
  console.log(
    JSON.stringify({
      ok: true,
      skipped: true,
      file: outFile,
      bytes: statSync(outFile).size,
      fid,
      fver,
      title: meta?.title ?? "",
      docUrl: target.url,
    })
  );
  process.exit(0);
}

const res = await fetch(info.url);
if (!res.ok) {
  console.error(`下载失败 HTTP ${res.status}`);
  process.exit(7);
}
const buf = Buffer.from(await res.arrayBuffer());
const magic = buf.subarray(0, 4).toString("hex");
if (magic !== "d0cf11e0" && magic !== "504b0304") {
  console.error(`下载到的不是表格文件（magic=${magic}）`);
  process.exit(8);
}
writeFileSync(outFile, buf);
writeFileSync(stampFile, fver);
console.log(
  JSON.stringify({
    ok: true,
    skipped: false,
    file: outFile,
    bytes: buf.length,
    fid,
    fver,
    title: meta?.title ?? "",
    docUrl: target.url,
  })
);
process.exit(0);
