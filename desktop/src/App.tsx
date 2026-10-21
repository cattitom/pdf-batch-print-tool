import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { api, SHOW_LABEL_TAB_KEY } from "./api";
import ImposePage from "./pages/ImposePage";
import LabelPage from "./pages/LabelPage";
import HistoryPage from "./pages/HistoryPage";
import SettingsPage from "./pages/SettingsPage";
import "./App.css";

type TabKey = "impose" | "label" | "history" | "settings";

interface TabDef {
  key: TabKey;
  label: string;
}

const BASE_TABS: TabDef[] = [
  { key: "impose", label: "🖨 PDF 拼版" },
  { key: "history", label: "🕘 历史" },
  { key: "settings", label: "⚙️ 设置" },
];

/** 箱唛不常用：默认不出现在页签栏，由「设置」里的开关控制。 */
const LABEL_TAB: TabDef = { key: "label", label: "🏷 箱唛生成" };


export interface PendingDrop {
  paths: string[];
  nonce: number;
}

export default function App() {
  const [tab, setTab] = useState<TabKey>("impose");
  const [pendingDrop, setPendingDrop] = useState<PendingDrop | null>(null);
  const [showLabelTab, setShowLabelTab] = useState(false);

  // 启动时读一次偏好；没存过就按隐藏处理
  useEffect(() => {
    api
      .allSettings()
      .then((s) => setShowLabelTab(s[SHOW_LABEL_TAB_KEY] === "1"))
      .catch(() => {});
  }, []);

  // 开关被关掉时如果正停在箱唛页，退回拼版页
  useEffect(() => {
    if (!showLabelTab && tab === "label") setTab("impose");
  }, [showLabelTab, tab]);

  // 全局系统拖放：把 PDF 拖进窗口任意位置 → 自动切到拼版页并添加
  useEffect(() => {
    let un: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type !== "drop") return;
        const paths = event.payload.paths.filter((p) => p.toLowerCase().endsWith(".pdf"));
        if (paths.length === 0) return;
        setTab("impose");
        setPendingDrop({ paths, nonce: Date.now() });
      })
      .then((u) => (un = u));
    return () => un?.();
  }, []);

  const tabs = showLabelTab ? [BASE_TABS[0], LABEL_TAB, ...BASE_TABS.slice(1)] : BASE_TABS;

  return (
    <div className="app">
      <nav className="tabbar">
        {tabs.map((t) => (
          <button
            key={t.key}
            className={`tab ${tab === t.key ? "active" : ""}`}
            onClick={() => setTab(t.key)}
          >
            {t.label}
          </button>
        ))}
      </nav>
      <main className="content">
        {tab === "impose" && <ImposePage pendingDrop={pendingDrop} />}
        {tab === "label" && <LabelPage />}
        {tab === "history" && <HistoryPage />}
        {tab === "settings" && (
          <SettingsPage showLabelTab={showLabelTab} onShowLabelTab={setShowLabelTab} />
        )}
      </main>
    </div>
  );
}
