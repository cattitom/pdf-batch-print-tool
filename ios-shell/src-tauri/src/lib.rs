//! 出货助手的壳：界面就是 web/ 里那个「自动发现 + iframe」的单页，
//! 没有后端逻辑，手机打开后连的是电脑上跑着的局域网服务。
//!
//! iOS 打包交给 CI（.github/workflows/ios-ipa.yml），这里只保留最小入口
//! 和一条 UDP 发现命令（省掉扫码/填地址）。

use std::{net::UdpSocket, time::Duration};

use serde::{Deserialize, Serialize};

/// 和电脑端 `mobile::DISCOVER_PORT` 保持一致
const DISCOVER_PORT: u16 = 17655;
const DISCOVER_MAGIC: &[u8] = b"PDF-MOBILE-DISCOVER";
/// 广播几轮、每轮等多久：总计约 3 秒，够慢 WiFi 应答了
const ROUNDS: usize = 6;
const WAIT: Duration = Duration::from_millis(500);

/// 手机常常同时开着蜂窝和 Wi-Fi：iOS 上直接往 255.255.255.255 发，经常因为找不到路由
/// 直接报 `No route to host (os error 65)`。所以按每张网卡算出子网定向广播
/// （比如 192.168.6.255）一起发，比只发受限广播靠谱得多。
fn broadcast_targets() -> Vec<std::net::SocketAddr> {
    let mut targets: Vec<std::net::SocketAddr> =
        vec![([255, 255, 255, 255], DISCOVER_PORT).into()];

    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for itf in ifaces {
            // 回环、蜂窝（pdp_ip*）上没有局域网广播
            if itf.is_loopback() || itf.name.starts_with("pdp_ip") {
                continue;
            }
            if let if_addrs::IfAddr::V4(v4) = itf.addr {
                let bcast = match v4.broadcast {
                    Some(b) => b,
                    None => {
                        let mask = u32::from(v4.netmask);
                        if mask == 0 {
                            continue;
                        }
                        std::net::Ipv4Addr::from(u32::from(v4.ip) | !mask)
                    }
                };
                if bcast == v4.ip || bcast.is_broadcast() {
                    continue;
                }
                let addr = std::net::SocketAddr::from((bcast, DISCOVER_PORT));
                if !targets.contains(&addr) {
                    targets.push(addr);
                }
            }
        }
    }
    targets
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    /// 电脑的局域网 IP（取回包的源地址，最稳）
    pub host: String,
    pub port: u16,
    pub token: String,
    /// 已经拼好口令，前端直接塞 iframe 就行
    pub url: String,
}

#[derive(Deserialize)]
struct Reply {
    port: u16,
    token: String,
}

/// 往局域网广播暗号，等电脑回 {port, token}。找不到就报错，前端显示「检查是否同一 WiFi」。
fn discover_blocking() -> Result<Peer, String> {
    let sock = UdpSocket::bind(("0.0.0.0", 0)).map_err(|e| format!("建 UDP socket 失败：{e}"))?;
    sock.set_broadcast(true)
        .map_err(|e| format!("开广播失败：{e}"))?;
    sock.set_read_timeout(Some(WAIT))
        .map_err(|e| format!("设超时失败：{e}"))?;

    let targets = broadcast_targets();
    let mut buf = [0u8; 1024];
    let mut sent = 0usize;
    let mut last_send_err: Option<String> = None;
    for _ in 0..ROUNDS {
        // 广播可能丢包，每轮重发一次。
        // 发不出去必须记下来：iOS 没给「本地网络」权限、或者受限广播没路由时，
        // send_to 会直接报错，以前用 let _ = 吞掉了，界面只能显示含糊的「没找到电脑」。
        for target in &targets {
            match sock.send_to(DISCOVER_MAGIC, target) {
                Ok(_) => sent += 1,
                Err(e) => last_send_err = Some(format!("{target}: {e}")),
            }
        }
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                let reply: Reply = serde_json::from_str(&text)
                    .map_err(|e| format!("电脑的应答看不懂（{e}）：{text}"))?;
                if reply.port == 0 || reply.token.is_empty() {
                    continue;
                }
                let host = from.ip().to_string();
                let url = format!("http://{host}:{}", reply.port);
                return Ok(Peer {
                    url: format!("{url}/?t={}", reply.token),
                    host,
                    port: reply.port,
                    token: reply.token,
                });
            }
            Err(_) => continue,
        }
    }
    if sent == 0 {
        return Err(format!(
            "广播发不出去（试了 {} 个地址）：{}。iOS 上多半是「本地网络」权限被拒，去 设置 → 出货助手 → 本地网络 打开后重试",
            targets.len(),
            last_send_err.unwrap_or_else(|| "未知错误".into())
        ));
    }
    Err(format!(
        "没找到电脑（广播了 {sent} 次，电脑没应答；确认手机和电脑同一个 WiFi）"
    ))
}

#[tauri::command]
async fn discover_peer() -> Result<Peer, String> {
    tauri::async_runtime::spawn_blocking(discover_blocking)
        .await
        .map_err(|e| format!("发现任务崩了：{e}"))?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![discover_peer])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
