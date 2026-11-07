//! 手机出货：桌面端起一个局域网 HTTP 服务，手机浏览器打开同一份表格勾选。
//!
//! 设计取舍：
//! - 只做「读当前表 + 改已录」，解析/OCR 仍在桌面端，手机端不重写业务。
//! - 勾选事务提交数据库后再返回成功；数据集和记录版本阻止旧请求覆盖。
//! - token 放在 URL 查询参数里，二维码直接带 token，局域网内不裸奔。

use std::{
    net::{IpAddr, UdpSocket},
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

/// 手机 App 自动发现用的 UDP 端口（TCP 是 17650-17652）。
pub const DISCOVER_PORT: u16 = 17655;
/// App 广播的固定暗号，收到别的 UDP 包一律不理。
const DISCOVER_MAGIC: &[u8] = b"PDF-MOBILE-DISCOVER";

/// mDNS 服务类型（和手机端 ios-shell 的 bonjour.rs 的 REGTYPE 一致）。
/// iOS 上系统不允许 App 自己发广播/组播，但 Bonjour 由 mDNSResponder 代发，能用。
const MDNS_SERVICE_TYPE: &str = "_shippingshell._tcp.local.";

/// 当前注册的 mDNS 服务：(守护进程, 服务全名)
static MDNS: Mutex<Option<(mdns_sd::ServiceDaemon, String)>> = Mutex::new(None);

const PAGE: &str = include_str!("../mobile/index.html");
const CSS: &str = include_str!("../mobile/m.css");
const JS: &str = include_str!("../mobile/m.js");

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct MobileRow {
    #[serde(default)]
    pub source_id: String,
    #[serde(default)]
    pub record_id: String,
    #[serde(default)]
    pub dataset_id: String,
    #[serde(default)]
    pub version: u64,
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
    /// 本次出货任务（桌面从截图导入的那批），手机端可以先只看这些。
    #[serde(default)]
    pub task: bool,
    /// 任务号（按单号分组，T1/T2…），手机端显示在票卡片上
    #[serde(default)]
    pub task_id: String,
    /// 这张票被谁领了（空 = 没人领）
    #[serde(default)]
    pub claimed_by: String,
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
    /// 认领：单号 → 谁领的（手机点「领取任务」写进来）
    claims: std::collections::HashMap<String, String>,
    stop: Option<Arc<AtomicBool>>,
}

pub struct Shared {
    inner: Mutex<Inner>,
    db: Arc<Mutex<pdfimpose_core::Db>>,
}

pub type MobileShared = Arc<Shared>;

#[cfg(test)]
pub fn new_shared() -> MobileShared {
    with_db(Arc::new(Mutex::new(pdfimpose_core::Db::open_in_memory().expect("测试数据库"))))
}

pub fn with_db(db: Arc<Mutex<pdfimpose_core::Db>>) -> MobileShared {
    Arc::new(Shared {
        db,
        inner: Mutex::new(Inner {
            info: None,
            rows: Vec::new(),
            claims: std::collections::HashMap::new(),
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
pub fn new_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("生成手机配对口令失败：{e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
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

fn bind_server(prefer: Option<u16>) -> Result<(Server, u16), String> {
    // 先把上次成功的端口放最前面：手机书签记的是完整 URL，端口一变书签就废了
    let mut order: Vec<u16> = Vec::with_capacity(4);
    if let Some(p) = prefer.filter(|p| *p > 1024) {
        order.push(p);
    }
    for p in [17650u16, 17651, 17652] {
        if !order.contains(&p) {
            order.push(p);
        }
    }
    for port in order {
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

/// 发现协议只公布地址，访问凭证只通过电脑上的二维码或已保存链接取得。
fn discovery_payload(info: &MobileInfo) -> serde_json::Value {
    json!({ "app": "pdf-mobile", "name": display_name(), "port": info.port,
        "urls": info.urls.iter().map(|url| url.split('?').next().unwrap_or(url)).collect::<Vec<_>>() })
}

/// UDP 发现仅返回地址；端口被占不影响 HTTP 和扫码配对。
fn start_discovery(shared: &MobileShared, stop: Arc<AtomicBool>) {
    let sock = match UdpSocket::bind(("0.0.0.0", DISCOVER_PORT)) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("UDP 发现端口 {DISCOVER_PORT} 绑定失败：{e}，App 自动发现不可用");
            return;
        }
    };
    // 收包用超时轮询，好让 stop 生效；不设超时会卡在 recv_from 上退不出去
    let _ = sock.set_read_timeout(Some(Duration::from_millis(300)));
    let _ = sock.set_broadcast(true);

    let shared = shared.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 256];
        while !stop.load(Ordering::Relaxed) {
            let (n, from) = match sock.recv_from(&mut buf) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if &buf[..n] != DISCOVER_MAGIC {
                continue;
            }
            // 口令/端口每次现取：重开服务换了端口也能立刻回报
            let info = shared.inner.lock().ok().and_then(|i| i.info.clone());
            if let Some(info) = info {
                let payload = discovery_payload(&info).to_string();
                let _ = sock.send_to(payload.as_bytes(), from);
            }
        }
    });
}

/// 仅广播手机服务地址和电脑名称，首次连接必须扫描电脑上的二维码。
/// 每次开服务都重新注册（换 Wi-Fi 后 IP 会变），旧的先注销。
fn start_mdns(info: &MobileInfo) {
    let ip = info
        .urls
        .first()
        .and_then(|u| u.strip_prefix("http://"))
        .and_then(|rest| rest.split(':').next())
        .unwrap_or("127.0.0.1")
        .to_string();
    let host = mdns_host_name();
    let display = display_name();

    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            log::warn!("mDNS 启动失败：{e}");
            return;
        }
    };
    let service = match mdns_sd::ServiceInfo::new(
        MDNS_SERVICE_TYPE,
        "shipping-helper",
        &host,
        ip.as_str(),
        info.port,
        &[("name", display.as_str())][..],
    ) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("mDNS 服务信息构造失败：{e}");
            return;
        }
    };
    let fullname = service.get_fullname().to_string();
    if let Err(e) = daemon.register(service) {
        log::warn!("mDNS 注册失败：{e}");
        return;
    }
    log::info!("mDNS 已广播：{fullname}（{ip}:{}）", info.port);

    if let Ok(mut slot) = MDNS.lock() {
        if let Some((old, _)) = slot.take() {
            let _ = old.shutdown();
        }
        *slot = Some((daemon, fullname));
    }
}

fn stop_mdns() {
    if let Ok(mut slot) = MDNS.lock() {
        if let Some((daemon, fullname)) = slot.take() {
            let _ = daemon.unregister(&fullname);
            let _ = daemon.shutdown();
        }
    }
}

/// 给人看的电脑名：手机 App 的「发现电脑」卡片用它，别只显示 IP
fn display_name() -> String {
    let name = std::env::var("COMPUTERNAME").unwrap_or_default();
    let name = name.trim();
    if name.is_empty() {
        "电脑".to_string()
    } else {
        name.to_string()
    }
}

/// mDNS 要求合法主机名：机器名里非 ASCII/连字符的字符去掉，兜底一个固定名
fn mdns_host_name() -> String {
    let label = std::env::var("COMPUTERNAME")
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_lowercase();
    let label = if label.is_empty() {
        "shipping-helper".to_string()
    } else {
        label
    };
    format!("{label}.local.")
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

pub fn start(
    shared: &MobileShared,
    mut rows: Vec<MobileRow>,
    token: String,
    prefer_port: Option<u16>,
) -> Result<MobileInfo, String> {
    let mut inner = shared.inner.lock().map_err(|e| e.to_string())?;
    prepare_rows(&mut rows, &inner.rows);
    if let Some(old) = inner.info.clone() {
        inner.rows = rows;
        // 换过 Wi-Fi / 开了 VPN 之后地址会变，重开弹窗时重新挑一次
        let info = build_info(old.port, old.token);
        inner.info = Some(info.clone());
        start_mdns(&info);
        return Ok(info);
    }

    let (server, port) = bind_server(prefer_port)?;
    let info = build_info(port, token);

    inner.rows = rows;
    let stop = Arc::new(AtomicBool::new(false));
    inner.stop = Some(stop.clone());
    inner.info = Some(info.clone());

    start_discovery(shared, stop.clone());
    start_mdns(&info);

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

    log::info!("手机出货服务已启动，端口 {}", info.port);
    Ok(info)
}

fn record_id(waybill: &str, inbound: &str, name: &str) -> String {
    serde_json::to_string(&(waybill.trim(), inbound.trim(), name.trim())).expect("字符串序列化")
}

fn prepare_rows(rows: &mut [MobileRow], old: &[MobileRow]) {
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let same = rows.len() == old.len() && rows.iter().zip(old).all(|(a,b)|
        record_id(&a.waybill,&a.inbound,&a.name) == record_id(&b.waybill,&b.inbound,&b.name)
        && a.qty == b.qty && a.issues == b.issues && a.blocked == b.blocked && a.source_id == b.source_id);
    let dataset = if same && !old.is_empty() { old[0].dataset_id.clone() } else {
        format!("{}-{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos(),
            GENERATION.fetch_add(1, Ordering::Relaxed))
    };
    for row in rows {
        row.record_id = record_id(&row.waybill,&row.inbound,&row.name);
        row.dataset_id = dataset.clone();
        row.version = 0;
        if let Some(previous) = old.iter().find(|r| r.record_id == row.record_id) {
            row.checked = previous.checked;
            if same { row.version = previous.version; }
        }
    }
}

/// 全部调用按“手机状态锁 → 数据库锁”排序，提交成功后才修改内存。
/// expected 来自客户端看到的版本；服务运行时缺失版本也拒绝写入。
pub fn commit_checks(
    shared: &MobileShared,
    changes: &[(String, String, String, bool)],
    expected: Option<&[MobileRow]>,
) -> Result<(), (u16, String)> {
    let mut inner = shared.inner.lock().map_err(|_| (500,"手机状态锁异常".into()))?;
    let mut indices = Vec::new();
    for (waybill,inbound,name,checked) in changes {
        let id = record_id(waybill,inbound,name);
        let index = inner.rows.iter().position(|r| record_id(&r.waybill,&r.inbound,&r.name) == id);
        // 例外只认桌面已保存的明确标记，不信任手机请求携带的字段。
        if *checked && (waybill.trim().is_empty() || inbound.trim().is_empty()) {
            let raw = shared.db.lock().map_err(|_| (500,"数据库锁异常".to_string()))?
                .get_setting("packing_special_shipments").map_err(|e| (500,e.to_string()))?;
            let special = raw.and_then(|s| serde_json::from_str::<std::collections::HashMap<String,String>>(&s).ok())
                .and_then(|m| m.get(&id).cloned());
            if special.as_deref() != Some("亚马逊") {
                return Err((400,"入库号和单号必须都填写，才能出货".into()));
            }
        }
        if inner.info.is_some() {
            let row = index.map(|i| &inner.rows[i]).ok_or((409,"表格已改变，请刷新后重试".to_string()))?;
            let expected = expected.and_then(|v| v.iter().find(|r| r.record_id == id))
                .ok_or((409,"缺少记录版本，请刷新后重试".to_string()))?;
            if expected.dataset_id != row.dataset_id || expected.version != row.version {
                return Err((409,"记录已被其他设备修改或表格已更新，请刷新核对后重试".into()));
            }
            if *checked && row.blocked { return Err((400,"该行不能出货".into())); }
        }
        indices.push(index);
    }
    shared.db.lock().map_err(|_| (500,"数据库锁异常".into()))?
        .set_packing_checks(changes).map_err(|e| (500,format!("出货保存失败：{e}")))?;
    for (index, (_,_,_,checked)) in indices.into_iter().zip(changes) {
        if let Some(index) = index {
            let id = inner.rows[index].record_id.clone();
            for row in inner.rows.iter_mut().filter(|r| r.record_id == id) {
                row.checked = *checked;
                row.version += 1;
            }
        }
    }
    Ok(())
}

pub fn sync(shared: &MobileShared, mut rows: Vec<MobileRow>) {
    if let Ok(mut inner) = shared.inner.lock() {
        if inner.info.is_none() {
            return;
        }
        prepare_rows(&mut rows, &inner.rows);
        inner.rows = rows;
    }
}

pub fn rows(shared: &MobileShared) -> Vec<MobileRow> {
    match shared.inner.lock() {
        Ok(inner) => {
            let mut rows = inner.rows.clone();
            for row in rows.iter_mut() {
                row.claimed_by = inner.claims.get(&row.waybill).cloned().unwrap_or_default();
            }
            rows
        }
        Err(_) => Vec::new(),
    }
}

pub fn info(shared: &MobileShared) -> Option<MobileInfo> {
    shared.inner.lock().ok().and_then(|i| i.info.clone())
}

pub fn stop(shared: &MobileShared) {
    stop_mdns();
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
        .with_header(header("Cache-Control", "no-store, max-age=0"))
        // iOS 外壳（tauri:// 本地页）要跨域拉清单、回传勾选，浏览器端没有这行会被拦
        .with_header(header("Access-Control-Allow-Origin", "*"));
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

    // 跨域预检：外壳的离线清单会带着 Content-Type: application/json 发 POST
    if *req.method() == Method::Options {
        let resp = Response::empty(StatusCode(204))
            .with_header(header("Access-Control-Allow-Origin", "*"))
            .with_header(header("Access-Control-Allow-Methods", "GET, POST, OPTIONS"))
            .with_header(header("Access-Control-Allow-Headers", "Content-Type"))
            .with_header(header("Access-Control-Max-Age", "86400"));
        let _ = req.respond(resp);
        return;
    }
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
        (Method::Get, "/api/app-update/manifest") | (Method::Get, "/api/app-update/bundle") => {
            let version = query.split('&').find_map(|kv| kv.strip_prefix("v="));
            match crate::mobile_update::response(version, path.ends_with("/manifest")) {
                Ok(body) => respond(req, body, "application/json; charset=utf-8", 200),
                Err((status, error)) => respond(req, json!({"error":error}).to_string(), "application/json; charset=utf-8", status),
            }
        }
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
            let row: MobileRow = match serde_json::from_value(v) {
                Ok(row) => row,
                Err(_) => { respond(req, json!({"error":"请求字段不完整，请刷新页面"}).to_string(), "application/json", 400); return; }
            };
            let changes = vec![(row.waybill.trim().into(), row.inbound.trim().into(), row.name.trim().into(), row.checked)];
            match commit_checks(shared, &changes, Some(&[row])) {
                Ok(()) => respond(req, json!({"ok":true}).to_string(), "application/json; charset=utf-8", 200),
                Err((status, message)) => respond(req, json!({"error":message}).to_string(), "application/json; charset=utf-8", status),
            }

        }
        (Method::Post, "/api/claim") => {
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
            let waybill = v["waybill"].as_str().unwrap_or("");
            let who = v["who"].as_str().unwrap_or("");
            match claim(shared, waybill, who) {
                Some(owner) => respond(
                    req,
                    json!({"ok": true, "claimedBy": owner}).to_string(),
                    "application/json; charset=utf-8",
                    200,
                ),
                None => respond(
                    req,
                    json!({"error": "单号或名字是空的"}).to_string(),
                    "application/json; charset=utf-8",
                    400,
                ),
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
        Ok(inner) => {
            // 认领状态不另存一份：每次下发按单号现填，避免两边漂移
            let mut rows = inner.rows.clone();
            for row in rows.iter_mut() {
                row.claimed_by = inner.claims.get(&row.waybill).cloned().unwrap_or_default();
            }
            (rows, now_secs())
        }
        Err(_) => (Vec::new(), 0),
    }
}

/// 手机端「领取任务」：把某一票记到某人名下。同一票先到先得，不覆盖别人。
///
/// 返回这票的归属人：刚领到就是 who，已经被别人领了就是那个人（前端据此提示）。
fn claim(shared: &MobileShared, waybill: &str, who: &str) -> Option<String> {
    let Ok(mut inner) = shared.inner.lock() else {
        return None;
    };
    let w = waybill.trim().to_string();
    let who = who.trim().to_string();
    if w.is_empty() || who.is_empty() {
        return None;
    }
    if let Some(existing) = inner.claims.get(&w) {
        return Some(existing.clone());
    }
    inner.claims.insert(w, who.clone());
    Some(who)
}

#[cfg(test)]
fn toggle(shared: &MobileShared, waybill: &str, inbound: &str, name: &str, checked: bool) -> bool {
    let Ok(mut inner) = shared.inner.lock() else {
        return false;
    };
    let mut hit = false;
    for row in inner.rows.iter_mut() {
        if row.waybill == waybill && row.inbound == inbound && row.name == name {
            // 缺任一号码均不能出货。
            if checked && (row.blocked || row.waybill.trim().is_empty() || row.inbound.trim().is_empty()) {
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
    fn claim_is_first_come_first_served() {
        let shared = new_shared();
        assert_eq!(claim(&shared, "W1", "张三"), Some("张三".to_string()));
        // 已经有人领了：不覆盖，返回原主人，前端据此提示「已被 X 领了」
        assert_eq!(claim(&shared, "W1", "李四"), Some("张三".to_string()));
        assert_eq!(
            shared.inner.lock().unwrap().claims.get("W1").cloned(),
            Some("张三".to_string())
        );
        // 空单号 / 空名字直接拒
        assert_eq!(claim(&shared, "", "王五"), None);
        assert_eq!(claim(&shared, "W2", "  "), None);
    }

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

                task: false,


                task_id: String::new(),


                claimed_by: String::new(),
            source_id: String::new(), record_id: String::new(), dataset_id: String::new(), version: 0,
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
                task: false,

                task_id: String::new(),

                claimed_by: String::new(),
            source_id: String::new(), record_id: String::new(), dataset_id: String::new(), version: 0,
            }];
        }
        assert!(!toggle(&shared, "", "", "没有箱唛", true));
        assert!(!rows(&shared)[0].checked);
        // 取消（改回未出）不受影响
        assert!(toggle(&shared, "", "", "没有箱唛", false));
    }

    #[test]
    fn discovery_answers_broadcast() {
        // 端口 17655 被真机占着就跳过（开发机常常同时开着 App）
        let shared = new_shared();
        let info = match start(&shared, Vec::new(), "dtoken".into(), None) {
            Ok(i) => i,
            Err(_) => return,
        };
        let sock = std::net::UdpSocket::bind(("127.0.0.1", 0)).expect("bind client");
        sock.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("timeout");
        sock.send_to(DISCOVER_MAGIC, ("127.0.0.1", DISCOVER_PORT))
            .expect("send");
        let mut buf = [0u8; 512];
        match sock.recv_from(&mut buf) {
            Ok((n, _)) => {
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                assert!(text.contains("\"port\""), "应答缺 port：{text}");
                // 开发机上真机可能同时在跑、抢着回包，那不是我们的口令就跳过
                if text.contains("dtoken") {
                    assert!(text.contains(&info.port.to_string()), "端口对不上：{text}");
                }
            }
            // 超时：17655 被真机占着，跳过
            Err(_) => {}
        }
        stop(&shared);
    }

    #[test]
    fn amazon_exception_requires_persisted_annotation() {
        let shared = new_shared();
        let id = record_id("", "", "亚马逊货品");
        let changes = vec![("".into(), "".into(), "亚马逊货品".into(), true)];
        assert_eq!(commit_checks(&shared, &changes, None).unwrap_err().0, 400);
        shared.db.lock().unwrap().set_setting("packing_special_shipments",
            &serde_json::json!({id.clone(): "亚马逊"}).to_string()).unwrap();
        commit_checks(&shared, &changes, None).unwrap();
        assert!(shared.db.lock().unwrap().all_packing_checks().unwrap()[0].3);
        let other = vec![("".into(), "".into(), "普通货品".into(), true)];
        assert_eq!(commit_checks(&shared, &other, None).unwrap_err().0, 400);
        shared.db.lock().unwrap().set_setting("packing_special_shipments", "{}").unwrap();
        assert_eq!(commit_checks(&shared, &changes, None).unwrap_err().0, 400);
        commit_checks(&shared, &[("".into(), "".into(), "亚马逊货品".into(), false)], None).unwrap();
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

            task: false,


            task_id: String::new(),


            claimed_by: String::new(),
            source_id: String::new(), record_id: String::new(), dataset_id: String::new(), version: 0,
        }];
        let info = start(&shared, seed, "testtoken".into(), None).expect("start server");
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
        let manifest = http(&host, &format!("GET /api/app-update/manifest?t={} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n", info.token));
        assert!(manifest.contains("200 OK") && manifest.contains("shellApi"));
        let private_update = http(&host, "GET /api/app-update/manifest HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert!(private_update.contains("403"));
        let stale_update = http(&host, &format!("GET /api/app-update/bundle?t={}&v=stale HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n", info.token));
        assert!(stale_update.contains("409"));

        let unpaired = http(&host, "GET /api/sheet HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert!(unpaired.contains("403"));

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

        let mut operation = rows(&shared)[0].clone();
        operation.checked = true;
        let body = serde_json::to_string(&operation).unwrap();
        let post = format!(
            "POST /api/check?t={} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            info.token,
            body.len(),
            body
        );
        let res = http(&host, &post);
        assert!(res.contains("200 OK"));
        assert!(rows(&shared)[0].checked);
        assert!(shared.db.lock().unwrap().all_packing_checks().unwrap()[0].3);
        let stale = http(&host, &post);
        assert!(stale.contains("409"));
        shared.inner.lock().unwrap().info.as_mut().unwrap().token = "rotated-token".into();
        let old_cookie = http(&host, "GET /api/sheet HTTP/1.1\r\nHost: x\r\nCookie: mt=testtoken\r\nConnection: close\r\n\r\n");
        assert!(old_cookie.contains("403"));


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
    fn commit_fixture() -> MobileShared {
        let shared = new_shared();
        let mut inner = shared.inner.lock().unwrap();
        let mut seed = vec![MobileRow {
            waybill: "W".into(), inbound: "I".into(), name: "货品".into(),
            source_id: "source-a".into(), ..Default::default()
        }];
        prepare_rows(&mut seed, &[]);
        inner.rows = seed;
        inner.info = Some(build_info(17650, "test".into()));
        drop(inner);
        shared
    }

    #[test]
    fn commits_before_returning_and_rejects_stale_versions() {
        let shared = commit_fixture();
        let snapshot = rows(&shared);
        let change = vec![("W".into(), "I".into(), "货品".into(), true)];
        commit_checks(&shared, &change, Some(&snapshot)).unwrap();
        assert!(shared.db.lock().unwrap().all_packing_checks().unwrap()[0].3);
        assert!(rows(&shared)[0].checked);
        assert_eq!(rows(&shared)[0].version, 1);
        let stale = vec![("W".into(), "I".into(), "货品".into(), false)];
        assert_eq!(commit_checks(&shared, &stale, Some(&snapshot)).unwrap_err().0, 409);
        assert!(rows(&shared)[0].checked);
        let mut changed = rows(&shared);
        changed[0].source_id = "source-b".into();
        sync(&shared, changed);
        assert_eq!(commit_checks(&shared, &stale, Some(&snapshot)).unwrap_err().0, 409);
    }

    #[test]
    fn concurrent_devices_only_one_matching_version_can_commit() {
        let shared = commit_fixture();
        let snapshot = rows(&shared);
        let handles: Vec<_> = (0..2).map(|_| {
            let shared = shared.clone();
            let snapshot = snapshot.clone();
            std::thread::spawn(move || commit_checks(&shared,
                &[("W".into(), "I".into(), "货品".into(), true)], Some(&snapshot)))
        }).collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|r| matches!(r, Err((409,_)))).count(), 1);
    }

    #[test]
    fn failed_database_write_preserves_memory_and_version() {
        let shared = commit_fixture();
        let path = test_database_path("failure");
        *shared.db.lock().unwrap() = pdfimpose_core::Db::open(&path).unwrap();
        let connection = pdfimpose_core::rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_save BEFORE INSERT ON packing_checks
            BEGIN SELECT RAISE(ABORT, '模拟磁盘保存失败'); END;").unwrap();
        let snapshot = rows(&shared);
        assert_eq!(commit_checks(&shared, &[("W".into(), "I".into(), "货品".into(), true)], Some(&snapshot)).unwrap_err().0, 500);
        assert!(!rows(&shared)[0].checked);
        assert_eq!(rows(&shared)[0].version, 0);
        assert!(shared.db.lock().unwrap().all_packing_checks().unwrap().is_empty());
        drop(connection);
        drop(shared);
        std::fs::remove_file(path).unwrap();
    }

    fn test_database_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("mobile-{label}-{}-{}.db", std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()))
    }

    #[test]
    fn mobile_commit_survives_shutdown_without_desktop_poll() {
        let path = test_database_path("restart");
        {
            let shared = commit_fixture();
            *shared.db.lock().unwrap() = pdfimpose_core::Db::open(&path).unwrap();
            let snapshot = rows(&shared);
            commit_checks(&shared, &[("W".into(), "I".into(), "货品".into(), true)], Some(&snapshot)).unwrap();
        }
        {
            let db = pdfimpose_core::Db::open(&path).unwrap();
            assert!(db.all_packing_checks().unwrap()[0].3);
        }
        std::fs::remove_file(path).unwrap();
    }


    #[test]
    fn discovery_payload_contains_no_pairing_credentials() {
        let info = build_info(17650, "secret-pairing-token".into());
        let payload = discovery_payload(&info);
        assert!(payload.get("token").is_none());
        assert!(!payload.to_string().contains("secret-pairing-token"));
        for url in payload["urls"].as_array().unwrap() {
            assert!(!url.as_str().unwrap().contains('?'));
        }
    }

    #[test]
    fn pairing_tokens_are_long_random_values() {
        let first = new_token().unwrap();
        let second = new_token().unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

}




#[cfg(test)]
mod required_shipping_ids_tests {
    use super::*;
    #[test]
    fn shipping_requires_both_numbers_but_allows_undo() {
        let shared = new_shared();
        for (waybill, inbound) in [("", "I1"), ("W1", ""), (" ", "I1"), ("", "")] {
            let result = commit_checks(&shared, &[(waybill.into(), inbound.into(), "商品".into(), true)], None);
            assert_eq!(result.unwrap_err().0, 400);
        }
        assert!(commit_checks(&shared, &[("W1".into(), "I1".into(), "商品".into(), true)], None).is_ok());
        assert!(commit_checks(&shared, &[("".into(), "".into(), "商品".into(), false)], None).is_ok());
    }
}