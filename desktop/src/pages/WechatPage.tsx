import { useEffect, useMemo, useRef, useState } from "react";
import { open as openFiles, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, errorHint, errorText, type WechatMsg, type WechatRoom, type WechatStatus } from "../api";

const KEY_WECHAT_DIRS = "wechat_data_dirs";
const KEY_WECHAT_PINNED = "wechat_pinned_rooms";

/**
 * 微信消息：读本机微信 4.x 的本地库。
 * 先「扫描 / 解密」把 db_storage 解成明文副本，再按群浏览、导出 CSV。
 */
export default function WechatPage() {
  const [status, setStatus] = useState<WechatStatus | null>(null);
  const [account, setAccount] = useState("");
  const [rooms, setRooms] = useState<WechatRoom[]>([]);
  const [roomsLoading, setRoomsLoading] = useState(false);
  const [roomsNonce, setRoomsNonce] = useState(0);
  const [roomQuery, setRoomQuery] = useState("");
  const [roomId, setRoomId] = useState("");
  const [msgs, setMsgs] = useState<WechatMsg[]>([]);
  const [loadingMsgs, setLoadingMsgs] = useState(false);
  const [days, setDays] = useState(0);
  const [kind, setKind] = useState(0);
  const [limit, setLimit] = useState(200);
  const [scanning, setScanning] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [monitoring, setMonitoring] = useState(false);
  const lastTsRef = useRef(0);
  const pollBusyRef = useRef(false);
  const [customDirs, setCustomDirs] = useState<string[]>([]);
  const [pinned, setPinned] = useState<string[]>([]);
  const [onlyPinned, setOnlyPinned] = useState(false);
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const accountInfo = status?.accounts.find((a) => a.dataDir === account);
  const selectedRoom = rooms.find((r) => r.id === roomId);

  // 启动：读上次手选的目录，再拉账号
  useEffect(() => {
    let alive = true;
    (async () => {
      let dirs: string[] = [];
      let pinnedRooms: string[] = [];
      try {
        const s = await api.allSettings();
        const raw = s[KEY_WECHAT_DIRS];
        if (raw) {
          const parsed = JSON.parse(raw);
          if (Array.isArray(parsed)) {
            dirs = parsed.filter((x): x is string => typeof x === "string" && !!x.trim());
          }
        }
        const rawPinned = s[KEY_WECHAT_PINNED];
        if (rawPinned) {
          const parsed = JSON.parse(rawPinned);
          if (Array.isArray(parsed)) {
            pinnedRooms = parsed.filter((x): x is string => typeof x === "string" && !!x.trim());
          }
        }
      } catch {
        // 设置读不出来就用自动发现
      }
      if (!alive) return;
      setCustomDirs(dirs);
      setPinned(pinnedRooms);
      setOnlyPinned(pinnedRooms.length > 0);
      const st = await api.wechatStatus(dirs);
      if (!alive) return;
      setStatus(st);
      setAccount(st.selected ?? "");
    })().catch((e) => {
      if (alive) setError(errorText(e) + errorHint(e));
    });
    return () => {
      alive = false;
    };
  }, []);

  // 账号变了（或刚扫完）就重拉群列表；没解密的账号不查，免得弹「还没解密」
  useEffect(() => {
    if (!account || !accountInfo?.decrypted) {
      setRooms([]);
      return;
    }
    let alive = true;
    setRoomsLoading(true);
    api
      .wechatRooms(account)
      .then((list) => {
        if (!alive) return;
        setRooms(list);
        setRoomId((prev) => (list.some((r) => r.id === prev) ? prev : (list[0]?.id ?? "")));
        setError(null);
      })
      .catch((e) => {
        if (alive) setError(errorText(e) + errorHint(e));
      })
      .finally(() => {
        if (alive) setRoomsLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [account, roomsNonce, accountInfo?.decrypted]);

  // 群 / 天数 / 类型 / 条数 变了就重拉消息
  useEffect(() => {
    if (!account || !roomId) {
      setMsgs([]);
      return;
    }
    let alive = true;
    setLoadingMsgs(true);
    api
      .wechatMessages(account, roomId, days || null, limit, kind || null)
      .then((r) => {
        if (alive) setMsgs(r);
      })
      .catch((e) => {
        if (alive) setError(errorText(e) + errorHint(e));
      })
      .finally(() => {
        if (alive) setLoadingMsgs(false);
      });
    return () => {
      alive = false;
    };
  }, [account, roomId, days, kind, limit]);

  // 监控：每 2 秒让后端补 WAL、取新消息；出错就停
  useEffect(() => {
    if (!monitoring || !account || !roomId) return;
    let alive = true;
    const tick = async () => {
      if (pollBusyRef.current) return; // 上一次还在补 WAL 就跳过，别叠着跑
      pollBusyRef.current = true;
      try {
        const r = await api.wechatPoll(account, roomId, lastTsRef.current);
        if (!alive) return;
        if (r.length) {
          setMsgs((prev) => {
            const seen = new Set(prev.map((m) => `${m.ts}|${m.sender}|${m.text}`));
            const add = r.filter((m) => !seen.has(`${m.ts}|${m.sender}|${m.text}`));
            return add.length ? [...prev, ...add] : prev;
          });
          lastTsRef.current = Math.max(lastTsRef.current, ...r.map((m) => m.ts));
        }
      } catch (e) {
        if (!alive) return;
        setMonitoring(false);
        setError("监控中断：" + errorText(e) + errorHint(e));
      } finally {
        pollBusyRef.current = false;
      }
    };
    void tick();
    const timer = window.setInterval(tick, 2000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [monitoring, account, roomId]);

  const pinnedSet = useMemo(() => new Set(pinned), [pinned]);
  const shownRooms = useMemo(() => {
    const base = onlyPinned ? rooms.filter((r) => pinnedSet.has(r.id)) : rooms;
    const q = roomQuery.trim().toLowerCase();
    if (!q) return base;
    return base.filter((r) => r.name.toLowerCase().includes(q) || r.id.toLowerCase().includes(q));
  }, [rooms, roomQuery, onlyPinned, pinnedSet]);

  const extracted = useMemo(
    () =>
      msgs.filter(
        (m) =>
          // 表情只当分割线；文本、引用、文件都参与提取（数量常在引用里）
          m.cat === "表情" ||
          (m.fields && (m.fields.customer || m.fields.sku || m.fields.qty)),
      ),
    [msgs],
  );

  const pickDir = async () => {
    const picked = await openFiles({
      directory: true,
      multiple: false,
      title: "选择微信数据目录",
    });
    if (!picked || Array.isArray(picked)) return;
    setPicking(true);
    setError(null);
    setNotice(null);
    try {
      const found = await api.wechatCheckDir(picked);
      if (!found.length) {
        setError(
          "这个目录里没找到 wxid_* 账号：选「微信 → 设置 → 文件管理」里的 xwechat_files，或它下面 wxid_ 开头那个文件夹",
        );
        return;
      }
      const dirs = Array.from(new Set([...customDirs, picked]));
      setCustomDirs(dirs);
      api.setSetting(KEY_WECHAT_DIRS, JSON.stringify(dirs)).catch(() => {});
      const st = await api.wechatStatus(dirs);
      setStatus(st);
      setAccount(found[0].dataDir);
      setNotice(`已添加账号 ${found[0].wxid}`);
    } catch (e) {
      setError(errorText(e) + errorHint(e));
    } finally {
      setPicking(false);
    }
  };

  const scan = async () => {
    if (!account) return;
    setScanning(true);
    setError(null);
    setNotice(null);
    try {
      const r = await api.wechatScan(account);
      const parts = [`解密 ${r.ok} 个库`];
      if (r.failed) parts.push(`失败 ${r.failed}`);
      if (r.walFrames) parts.push(`WAL 补 ${r.walFrames} 帧`);
      parts.push(`${(r.ms / 1000).toFixed(1)}s`);
      setNotice(parts.join("，"));
      const s = await api.wechatStatus(customDirs);
      setStatus(s);
      setRoomsNonce((n) => n + 1);
    } catch (e) {
      setError(errorText(e) + errorHint(e));
    } finally {
      setScanning(false);
    }
  };

  const exportCsv = async () => {
    if (!account || !roomId) return;
    const name = selectedRoom?.name || "微信消息";
    const day = new Date().toISOString().slice(0, 10);
    const path = await saveFileDialog({
      defaultPath: `${name}-${day}.csv`,
      filters: [{ name: "CSV", extensions: ["csv"] }],
    });
    if (!path) return;
    setExporting(true);
    setError(null);
    try {
      const r = await api.wechatExportCsv(account, roomId, days || null, kind || null, path);
      setNotice(`已导出 ${r.rows} 条`);
      revealItemInDir(r.path).catch(() => {});
    } catch (e) {
      setError(errorText(e) + errorHint(e));
    } finally {
      setExporting(false);
    }
  };

  const togglePin = (id: string) => {
    const next = pinned.includes(id) ? pinned.filter((x) => x !== id) : [...pinned, id];
    setPinned(next);
    api.setSetting(KEY_WECHAT_PINNED, JSON.stringify(next)).catch(() => {});
  };

  const togglePinnedView = (v: boolean) => {
    setOnlyPinned(v);
    if (v) {
      const first = rooms.find((r) => pinned.includes(r.id));
      if (first && !pinned.includes(roomId)) {
        setRoomId(first.id);
        setMonitoring(false);
      }
    }
  };

  const toggleMonitor = () => {
    if (!monitoring) {
      // 从当前最后一条开始盯，避免把历史消息当新消息重复追加
      lastTsRef.current = msgs.length ? msgs[msgs.length - 1].ts : Math.floor(Date.now() / 1000);
      setError(null);
    }
    setMonitoring((v) => !v);
  };

  return (
    <div className="page wide">
      <header className="page-head">
        <div>
          <h1 className="page-title">微信消息</h1>
          <p className="page-sub">读取本机微信 4.x 的群消息：扫描解密 → 选群 → 导出 CSV</p>
        </div>
        <div className="page-actions">
          <button
            className="btn ghost"
            onClick={pickDir}
            disabled={picking}
            title="微信把文件目录改到别的盘时，手动指定 xwechat_files 或 wxid_* 账号目录"
          >
            {picking ? "检查中…" : "选择目录"}
          </button>
          {status && status.accounts.length > 1 && (
            <select
              className="wechat-select"
              value={account}
              onChange={(e) => {
                setAccount(e.target.value);
                setRoomId("");
                setMsgs([]);
                setMonitoring(false);
              }}
              title="选择微信账号"
            >
              {status.accounts.map((a) => (
                <option key={a.dataDir} value={a.dataDir}>
                  {a.wxid}
                  {a.decrypted ? "" : "（未解密）"}
                </option>
              ))}
            </select>
          )}
          <button className="btn primary" onClick={scan} disabled={!account || scanning}>
            {scanning ? "解密中…" : "扫描 / 解密"}
          </button>
        </div>
      </header>

      {status && status.accounts.length === 0 && (
        <section className="card">
          <div className="card-body">
            <p className="wechat-empty-title">没找到微信数据目录</p>
            <p className="muted">
              默认找 <span className="mono">Documents\xwechat_files\wxid_*</span>。如果微信把文件目录
              改到了别的盘，点下面按钮选一下：路径在「微信 → 设置 → 文件管理」里，选
              <span className="mono"> xwechat_files </span>
              或它下面 <span className="mono">wxid_</span> 开头的文件夹都行。
            </p>
            <button className="btn" style={{ marginTop: 10 }} onClick={pickDir} disabled={picking}>
              {picking ? "检查中…" : "选择微信数据目录"}
            </button>
          </div>
        </section>
      )}

      {status && status.accounts.length > 0 && accountInfo && !accountInfo.decrypted && (
        <section className="card">
          <div className="card-body">
            <p className="wechat-empty-title">这个账号还没解密</p>
            <p className="muted">
              点右上角「扫描 / 解密」。需要微信保持登录；如果提示找不到密钥，用管理员权限重新打开本程序。
              解密结果写在账号目录同级的 <span className="mono">wechat_msg_decrypted\&lt;wxid&gt;</span>，原库只读。
            </p>
          </div>
        </section>
      )}

      {accountInfo?.decrypted && (
        <div className="wechat-layout">
          <section className="card wechat-left">
            <div className="card-head">
              <span className="card-title">群聊</span>
              <span className="card-sub">
                {pinned.length ? `${pinned.length} 关注` : `${rooms.length} 个`}
              </span>
              <label className="wechat-only-pinned" title="只显示标了星的群">
                <input
                  type="checkbox"
                  checked={onlyPinned}
                  onChange={(e) => togglePinnedView(e.target.checked)}
                />
                只看关注
              </label>
            </div>
            <div className="card-body tight">
              <input
                className="wechat-room-search"
                placeholder="搜群名 / wxid"
                value={roomQuery}
                onChange={(e) => setRoomQuery(e.target.value)}
                spellCheck={false}
              />
              <div className="wechat-room-list">
                {roomsLoading && <div className="muted">读取中…</div>}
                {!roomsLoading && shownRooms.length === 0 && (
                  <div className="muted">
                    {onlyPinned
                      ? pinned.length
                        ? "关注的群里没有匹配的"
                        : "点群名右边的 ★ 关注几个常看的群"
                      : rooms.length
                        ? "没匹配的群"
                        : "没有群消息"}
                  </div>
                )}
                {shownRooms.map((r) => (
                  <div key={r.id} className="wechat-room-row">
                    <button
                      className={`wechat-room${r.id === roomId ? " active" : ""}`}
                      onClick={() => {
                        setRoomId(r.id);
                        setMonitoring(false);
                      }}
                      title={r.id}
                    >
                      <span className="rname">{r.name}</span>
                      <span className="rmeta">
                        {r.count} 条{r.last ? ` · ${dayText(r.last)}` : ""}
                      </span>
                    </button>
                    <button
                      className={`wechat-pin${pinnedSet.has(r.id) ? " on" : ""}`}
                      onClick={() => togglePin(r.id)}
                      title={pinnedSet.has(r.id) ? "取消关注" : "关注，列表里常显示"}
                      aria-label="关注群"
                    >
                      ★
                    </button>
                  </div>
                ))}
              </div>
            </div>
          </section>

          <div className="wechat-right">
          <section className="card wechat-main">
            <div className="card-head">
              <span className="card-title">{selectedRoom?.name ?? "选一个群"}</span>
              <span className="card-sub">{msgs.length} 条</span>
              <div className="wechat-filters">
                <select
                  className="wechat-select"
                  value={days}
                  onChange={(e) => setDays(Number(e.target.value))}
                >
                  <option value={0}>全部时间</option>
                  <option value={1}>最近 1 天</option>
                  <option value={3}>最近 3 天</option>
                  <option value={7}>最近 7 天</option>
                  <option value={30}>最近 30 天</option>
                </select>
                <select
                  className="wechat-select"
                  value={kind}
                  onChange={(e) => setKind(Number(e.target.value))}
                >
                  <option value={0}>全部类型</option>
                  <option value={1}>文本</option>
                  <option value={3}>图片</option>
                  <option value={49}>文件 / 引用</option>
                  <option value={10000}>系统</option>
                </select>
                <select
                  className="wechat-select"
                  value={limit}
                  onChange={(e) => setLimit(Number(e.target.value))}
                >
                  <option value={100}>最近 100 条</option>
                  <option value={200}>最近 200 条</option>
                  <option value={500}>最近 500 条</option>
                  <option value={1000}>最近 1000 条</option>
                </select>
                <button
                  className={`btn sm${monitoring ? " danger" : ""}`}
                  onClick={toggleMonitor}
                  disabled={!roomId}
                  title="每 2 秒补一次 WAL，盯这个群的新消息"
                >
                  {monitoring ? "停止监控" : "监控新消息"}
                </button>
                <button
                  className="btn sm"
                  onClick={exportCsv}
                  disabled={!roomId || !msgs.length || exporting}
                  title="导出当前筛选条件下的全部消息（不受条数限制）"
                >
                  {exporting ? "导出中…" : "导出 CSV"}
                </button>
              </div>
            </div>
            <div className="card-body tight">
              {loadingMsgs ? (
                <div className="muted">读取中…</div>
              ) : msgs.length === 0 ? (
                <div className="muted">没有消息，换个群或放宽时间/类型试试</div>
              ) : (
                <div className="wechat-msg-wrap">
                  <table className="hist-table wechat-msg-table">
                    <thead>
                      <tr>
                        <th style={{ width: 150 }}>时间</th>
                        <th style={{ width: 130 }}>发送者</th>
                        <th style={{ width: 70 }}>类别</th>
                        <th>内容</th>
                      </tr>
                    </thead>
                    <tbody>
                      {msgs.map((m, i) => (
                        <tr key={`${m.ts}-${i}`}>
                          <td className="mono">{m.time}</td>
                          <td>{m.sender}</td>
                          <td>{m.cat}</td>
                          <td className="msg-text">{m.text}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </div>
          </section>

          <section className="card wechat-extract">
            <div className="card-head">
              <span className="card-title">提取信息</span>
              <span className="card-sub">{extracted.length} 条</span>
            </div>
            <div className="card-body tight">
              {extracted.length === 0 ? (
                <div className="muted">
                  消息里没抓到打包记录（文本、引用、文件都会解析；表情当分割线）
                </div>
              ) : (
                <div className="wechat-extract-wrap">
                  <table className="hist-table wechat-msg-table">
                    <thead>
                      <tr>
                        <th style={{ width: 110 }}>日期</th>
                        <th style={{ width: 120 }}>客户</th>
                        <th>SKU</th>
                        <th style={{ width: 80 }}>数量</th>
                      </tr>
                    </thead>
                    <tbody>
                      {extracted.map((m, i) =>
                        m.cat === "表情" ? (
                          <tr key={`x-${m.ts}-${i}`} className="wechat-divider-row">
                            <td colSpan={4}>—— 分割线 ——</td>
                          </tr>
                        ) : (
                          <tr key={`x-${m.ts}-${i}`}>
                            <td className="mono">{m.time.slice(0, 10)}</td>
                            <td>{m.fields?.customer}</td>
                            <td className="msg-text">{m.fields?.sku}</td>
                            <td>{m.fields?.qty}</td>
                          </tr>
                        ),
                      )}
                    </tbody>
                  </table>
                </div>
              )}
            </div>
          </section>
          </div>
        </div>
      )}

      {error && <div className="banner err">{error}</div>}
      {notice && <div className="banner ok">{notice}</div>}
    </div>
  );
}

function dayText(ts: number): string {
  const d = new Date(ts * 1000);
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${mm}-${dd}`;
}
