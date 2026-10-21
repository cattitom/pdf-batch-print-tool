import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, errorText, SHOW_LABEL_TAB_KEY } from "../api";

export default function SettingsPage({
  showLabelTab,
  onShowLabelTab,
}: {
  showLabelTab: boolean;
  onShowLabelTab: (v: boolean) => void;
}) {
  const [settings, setSettings] = useState<Record<string, string>>({});
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [newKey, setNewKey] = useState("");
  const [saved, setSaved] = useState(false);

  const reload = () => {
    api.allSettings().then((s) => {
      setSettings(s);
      setDrafts(s);
    });
  };

  useEffect(reload, []);

  const save = async (key: string) => {
    const v = drafts[key] ?? "";
    await api.setSetting(key, v);
    setSettings((prev) => ({ ...prev, [key]: v }));
    flash();
  };

  const flash = () => {
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };

  return (
    <div className="page">
      <div className="toolbar">
        <span className="muted">偏好设置保存在本机数据库，界面打开时按需读取。</span>
        <div className="spacer" />
        <button
          className="btn"
          title="打开 app.log 所在目录（报障时附上它）"
          onClick={async () => {
            try {
              const p = await api.logDir();
              await revealItemInDir(p);
            } catch (e) {
              alert(`打开日志目录失败：${errorText(e)}`);
            }
          }}
        >
          📂 打开日志目录
        </button>
        {saved && <span className="saved-hint">已保存 ✓</span>}
      </div>

      <div className="settings-list">
        <div className="setting-row">
          <div className="setting-key">显示「箱唛生成」页签</div>
          <label className="setting-toggle">
            <input
              type="checkbox"
              checked={showLabelTab}
              onChange={async (e) => {
                const v = e.target.checked;
                onShowLabelTab(v);
                try {
                  await api.setSetting(SHOW_LABEL_TAB_KEY, v ? "1" : "0");
                } catch (err) {
                  alert(`保存失败：${errorText(err)}`);
                }
              }}
            />
            <span>箱唛不常用，默认隐藏；勾选后页签栏才出现</span>
          </label>
        </div>
        {Object.keys(settings).length === 0 && (
          <div className="empty">
            <div className="empty-icon">⚙️</div>
            <div>暂无设置项</div>
            <div className="empty-sub">在下方添加，如 defaultTarget / sepPages / sepText</div>
          </div>
        )}
        {Object.entries(drafts)
          .filter(([k]) => k !== SHOW_LABEL_TAB_KEY)
          .map(([k, v]) => (
          <div className="setting-row" key={k}>
            <div className="setting-key mono">{k}</div>
            <input
              value={v}
              onChange={(e) => setDrafts((prev) => ({ ...prev, [k]: e.target.value }))}
            />
            <button className="btn" onClick={() => save(k)}>
              保存
            </button>
          </div>
          ))}
        <div className="setting-row new">
          <input
            className="mono"
            value={newKey}
            onChange={(e) => setNewKey(e.target.value)}
            placeholder="新设置项 key"
          />
          <button
            className="btn"
            disabled={newKey.trim() === ""}
            onClick={() => {
              save(newKey.trim());
              setNewKey("");
            }}
          >
            添加
          </button>
        </div>
      </div>
    </div>
  );
}
