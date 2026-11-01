import { useEffect, useRef, useState } from "react";
import { Drawer } from "antd";
import type { MobileInfoDto } from "../../api";

/** 防火墙放行命令：手机连不上时复制到管理员 PowerShell 跑一次（"一键放行"用的是同一条规则） */
const FIREWALL_CMD =
  'netsh advfirewall firewall add rule name="PdfMobileReceive" dir=in action=allow protocol=TCP localport=17650-17652';

type Props = {
  info: MobileInfoDto;
  /** 复制任意文本（页面那边 Clipboard 有回退实现） */
  onCopy: (text: string) => void;
  onCopyUrl: () => void;
  /** 换新口令 */
  onReset: () => void;
  /** 一键放行防火墙（后端会请 UAC）；返回给用户看的一句话 */
  onAllowFirewall: () => Promise<string>;
  /** 只收起面板，手机端服务继续跑 */
  onClose: () => void;
  /** 真正停掉手机端服务 */
  onStop: () => void;
};

/**
 * 手机出货面板：右侧抽屉 + 不遮罩，收起时服务照常跑，
 * 这样一边看表格一边用手机勾选（2026-11-01 用户反馈：居中弹窗挡视线）。
 */
export function MobileSheetModal({
  info,
  onCopy,
  onCopyUrl,
  onReset,
  onAllowFirewall,
  onClose,
  onStop,
}: Props) {
  const [copiedCmd, setCopiedCmd] = useState(false);
  const [allowBusy, setAllowBusy] = useState(false);
  const [allowMsg, setAllowMsg] = useState<string | null>(null);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const allowFirewall = async () => {
    setAllowBusy(true);
    setAllowMsg(null);
    try {
      setAllowMsg(await onAllowFirewall());
    } catch (e) {
      setAllowMsg(e instanceof Error ? e.message : String(e));
    } finally {
      setAllowBusy(false);
    }
  };

  const copyFirewallCmd = async () => {
    await onCopy(FIREWALL_CMD);
    setCopiedCmd(true);
    timer.current = window.setTimeout(() => setCopiedCmd(false), 2000);
  };

  return (
    <Drawer
      title="手机出货"
      placement="right"
      width={420}
      open
      onClose={onClose}
      mask={false}
      styles={{ body: { padding: "12px 16px" } }}
    >
      <div className="mobile-body">
        <div className="qr-box" dangerouslySetInnerHTML={{ __html: info.qrSvg }} />
        <div className="mobile-url mono">{info.url}</div>
        {info.warn && <div className="mobile-warn">{info.warn}</div>}
        {info.urls.length > 1 && (
          <div className="mobile-alt">
            <span>第一个打不开，可以试这些地址：</span>
            {info.urls.slice(1).map((u) => (
              <button key={u} className="link-btn mono" onClick={() => onCopy(u)} title="点击复制">
                {u}
              </button>
            ))}
          </div>
        )}
        <p className="mobile-tip">
          手机第一次扫码后会记住这台电脑：把上面的地址存成书签，下次直接打开，不用再扫。
          勾选会实时回到电脑并写进本机记录。收起这个面板不影响手机继续用。
        </p>
        <div className="mobile-help">
          <div className="help-title">手机打不开？按顺序检查</div>
          <ol>
            <li>手机和电脑连的是同一个 Wi-Fi，不是访客网络 / 5G</li>
            <li>
              Windows 防火墙：当前 Wi-Fi 若是「公用网络」会被拦；点下面的一键放行，
              或把这个 Wi-Fi 改成「专用网络」
            </li>
          </ol>
          <code className="mono">{FIREWALL_CMD}</code>
          <div className="mobile-firewall-actions">
            <button className="btn sm primary" onClick={() => void allowFirewall()} disabled={allowBusy}>
              {allowBusy ? "放行中…" : "一键放行（弹一次 UAC）"}
            </button>
            <button className="btn sm" onClick={copyFirewallCmd}>
              {copiedCmd ? "已复制" : "复制放行命令"}
            </button>
          </div>
          {allowMsg && <div className="mobile-warn">{allowMsg}</div>}
        </div>
        <div className="mobile-actions">
          <button className="btn" onClick={onCopyUrl}>
            复制链接
          </button>
          <button className="btn" onClick={onReset}>
            换新口令
          </button>
          <button className="btn danger" onClick={onStop}>
            停止手机端
          </button>
        </div>
      </div>
    </Drawer>
  );
}
