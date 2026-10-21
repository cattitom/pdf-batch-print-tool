// E2E：CDP 连 WebView2，验证 UI 渲染 + 真实调用 impose_pdfs
const path = require("path");
const { chromium } = require("playwright-core");

const CDP = "http://localhost:9223";
// 测试 PDF 在仓库内（core/tests/fixtures），不再依赖项目外目录
const FIX = path.join(__dirname, "..", "core", "tests", "fixtures");
const OUT = path.join(require("os").tmpdir(), "e2e_impose_out.pdf");

(async () => {
  const browser = await chromium.connectOverCDP(CDP);
  const ctx = browser.contexts()[0];
  let page = ctx.pages().find((p) => p.url().includes("localhost:1420")) || ctx.pages()[0];
  if (!page) throw new Error("找不到页面: " + ctx.pages().map((p) => p.url()));
  console.log("PAGE:", page.url());

  // 1. UI 渲染检查
  const tabs = await page.locator("button.tab").allTextContents();
  console.log("TABS:", JSON.stringify(tabs));
  if (!tabs.some((t) => t.includes("PDF 拼版"))) throw new Error("拼版页签缺失");

  // 2. pdf_infos
  const infos = await page.evaluate(async (p) => {
    const r = await window.__TAURI__.core.invoke("pdf_infos", { paths: [p] });
    return r;
  }, `${FIX}\\CROP-A4-3页.pdf`);
  console.log("PDF_INFOS:", JSON.stringify(infos));
  if (infos[0].pages !== 3) throw new Error("pdf_infos 页数错误: " + JSON.stringify(infos));
  // camelCase 修复验证（NaN×NaNmm bug）
  if (typeof infos[0].widthMm !== "number" || !(infos[0].widthMm > 0))
    throw new Error("widthMm 缺失（camelCase 未生效）: " + JSON.stringify(infos));

  // 2b. preview_pdf 命令
  const pv = await page.evaluate(async (p) => {
    return await window.__TAURI__.core.invoke("preview_pdf", { path: p, dpi: 72, maxPages: 2 });
  }, `${FIX}\\CROP-A4-3页.pdf`);
  console.log("PREVIEW_PAGES:", pv.length, "first len:", pv[0].length);
  if (pv.length !== 2 || pv[0].length < 1000) throw new Error("preview_pdf 异常");

  // 2c. HTML5 拖拽机制验证（dragDropEnabled=false 后 WebView2 恢复原生 DnD）
  await page.evaluate(() => {
    window.__dndLog = [];
    const mk = (id, color, top) => {
      const d = document.createElement("div");
      d.id = id;
      d.textContent = id;
      d.style.cssText = `position:fixed;top:${top}px;left:60px;width:120px;height:40px;background:${color};z-index:99999;`;
      d.draggable = true;
      d.addEventListener("dragstart", () => window.__dndLog.push("dragstart:" + id));
      d.addEventListener("dragover", (e) => e.preventDefault());
      d.addEventListener("drop", (e) => {
        e.preventDefault();
        window.__dndLog.push("drop:" + id);
      });
      document.body.appendChild(d);
    };
    mk("dnd-src", "#fbb", 300);
    mk("dnd-dst", "#bfb", 360);
  });
  const src = page.locator("#dnd-src");
  const dst = page.locator("#dnd-dst");
  await src.hover();
  await page.mouse.down();
  await dst.hover();
  await page.mouse.up();
  await page.waitForTimeout(300);
  const log = await page.evaluate(() => window.__dndLog);
  console.log("DND_LOG:", JSON.stringify(log));
  await page.evaluate(() => {
    document.getElementById("dnd-src")?.remove();
    document.getElementById("dnd-dst")?.remove();
  });
  if (!log.some((l) => l.startsWith("dragstart")) || !log.includes("drop:dnd-dst"))
    throw new Error("HTML5 拖拽未生效（dragDropEnabled 修复失败）");


  // 3. 真实拼版：2 个文件，A4 纸 + 分隔页 + 自动裁剪 + 旋转
  const res = await page.evaluate(
    async ({ a, b, out }) => {
      return await window.__TAURI__.core.invoke("impose_pdfs", {
        files: [
          { name: "CROP-A4-3页.pdf", path: a, targetPages: 4, rotate: 90, crop: { type: "auto", marginMm: 2 } },
          { name: "PROBE-A4.pdf", path: b, targetPages: 2, rotate: 0, crop: null },
        ],
        opts: { paperWmm: 210, paperHmm: 297, sepPages: 1, sepText: "这是分隔线" },
        outPath: out,
      });
    },
    { a: `${FIX}\\CROP-A4-3页.pdf`, b: `${FIX}\\PROBE-A4.pdf`, out: OUT }
  );
  console.log("IMPOSE_RESULT:", JSON.stringify(res));
  // 4 页 + 1 分隔页 + 2 页 = 7 页
  if (res.pages !== 7) throw new Error("拼版页数错误: " + JSON.stringify(res));

  // 4. 历史：模拟 UI 流程写入并查询
  const added = await page.evaluate((out) => {
    return window.__TAURI__.core.invoke("add_print_history", {
      entry: {
        outputName: "e2e_out.pdf",
        outputPath: out,
        outputPages: 7,
        paper: "210×297mm",
        sepPages: 1,
        params: { via: "e2e" },
        files: [{ name: "CROP-A4-3页.pdf", targetPages: 4, rotate: 90, crop: null }],
      },
    });
  }, OUT);
  const hist = await page.evaluate(() => window.__TAURI__.core.invoke("list_print_history", { limit: 5, offset: 0 }));
  console.log("HISTORY_ADD_ID:", added, "COUNT:", hist.length, "first:", hist[0] && hist[0].outputName);
  if (hist.length === 0) throw new Error("历史未写入");

  // 5. 箱唛预览（base64）
  const b64 = await page.evaluate(() =>
    window.__TAURI__.core.invoke("preview_label", {
      spec: {
        inboundNo: "IN-E2E-TEST",
        warehouse: "测试仓",
        client: "测试客户",
        shelf: "1",
        skus: [{ sku: "220-ABC", boxes: 2, pcsPerBox: 250, quantities: "250,249", newFlag: true }],
      },
    })
  );
  console.log("LABEL_PREVIEW_B64_LEN:", b64.length);
  if (b64.length < 1000) throw new Error("箱唛预览太小");

  console.log("E2E ALL PASS");
  await browser.close();
})().catch((e) => {
  console.error("E2E FAIL:", e.message);
  process.exit(1);
});
