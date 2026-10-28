import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  api,
  errorText,
  SHOW_BOXPLAN_TAB_KEY,
  SHOW_LABEL_TAB_KEY,
  SHOW_PRODUCT_TAB_KEY,
} from "../api";

/** 这些键由隐藏页签开关管理，不再出现在下面的通用 key-value 列表里。 */
const MANAGED_KEYS = new Set([
  SHOW_LABEL_TAB_KEY,
  SHOW_BOXPLAN_TAB_KEY,
  SHOW_PRODUCT_TAB_KEY,
  // 旧版快捷键设置，已废弃
  "shortcut_label_tab",
  "shortcut_boxplan_tab",
]);

/** 隐藏页签开关：勾选即写入偏好，App 启动时读取。 */
function TabToggle({
  settingKey,
  title,
  hint,
  checked,
  onChecked,
}: {
  settingKey: string;
  title: string;
  hint: string;
  checked: boolean;
  onChecked: (v: boolean) => void;
}) {
  return (
    <div className="setting-row">
      <div className="setting-key">{title}</div>
      <label className="setting-toggle">
        <input
          type="checkbox"
          checked={checked}
          onChange={async (e) => {
            const v = e.target.checked;
            onChecked(v);
            try {
              await api.setSetting(settingKey, v ? "1" : "0");
            } catch (err) {
              alert(`保存失败：${errorText(err)}`);
            }
          }}
        />
        <span>{hint}</span>
      </label>
    </div>
  );
}

export default function SettingsPage({
  showLabelTab,
  onShowLabelTab,
  showBoxplanTab,
  onShowBoxplanTab,
  showProductTab,
  onShowProductTab,
}: {
  showLabelTab: boolean;
  onShowLabelTab: (v: boolean) => void;
  showBoxplanTab: boolean;
  onShowBoxplanTab: (v: boolean) => void;
  showProductTab: boolean;
  onShowProductTab: (v: boolean) => void;
}) {
  const [settings, setSettings] = useState<Record<string, string>>({});
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [newKey, setNewKey] = useState("");
  const [saved, setSaved] = useState(false);
  // 隐藏页签开关默认不渲染，三击「其它设置」才展开
  const [hiddenTabsVisible, setHiddenTabsVisible] = useState(false);

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

  const otherSettingCount = Object.keys(settings).filter((k) => !MANAGED_KEYS.has(k)).length;

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
        <div
          className="settings-group-title secret-trigger"
          onClick={(e) => {
            // 三击展开/收起隐藏页签开关
            if (e.detail === 3) setHiddenTabsVisible((v) => !v);
          }}
        >
          其它设置
        </div>

        {hiddenTabsVisible && (
          <>
            <TabToggle
              settingKey={SHOW_LABEL_TAB_KEY}
              title="显示「箱唛生成」页签"
              hint="箱唛不常用，默认隐藏；勾选后页签栏才出现"
              checked={showLabelTab}
              onChecked={onShowLabelTab}
            />
            <TabToggle
              settingKey={SHOW_BOXPLAN_TAB_KEY}
              title="显示「箱规计划」页签"
              hint="箱规计划不常用，默认隐藏；勾选后页签栏才出现"
              checked={showBoxplanTab}
              onChecked={onShowBoxplanTab}
            />
            <TabToggle
              settingKey={SHOW_PRODUCT_TAB_KEY}
              title="显示「产品标签」页签"
              hint="默认显示；不想用时可在这里关掉"
              checked={showProductTab}
              onChecked={onShowProductTab}
            />
          </>
        )}

        {otherSettingCount === 0 && (
          <div className="empty">
            <div className="empty-icon">⚙️</div>
            <div>暂无其它设置项</div>
            <div className="empty-sub">在下方添加，如 defaultTarget / sepPages / sepText</div>
          </div>
        )}
        {Object.entries(drafts)
          .filter(([k]) => !MANAGED_KEYS.has(k))
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

