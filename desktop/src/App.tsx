import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import ImposePage from "./pages/ImposePage";
import LabelPage from "./pages/LabelPage";
import HistoryPage from "./pages/HistoryPage";
import SettingsPage from "./pages/SettingsPage";
import "./App.css";

const TABS = [
  { key: "impose", label: "🖨 PDF 拼版" },
  { key: "label", label: "🏷 箱唛生成" },
  { key: "history", label: "🕘 历史" },
  { key: "settings", label: "⚙️ 设置" },
] as const;

type TabKey = (typeof TABS)[number]["key"];

export interface PendingDrop {
  paths: string[];
  nonce: number;
}

export default function App() {
  const [tab, setTab] = useState<TabKey>("impose");
  const [pendingDrop, setPendingDrop] = useState<PendingDrop | null>(null);

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

  return (
    <div className="app">
      <nav className="tabbar">
        {TABS.map((t) => (
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
        {tab === "settings" && <SettingsPage />}
      </main>
    </div>
  );
}
