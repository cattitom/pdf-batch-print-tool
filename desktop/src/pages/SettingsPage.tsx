import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { FolderOpenOutlined, SettingOutlined } from "@ant-design/icons";
import {
  api,
  errorText,
  SHOW_BOXPLAN_TAB_KEY,
  SHOW_LABEL_TAB_KEY,
  SHOW_PRODUCT_TAB_KEY,
} from "../api";

/** 这些键由隐藏页签开关管理，不再出现在下面的通用列表里。 */
const MANAGED_KEYS = new Set([
  SHOW_LABEL_TAB_KEY,
  SHOW_BOXPLAN_TAB_KEY,
  SHOW_PRODUCT_TAB_KEY,
  // 旧版快捷键设置，已废弃
  "shortcut_label_tab",
  "shortcut_boxplan_tab",
]);

/**
 * 已知设置项的中文名与用途。
 * state: true 表示功能自己写的状态（只读展示，不给输入框，避免手改坏 JSON）。
 */
const KEY_META: Record<string, { label: string; desc: string; state?: boolean }> = {
  packing_auto_open: {
    label: "启动时打开上次文件",
    desc: "1 = 打开，0 = 不打开（出货页的勾选框）",
  },
  packing_last_dir: {
    label: "上次打开的文件夹",
    desc: "「选择文件」默认从这个目录开始",
    state: true,
  },
  packing_last_file: {
    label: "上次打开的表格",
    desc: "启动时自动打开、点「刷新」重新解析都用它",
    state: true,
  },
  packing_picked_waybills: {
    label: "上次筛选的单号",
    desc: "JSON 数组；出货页按这几个单号过滤",
    state: true,
  },
  packing_pinned: {
    label: "钉住的条目",
    desc: "JSON 数组；出货页左侧「钉住」栏里的货",
    state: true,
  },
  product_label_csv_path: {
    label: "上次导出的 CSV",
    desc: "再导出产品标签时默认覆盖这个文件",
    state: true,
  },
};

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
      <div className="setting-main">
        <div className="setting-name">{title}</div>
        <div className="setting-desc">{hint}</div>
      </div>
      <label className="switch">
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
        <span className="switch-track" aria-hidden="true" />
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

  const flash = () => {
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };

  const save = async (key: string) => {
    const v = drafts[key] ?? "";
    await api.setSetting(key, v);
    setSettings((prev) => ({ ...prev, [key]: v }));
    flash();
  };

  const clearKey = async (key: string) => {
    await api.setSetting(key, "");
    setSettings((prev) => ({ ...prev, [key]: "" }));
    setDrafts((prev) => ({ ...prev, [key]: "" }));
    flash();
  };

  const entries = Object.entries(drafts).filter(([k]) => !MANAGED_KEYS.has(k));
  const prefEntries = entries.filter(([k]) => !KEY_META[k]?.state);
  const stateEntries = Object.entries(settings).filter(
    ([k, v]) => !MANAGED_KEYS.has(k) && KEY_META[k]?.state && v !== ""
  );

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1 className="page-title">设置</h1>
          <p className="page-sub">偏好设置保存在本机数据库，界面打开时按需读取</p>
        </div>
        <div className="page-actions">
          {saved && <span className="saved-hint">已保存 ✓</span>}
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
            <FolderOpenOutlined />
            打开日志目录
          </button>
        </div>
      </header>

      <section className="card">
        <div
          className="card-head"
          onClick={(e) => {
            // 三击展开/收起隐藏页签开关
            if (e.detail === 3) setHiddenTabsVisible((v) => !v);
          }}
        >
          <span className="card-title">其它设置</span>
          <span className="card-sub">改完点「保存」即写入本机；灰字是程序自己记的状态</span>
        </div>
        <div className="card-body">
          <div className="settings-list">
            {hiddenTabsVisible && (
              <>
                <div className="settings-group-title">导航栏显示</div>
                <TabToggle
                  settingKey={SHOW_LABEL_TAB_KEY}
                  title="显示「箱唛生成」"
                  hint="箱唛不常用，默认隐藏；打开后导航栏才出现"
                  checked={showLabelTab}
                  onChecked={onShowLabelTab}
                />
                <TabToggle
                  settingKey={SHOW_BOXPLAN_TAB_KEY}
                  title="显示「箱规计划」"
                  hint="箱规计划不常用，默认隐藏；打开后导航栏才出现"
                  checked={showBoxplanTab}
                  onChecked={onShowBoxplanTab}
                />
                <TabToggle
                  settingKey={SHOW_PRODUCT_TAB_KEY}
                  title="显示「产品标签」"
                  hint="默认显示；不想用时可在这里关掉"
                  checked={showProductTab}
                  onChecked={onShowProductTab}
                />
                <div className="settings-group-title" style={{ marginTop: 10 }}>
                  可改的偏好
                </div>
              </>
            )}

            {prefEntries.length === 0 && (
              <div className="settings-empty">
                <SettingOutlined className="settings-empty-icon" />
                <div>没有可改的偏好</div>
                <div className="empty-sub">在下方添加键值对，如 defaultTarget / sepPages</div>
              </div>
            )}

            {prefEntries.map(([k, v]) => {
              const meta = KEY_META[k];
              return (
                <div className="setting-row" key={k}>
                  <div className="setting-key">
                    <div className="setting-name">{meta?.label ?? k}</div>
                    {meta && <div className="setting-desc mono">{k}</div>}
                    {meta?.desc && <div className="setting-desc">{meta.desc}</div>}
                  </div>
                  <input
                    value={v}
                    onChange={(e) => setDrafts((prev) => ({ ...prev, [k]: e.target.value }))}
                  />
                  <button className="btn sm" onClick={() => save(k)}>
                    保存
                  </button>
                </div>
              );
            })}

            <div className="setting-row new">
              <input
                className="mono"
                value={newKey}
                onChange={(e) => setNewKey(e.target.value)}
                placeholder="新设置项 key"
              />
              <button
                className="btn soft"
                disabled={newKey.trim() === ""}
                onClick={() => {
                  save(newKey.trim());
                  setNewKey("");
                }}
              >
                添加
              </button>
            </div>

            {stateEntries.length > 0 && (
              <>
                <div className="settings-group-title" style={{ marginTop: 14 }}>
                  程序自动记录
                </div>
                <div className="settings-note">
                  打开文件、筛选、钉住时自动写入；删掉只是丢上次的位置和筛选，不影响已经录进去的数据。
                </div>
                {stateEntries.map(([k, v]) => (
                  <div className="setting-row state" key={k}>
                    <div className="setting-main">
                      <div className="setting-name">
                        {KEY_META[k].label}
                        <code className="setting-raw">{k}</code>
                      </div>
                      <div className="setting-desc">{KEY_META[k].desc}</div>
                    </div>
                    <div className="setting-value mono" title={v}>
                      {v}
                    </div>
                    <button className="link-btn" onClick={() => clearKey(k)} title="清空这条记录">
                      清除
                    </button>
                  </div>
                ))}
              </>
            )}
          </div>
        </div>
      </section>
    </div>
  );
}
