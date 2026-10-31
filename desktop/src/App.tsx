import { useEffect, useState, type ReactNode } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  BarcodeOutlined,
  FilePdfOutlined,
  FolderOpenOutlined,
  InboxOutlined,
  PrinterOutlined,
  SettingOutlined,
  TagsOutlined,
  UnorderedListOutlined,
} from "@ant-design/icons";
import { api, SHOW_BOXPLAN_TAB_KEY, SHOW_LABEL_TAB_KEY, SHOW_PRODUCT_TAB_KEY } from "./api";
import ImposePage from "./pages/ImposePage";
import LabelPage from "./pages/LabelPage";
import ProductLabelPage from "./pages/ProductLabelPage";
import SettingsPage from "./pages/SettingsPage";
import BoxPlanPage from "./pages/BoxPlanPage";
import PackingPage from "./pages/PackingPage";
import "./App.css";

type TabKey = "impose" | "packing" | "boxplan" | "label" | "product" | "settings";

interface TabDef {
  key: TabKey;
  label: string;
  icon: ReactNode;
}

const BASE_TABS: TabDef[] = [
  { key: "impose", label: "PDF 拼版", icon: <FilePdfOutlined /> },
  { key: "packing", label: "出货清单", icon: <UnorderedListOutlined /> },
  { key: "settings", label: "设置", icon: <SettingOutlined /> },
];

/** 箱唛、箱规、产品标签不常用：默认不出现在导航栏，在设置里开启后常驻。 */
const LABEL_TAB: TabDef = { key: "label", label: "箱唛生成", icon: <BarcodeOutlined /> };
const BOXPLAN_TAB: TabDef = { key: "boxplan", label: "箱规计划", icon: <InboxOutlined /> };
const PRODUCT_TAB: TabDef = { key: "product", label: "产品标签", icon: <TagsOutlined /> };

export interface PendingDrop {
  paths: string[];
  nonce: number;
}

export default function App() {
  const [tab, setTab] = useState<TabKey>("impose");
  const [pendingDrop, setPendingDrop] = useState<PendingDrop | null>(null);
  const [packingDrop, setPackingDrop] = useState<PendingDrop | null>(null);
  const [showLabelTab, setShowLabelTab] = useState(false);
  const [showBoxplanTab, setShowBoxplanTab] = useState(false);
  const [showProductTab, setShowProductTab] = useState(false);

  // 启动时读一次偏好；没存过就按隐藏处理
  useEffect(() => {
    api
      .allSettings()
      .then((s) => {
        setShowLabelTab(s[SHOW_LABEL_TAB_KEY] === "1");
        setShowBoxplanTab(s[SHOW_BOXPLAN_TAB_KEY] === "1");
        setShowProductTab(s[SHOW_PRODUCT_TAB_KEY] !== "0");
      })
      .catch(() => {});
  }, []);

  // 开关被关掉时如果正停在被隐藏的页，退回拼版页
  useEffect(() => {
    if (!showLabelTab && tab === "label") setTab("impose");
    if (!showBoxplanTab && tab === "boxplan") setTab("impose");
    if (!showProductTab && tab === "product") setTab("impose");
  }, [showLabelTab, showBoxplanTab, showProductTab, tab]);

  // 全局系统拖放：PDF → 拼版页；表格/图片 → 装箱单解析页
  useEffect(() => {
    let un: (() => void) | undefined;
    const SHEET_OR_IMAGE = /\.(xlsx|xlsm|xls|csv|txt|tsv|png|jpe?g|webp|bmp)$/i;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type !== "drop") return;
        const paths = event.payload.paths;
        const pdfs = paths.filter((p) => p.toLowerCase().endsWith(".pdf"));
        if (pdfs.length > 0) {
          setTab("impose");
          setPendingDrop({ paths: pdfs, nonce: Date.now() });
          return;
        }
        const sheets = paths.filter((p) => SHEET_OR_IMAGE.test(p));
        if (sheets.length > 0) {
          setTab("packing");
          setPackingDrop({ paths: sheets, nonce: Date.now() });
        }
      })
      .then((u) => (un = u));
    return () => un?.();
  }, []);

  const tabs = [
    BASE_TABS[0],
    BASE_TABS[1],
    ...(showLabelTab ? [LABEL_TAB] : []),
    ...(showBoxplanTab ? [BOXPLAN_TAB] : []),
    ...(showProductTab ? [PRODUCT_TAB] : []),
    BASE_TABS[2],
  ];

  const openLogDir = async () => {
    try {
      const p = await api.logDir();
      await revealItemInDir(p);
    } catch {
      // 日志目录打不开不该打断主流程
    }
  };

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">
            <PrinterOutlined />
          </div>
          <div>
            <div className="brand-name">批量打印</div>
            <div className="brand-sub">PDF 拼版工具</div>
          </div>
        </div>

        <nav className="nav">
          <div className="nav-label">工作台</div>
          {tabs.map((t) => (
            <button
              key={t.key}
              className={`tab${tab === t.key ? " active" : ""}`}
              onClick={() => setTab(t.key)}
              aria-current={tab === t.key ? "page" : undefined}
            >
              {t.icon}
              <span>{t.label}</span>
            </button>
          ))}
        </nav>

        <div className="sidebar-foot">
          <button className="sidebar-link" onClick={openLogDir} title="打开日志目录（报障时附上 app.log）">
            <FolderOpenOutlined />
            日志目录
          </button>
          <span className="sidebar-ver">v0.1.0</span>
        </div>
      </aside>

      <main className="content">
        {/* 各页保持挂载、只切显隐：切走再回来时表格、勾选、筛选都还在 */}
        <div style={{ display: tab === "impose" ? "block" : "none" }}>
          <ImposePage pendingDrop={pendingDrop} />
        </div>
        <div style={{ display: tab === "packing" ? "block" : "none" }}>
          <PackingPage pendingDrop={packingDrop} />
        </div>
        <div style={{ display: tab === "boxplan" ? "block" : "none" }}>
          <BoxPlanPage />
        </div>
        <div style={{ display: tab === "label" ? "block" : "none" }}>
          <LabelPage />
        </div>
        <div style={{ display: tab === "product" ? "block" : "none" }}>
          <ProductLabelPage />
        </div>
        <div style={{ display: tab === "settings" ? "block" : "none" }}>
          <SettingsPage
            showLabelTab={showLabelTab}
            onShowLabelTab={setShowLabelTab}
            showBoxplanTab={showBoxplanTab}
            onShowBoxplanTab={setShowBoxplanTab}
            showProductTab={showProductTab}
            onShowProductTab={setShowProductTab}
          />
        </div>
      </main>
    </div>
  );
}
