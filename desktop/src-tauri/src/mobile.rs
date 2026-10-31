//! 手机出货：桌面端起一个局域网 HTTP 服务，手机浏览器打开同一份表格勾选。
//!
//! 设计取舍：
//! - 只做「读当前表 + 改已录」，解析/OCR 仍在桌面端，手机端不重写业务。
//! - 数据只在内存里，桌面端关闭即失效；勾选由桌面端轮询后落库（沿用 set_packing_check）。
//! - token 放在 URL 查询参数里，二维码直接带 token，局域网内不裸奔。

use std::{
    net::IpAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const PAGE: &str = include_str!("../mobile/index.html");
const CSS: &str = include_str!("../mobile/m.css");
const JS: &str = include_str!("../mobile/m.js");

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MobileRow {
    pub waybill: String,
    pub inbound: String,
    pub name: String,
    pub customer: String,
    pub qty: i64,
    #[serde(default)]
    pub checked: bool,
    #[serde(default)]
    pub issues: Vec<String>,
    /// 既没入库号也没单号：不能标已出，但照常算未出、照常下发。
    #[serde(default)]
    pub blocked: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileInfo {
    pub url: String,
    /// 所有可试的地址：第一个进二维码，其它列在弹窗里备选
    pub urls: Vec<String>,
    pub token: String,
    pub port: u16,
    pub qr_svg: String,
    /// 没找到局域网地址时的提示
    pub warn: Option<String>,
}

struct Inner {
    info: Option<MobileInfo>,
    rows: Vec<MobileRow>,
    stop: Option<Arc<AtomicBool>>,
}

pub struct Shared {
    inner: Mutex<Inner>,
}

pub type MobileShared = Arc<Shared>;

pub fn new_shared() -> MobileShared {
    Arc::new(Shared {
        inner: Mutex::new(Inner {
            info: None,
            rows: Vec::new(),
            stop: None,
        }),
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 生成一个新口令（也用于首次启动时写进本机设置）。
pub fn new_token() -> String {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:08x}", ((t as u64) ^ ((std::process::id() as u64) << 32)) as u32)
}

/// 从网卡列表里挑出手机能访问的地址：跳过 VPN / 虚拟网卡和 169.254，私有网段优先。
///
/// 背景：本机装了 Mihomo（TUN 模式），默认路由被它接管，靠「连 8.8.8.8 看源地址」
/// 会拿到 198.18.0.1 这种只有本机能用的地址。所以改成枚举网卡再过滤。
fn pick_ips(list: Vec<(String, IpAddr)>) -> Vec<String> {
    const SKIP: [&str; 15] = [
        "vmware",
        "virtualbox",
        "vethernet",
        "hyper-v",
        "mihomo",
        "clash",
        "sing-box",
        "tailscale",
        "zerotier",
        "wintun",
        "tap",
        "tun",
        "loopback",
        "docker",
        "wsl",
    ];
    let mut out: Vec<(u8, String)> = Vec::new();
    for (name, ip) in list {
        let IpAddr::V4(v4) = ip else { continue };
        if v4.is_loopback() || v4.is_unspecified() || v4.is_link_local() {
            continue;
        }
        let lower = name.to_lowercase();
        if SKIP.iter().any(|k| lower.contains(k)) {
            continue;
        }
        let rank = match v4.octets() {
            [192, 168, ..] => 0,
            [10, ..] => 1,
            [172, b, ..] if (16..=31).contains(&b) => 2,
            _ if v4.is_private() => 3,
            _ => 4,
        };
        let s = v4.to_string();
        if !out.iter().any(|(_, seen)| *seen == s) {
            out.push((rank, s));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    out.into_iter().map(|(_, s)| s).collect()
}

fn lan_ips() -> Vec<String> {
    match local_ip_address::list_afinet_netifas() {
        Ok(list) => pick_ips(list),
        Err(_) => Vec::new(),
    }
}

fn build_info(port: u16, token: String) -> MobileInfo {
    let ips = lan_ips();
    let fallback = format!("127.0.0.1:{port}/?t={token}");
    let urls: Vec<String> = if ips.is_empty() {
        vec![format!("http://{fallback}")]
    } else {
        ips.iter()
            .map(|ip| format!("http://{ip}:{port}/?t={token}"))
            .collect()
    };
    let warn = ips
        .is_empty()
        .then(|| "没找到局域网地址：确认电脑连着 Wi-Fi / 网线，再重新打开这个窗口".to_string());
    let primary = urls[0].clone();
    MobileInfo {
        url: primary.clone(),
        urls,
        token,
        port,
        qr_svg: qr_svg(&primary),
        warn,
    }
}

fn bind_server() -> Result<(Server, u16), String> {
    // 先试固定端口，方便手机记住地址；占用就退回系统分配
    for port in [17650u16, 17651, 17652] {
        if let Ok(server) = Server::http(("0.0.0.0", port)) {
            return Ok((server, port));
        }
    }
    let server = Server::http("0.0.0.0:0").map_err(|e| format!("手机服务启动失败：{e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .ok_or_else(|| "拿不到监听端口".to_string())?;
    Ok((server, port))
}

fn qr_svg(url: &str) -> String {
    match qrcode::QrCode::new(url.as_bytes()) {
        Ok(code) => code
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(220, 220)
            .quiet_zone(true)
            .build(),
        Err(_) => String::new(),
    }
}

pub fn start(shared: &MobileShared, rows: Vec<MobileRow>, token: String) -> Result<MobileInfo, String> {
    let mut inner = shared.inner.lock().map_err(|e| e.to_string())?;
    if let Some(old) = inner.info.clone() {
        inner.rows = rows;
        // 换过 Wi-Fi / 开了 VPN 之后地址会变，重开弹窗时重新挑一次
        let info = build_info(old.port, old.token);
        inner.info = Some(info.clone());
        return Ok(info);
    }

    let (server, port) = bind_server()?;
    let info = build_info(port, token);

    inner.rows = rows;
    let stop = Arc::new(AtomicBool::new(false));
    inner.stop = Some(stop.clone());
    inner.info = Some(info.clone());

    let shared_for_thread = shared.clone();
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            match server.recv_timeout(Duration::from_millis(300)) {
                Ok(Some(req)) => handle(req, &shared_for_thread),
                Ok(None) => {}
                Err(_) => break,
            }
        }
    });

    log::info!("手机出货服务已启动：{}", info.url);
    Ok(info)
}

pub fn sync(shared: &MobileShared, mut rows: Vec<MobileRow>) {
    if let Ok(mut inner) = shared.inner.lock() {
        if inner.info.is_none() {
            return;
        }
        // 勾选以手机端为准：电脑勾变更走 set_check，这里只换结构（换表、改名字/数量）
        for row in rows.iter_mut() {
            if let Some(old) = inner.rows.iter().find(|o| {
                o.waybill == row.waybill && o.inbound == row.inbound && o.name == row.name
            }) {
                row.checked = old.checked;
            }
        }
        inner.rows = rows;
    }
}

/// 电脑端勾选后同步给手机（避免整表 sync 覆盖手机刚勾的状态）。
pub fn set_check(shared: &MobileShared, waybill: &str, inbound: &str, name: &str, checked: bool) {
    toggle(shared, waybill, inbound, name, checked);
}

pub fn rows(shared: &MobileShared) -> Vec<MobileRow> {
    shared
        .inner
        .lock()
        .map(|inner| inner.rows.clone())
        .unwrap_or_default()
}

pub fn info(shared: &MobileShared) -> Option<MobileInfo> {
    shared.inner.lock().ok().and_then(|i| i.info.clone())
}

pub fn stop(shared: &MobileShared) {
    if let Ok(mut inner) = shared.inner.lock() {
        if let Some(flag) = inner.stop.take() {
            flag.store(true, Ordering::Relaxed);
        }
        inner.info = None;
        inner.rows.clear();
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

fn respond(req: Request, body: String, ctype: &str, status: u16) {
    let resp = Response::from_string(body)
        .with_status_code(StatusCode(status))
        .with_header(header("Content-Type", ctype))
        .with_header(header("Cache-Control", "no-store, max-age=0"));
    let _ = req.respond(resp);
}

fn split_url(url: &str) -> (&str, &str) {
    match url.split_once('?') {
        Some((p, q)) => (p, q),
        None => (url, ""),
    }
}

fn query_token(query: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == "t").then(|| v.to_string())
    })
}

fn current_token(shared: &MobileShared) -> Option<String> {
    info(shared).map(|i| i.token)
}

fn cookie_token(req: &Request) -> Option<String> {
    for h in req.headers() {
        if h.field.equiv("Cookie") {
            for part in h.value.as_str().split(';') {
                if let Some(v) = part.trim().strip_prefix("mt=") {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

fn handle(mut req: Request, shared: &MobileShared) {
    let (path, query) = split_url(req.url());
    let token = current_token(shared);
    let query_tok = query_token(query);
    let cookie_tok = cookie_token(&req);
    let authed = match &token {
        Some(have) => {
            query_tok.as_deref() == Some(have.as_str()) || cookie_tok.as_deref() == Some(have.as_str())
        }
        None => false,
    };
    // 用带口令的链接打开一次，就让浏览器记住：之后手机开书签里的地址也能进
    let set_cookie = match (&token, &query_tok) {
        (Some(have), Some(give)) if have == give => Some(format!(
            "mt={have}; Max-Age=31536000; Path=/; SameSite=Lax; HttpOnly"
        )),
        _ => None,
    };

    match (req.method().clone(), path) {
        (Method::Get, "/") | (Method::Get, "/index.html") => {
            let mut resp = Response::from_string(PAGE)
                .with_status_code(StatusCode(200))
                .with_header(header("Content-Type", "text/html; charset=utf-8"))
                .with_header(header("Cache-Control", "no-store, max-age=0"));
            if let Some(c) = &set_cookie {
                resp = resp.with_header(header("Set-Cookie", c));
            }
            let _ = req.respond(resp);
        }
        (Method::Get, "/m.css") => respond(req, CSS.to_string(), "text/css; charset=utf-8", 200),
        (Method::Get, "/m.js") => {
            respond(req, JS.to_string(), "application/javascript; charset=utf-8", 200)
        }
        (Method::Get, "/favicon.ico") => respond(req, String::new(), "image/x-icon", 204),
        _ if !authed => respond(
            req,
            json!({"error": "链接里的口令不对，回电脑上重新扫码"}).to_string(),
            "application/json; charset=utf-8",
            403,
        ),
        (Method::Get, "/api/sheet") => {
            let (rows, updated) = previous(shared);
            respond(
                req,
                json!({"rows": rows, "updatedAt": updated}).to_string(),
                "application/json; charset=utf-8",
                200,
            )
        }
        (Method::Post, "/api/check") => {
            let mut body = String::new();
            if req.as_reader().read_to_string(&mut body).is_err() {
                respond(req, json!({"error": "读不到请求体"}).to_string(), "application/json", 400);
                return;
            }
            let v: serde_json::Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(_) => {
                    respond(req, json!({"error": "请求体不是 JSON"}).to_string(), "application/json", 400);
                    return;
                }
            };
            let waybill = v["waybill"].as_str().unwrap_or("").to_string();
            let inbound = v["inbound"].as_str().unwrap_or("").to_string();
            let name = v["name"].as_str().unwrap_or("").to_string();
            let checked = v["checked"].as_bool().unwrap_or(false);
            let ok = toggle(shared, &waybill, &inbound, &name, checked);
            if ok {
                respond(req, json!({"ok": true}).to_string(), "application/json; charset=utf-8", 200)
            } else {
                respond(req, json!({"error": "没找到这一行"}).to_string(), "application/json; charset=utf-8", 404)
            }
        }
        (Method::Get, "/api/ping") => respond(
            req,
            json!({"ok": true}).to_string(),
            "application/json; charset=utf-8",
            200,
        ),
        _ => respond(
            req,
            json!({"error": "not found"}).to_string(),
            "application/json; charset=utf-8",
            404,
        ),
    }
}

fn previous(shared: &MobileShared) -> (Vec<MobileRow>, u64) {
    match shared.inner.lock() {
        Ok(inner) => (inner.rows.clone(), now_secs()),
        Err(_) => (Vec::new(), 0),
    }
}

fn toggle(shared: &MobileShared, waybill: &str, inbound: &str, name: &str, checked: bool) -> bool {
    let Ok(mut inner) = shared.inner.lock() else {
        return false;
    };
    let mut hit = false;
    for row in inner.rows.iter_mut() {
        if row.waybill == waybill && row.inbound == inbound && row.name == name {
            // 没入库号也没单号的货不能出货：手机点上去也不认
            if checked && row.blocked {
                return false;
            }
            row.checked = checked;
            hit = true;
        }
    }
    hit
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::TcpStream;

    #[test]
    fn token_roundtrip() {
        let query = "t=abc&x=1";
        assert_eq!(query_token(query).as_deref(), Some("abc"));
        assert_eq!(query_token("x=1"), None);
    }

    #[test]
    fn toggle_updates_row() {
        let shared = new_shared();
        {
            let mut inner = shared.inner.lock().unwrap();
            inner.info = Some(MobileInfo {
                url: "http://x".into(),
                urls: vec!["http://x".into()],
                token: "t".into(),
                port: 1,
                qr_svg: String::new(),
                warn: None,
            });
            inner.rows = vec![MobileRow {
                waybill: "S1".into(),
                inbound: "I1".into(),
                name: "货".into(),
                customer: "王".into(),
                qty: 2,
                checked: false,
                issues: vec![],
                blocked: false,
            }];
        }
        assert!(toggle(&shared, "S1", "I1", "货", true));
        assert!(rows(&shared)[0].checked);
        assert!(!toggle(&shared, "S9", "I9", "没有", true));
    }

    #[test]
    fn blocked_row_cannot_be_ticked() {
        // 没入库号也没单号的货：手机上点也认不了，勾不动
        let shared = new_shared();
        {
            let mut inner = shared.inner.lock().unwrap();
            inner.info = Some(MobileInfo {
                url: "http://x".into(),
                urls: vec!["http://x".into()],
                token: "t".into(),
                port: 1,
                qr_svg: String::new(),
                warn: None,
            });
            inner.rows = vec![MobileRow {
                waybill: String::new(),
                inbound: String::new(),
                name: "没有箱唛".into(),
                customer: "王".into(),
                qty: 5,
                checked: false,
                issues: vec![],
                blocked: true,
            }];
        }
        assert!(!toggle(&shared, "", "", "没有箱唛", true));
        assert!(!rows(&shared)[0].checked);
        // 取消（改回未出）不受影响
        assert!(toggle(&shared, "", "", "没有箱唛", false));
    }

    fn http(host: &str, req: &str) -> String {
        let mut s = TcpStream::connect(host).expect("connect");
        s.write_all(req.as_bytes()).expect("write");
        let mut out = String::new();
        s.read_to_string(&mut out).expect("read");
        out
    }

    #[test]
    fn server_serves_sheet_and_toggles() {
        let shared = new_shared();
        let seed = vec![MobileRow {
            waybill: "S1".into(),
            inbound: "I1".into(),
            name: "测试货".into(),
            customer: "王".into(),
            qty: 3,
            checked: false,
            issues: vec![],
            blocked: false,
        }];
        let info = start(&shared, seed, "testtoken".into()).expect("start server");
        let host = format!("127.0.0.1:{}", info.port);

        let page = http(&host, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert!(page.contains("200 OK"));
        assert!(page.contains("出货清单"));

        let sheet = http(
            &host,
            &format!("GET /api/sheet?t={} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n", info.token),
        );
        assert!(sheet.contains("测试货"));
        assert!(sheet.contains("\"rows\""));

        let denied = http(&host, "GET /api/sheet?t=bad HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert!(denied.contains("403"));

        // 带口令打开页面会给浏览器种 cookie；之后不带 token 也能进
        let page = http(
            &host,
            "GET /?t=testtoken HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        );
        assert!(page.contains("Set-Cookie: mt=testtoken"));
        let via_cookie = http(
            &host,
            "GET /api/sheet HTTP/1.1\r\nHost: x\r\nCookie: mt=testtoken\r\nConnection: close\r\n\r\n",
        );
        assert!(via_cookie.contains("测试货"));

        let body = r#"{"waybill":"S1","inbound":"I1","name":"测试货","checked":true}"#;
        let post = format!(
            "POST /api/check?t={} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            info.token,
            body.len(),
            body
        );
        let res = http(&host, &post);
        assert!(res.contains("200 OK"));
        assert!(rows(&shared)[0].checked);

        stop(&shared);
        // 停掉后端口应该很快释放
        std::thread::sleep(Duration::from_millis(500));
    }

    #[test]
    fn picks_lan_ip_over_vpn() {
        let list = vec![
            ("Mihomo".to_string(), "198.18.0.1".parse().unwrap()),
            (
                "VMware Network Adapter VMnet8".to_string(),
                "169.254.150.209".parse().unwrap(),
            ),
            ("WLAN".to_string(), "192.168.6.125".parse().unwrap()),
            (
                "vEthernet (Default Switch)".to_string(),
                "172.20.0.1".parse().unwrap(),
            ),
            ("Ethernet".to_string(), "10.1.2.3".parse().unwrap()),
        ];
        assert_eq!(pick_ips(list), vec!["192.168.6.125", "10.1.2.3"]);
    }

    #[test]
    #[ignore]
    fn shows_real_ips() {
        println!("lan_ips = {:?}", lan_ips());
    }
}



