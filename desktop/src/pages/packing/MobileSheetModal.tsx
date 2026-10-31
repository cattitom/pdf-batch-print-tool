import { useEffect, useRef, useState } from "react";
import type { MobileInfoDto } from "../../api";

/** 防火墙放行命令：手机连不上时复制到管理员 PowerShell 跑一次 */
const FIREWALL_CMD =
  'netsh advfirewall firewall add rule name="PdfMobileReceipt" dir=in action=allow protocol=TCP localport=17650-17652';

type Props = {
  info: MobileInfoDto;
  /** 复制任意文本（页面那边 Clipboard 有回退实现） */
  onCopy: (text: string) => void;
  onCopyUrl: () => void;
  /** 换新口令 */
  onReset: () => void;
  /** 关窗口 + 停手机端（点遮罩也是这个） */
  onClose: () => void;
};

/** 手机出货弹窗：二维码、备用地址、防火墙命令、停止手机端。 */
export function MobileSheetModal({ info, onCopy, onCopyUrl, onReset, onClose }: Props) {
  const [copiedCmd, setCopiedCmd] = useState(false);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const copyFirewallCmd = async () => {
    await onCopy(FIREWALL_CMD);
    setCopiedCmd(true);
    timer.current = window.setTimeout(() => setCopiedCmd(false), 2000);
  };

  return (
    <div className="modal-mask" onClick={onClose}>
      <div className="modal mobile-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <span>手机出货</span>
          <button className="btn sm" onClick={onClose}>
            关闭
          </button>
        </div>
        <div className="modal-body mobile-body">
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
            勾选会实时回到电脑并写进本机记录。
          </p>
          <div className="mobile-help">
            <div className="help-title">手机打不开？按顺序检查</div>
            <ol>
              <li>手机和电脑连的是同一个 Wi-Fi，不是访客网络 / 5G</li>
              <li>
                Windows 防火墙：当前 Wi-Fi 若是「公用网络」会被拦；以管理员身份运行下面命令，
                或把这个 Wi-Fi 改成「专用网络」
              </li>
            </ol>
            <code className="mono">{FIREWALL_CMD}</code>
            <button className="btn sm" onClick={copyFirewallCmd}>
              {copiedCmd ? "已复制" : "复制放行命令"}
            </button>
          </div>
          <div className="mobile-actions">
            <button className="btn" onClick={onCopyUrl}>
              复制链接
            </button>
            <button className="btn" onClick={onReset}>
              换新口令
            </button>
            <button className="btn danger" onClick={onClose}>
              停止手机端
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
