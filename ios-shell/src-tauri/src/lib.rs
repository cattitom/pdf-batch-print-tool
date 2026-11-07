//! 出货助手的壳：界面就是 web/ 里那个「自动发现 + iframe」的单页，
//! 没有后端逻辑，手机打开后连的是电脑上跑着的局域网服务。
//!
//! iOS 打包交给 CI（.github/workflows/ios-ipa.yml），这里只保留最小入口
//! 和 UDP 发现命令（省掉扫码/填地址）。

use std::{net::UdpSocket, time::Duration};

use serde::{Deserialize, Serialize};

#[cfg(target_os = "ios")]
mod bonjour;

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

/// 本机的 IPv4 列表（给错误提示用：一眼看出手机到底在哪个网段）
fn local_ipv4_summary() -> String {
    let mut list = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for itf in ifaces {
            if itf.is_loopback() {
                continue;
            }
            if let if_addrs::IfAddr::V4(v4) = itf.addr {
                list.push(format!("{}/{}", v4.ip, v4.prefixlen));
            }
        }
    }
    if list.is_empty() {
        "无".to_string()
    } else {
        list.join("、")
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    /// 电脑的局域网 IP（取回包的源地址，最稳）
    pub host: String,
    pub port: u16,
    pub token: String,
    /// 电脑名（电脑端 TXT/应答里带，老版本电脑没有就是空）
    #[serde(default)]
    pub name: String,
    /// 仅服务地址，授权链接必须从扫码或本机历史取得
    pub url: String,
}

#[derive(Deserialize)]
struct Reply {
    port: u16,
    #[serde(default)]
    name: Option<String>,
}

fn make_peer(reply: &Reply, from: std::net::SocketAddr) -> Option<Peer> {
    if reply.port == 0 {
        return None;
    }
    let host = from.ip().to_string();
    let base = format!("http://{host}:{}", reply.port);
    // 发现数据不能授予访问权限，凭证只从扫码后的本机历史读取。
    let token = String::new();
    Some(Peer {
        url: format!("{base}/"),
        host,
        port: reply.port,
        token,
        name: reply.name.clone().unwrap_or_default(),
    })
}

fn decode(buf: &[u8], from: std::net::SocketAddr) -> Result<Option<Peer>, String> {
    let text = String::from_utf8_lossy(buf).to_string();
    match serde_json::from_str::<Reply>(&text) {
        Ok(reply) => Ok(make_peer(&reply, from)),
        Err(e) => Err(format!("电脑的应答看不懂（{e}）：{text}")),
    }
}

/// 往局域网广播发现请求，只接收电脑地址，不接收访问凭证。
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
            Ok((n, from)) => match decode(&buf[..n], from)? {
                Some(peer) => return Ok(peer),
                None => continue,
            },
            Err(_) => continue,
        }
    }
    if sent == 0 {
        return Err(format!(
            "广播发不出去（试了 {} 个地址）：{}。本机 IP：{}",
            targets.len(),
            last_send_err.unwrap_or_else(|| "未知错误".into()),
            local_ipv4_summary()
        ));
    }
    Err(format!(
        "没找到电脑（广播了 {sent} 次，电脑没应答；本机 IP：{}）",
        local_ipv4_summary()
    ))
}

/// 直接朝指定地址发暗号（广播被系统/路由器拦掉时的兜底，走单播）。
fn discover_host(host: &str) -> Result<Peer, String> {
    let host = host.trim();
    let target: std::net::SocketAddr = if host.contains(':') {
        host.parse()
            .map_err(|_| format!("地址看不懂：{host}（该填 192.168.6.125 这种）"))?
    } else {
        format!("{host}:{DISCOVER_PORT}")
            .parse()
            .map_err(|_| format!("地址看不懂：{host}（该填 192.168.6.125 这种）"))?
    };

    let sock = UdpSocket::bind(("0.0.0.0", 0)).map_err(|e| format!("建 UDP socket 失败：{e}"))?;
    sock.set_read_timeout(Some(WAIT))
        .map_err(|e| format!("设超时失败：{e}"))?;

    let mut buf = [0u8; 1024];
    let mut last_err = String::new();
    for _ in 0..ROUNDS {
        if let Err(e) = sock.send_to(DISCOVER_MAGIC, target) {
            last_err = e.to_string();
        }
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => match decode(&buf[..n], from)? {
                Some(peer) => return Ok(peer),
                None => continue,
            },
            Err(_) => continue,
        }
    }
    Err(format!(
        "连不上 {target}：{}（电脑上的「手机出货」面板要开着）",
        if last_err.is_empty() {
            "没人应答".to_string()
        } else {
            last_err
        }
    ))
}

#[tauri::command]
async fn discover_peer() -> Result<Peer, String> {
    tauri::async_runtime::spawn_blocking(|| {
        // iOS 上广播会被系统拦，先走系统的 Bonjour（mDNSResponder 代发组播）
        #[allow(unused_mut)]
        let mut bonjour_err = String::new();
        #[cfg(target_os = "ios")]
        {
            match bonjour::discover(Duration::from_secs(3)) {
                Ok((host, port, _token, name)) => {
                    let base = format!("http://{host}:{port}");
                    return Ok(Peer {
                        url: format!("{base}/"),
                        host,
                        port,
                        token: String::new(),
                        name,
                    });
                }
                Err(e) => bonjour_err = e,
            }
        }
        match discover_blocking() {
            Ok(peer) => Ok(peer),
            Err(e) => {
                if bonjour_err.is_empty() {
                    Err(e)
                } else {
                    Err(format!("Bonjour：{bonjour_err}；广播：{e}"))
                }
            }
        }
    })
    .await
    .map_err(|e| format!("发现任务崩了：{e}"))?
}

#[tauri::command]
async fn discover_at(host: String) -> Result<Peer, String> {
    tauri::async_runtime::spawn_blocking(move || discover_host(&host))
        .await
        .map_err(|e| format!("发现任务崩了：{e}"))?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    // 扫码插件是 mobile-only 的 crate（桌面编译出来是空壳，没有 init）
    #[cfg(mobile)]
    {
        builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    }
    builder
        .invoke_handler(tauri::generate_handler![discover_peer, discover_at])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
