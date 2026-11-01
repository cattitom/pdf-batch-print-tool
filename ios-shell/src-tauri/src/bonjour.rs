//! iOS 上走系统 DNS-SD（mDNSResponder）做零配置发现。
//!
//! 为什么不用 Rust 的 mDNS 库：iPhone 上 App 自己发广播/组播会被系统拦
//! （需要 com.apple.developer.networking.multicast entitlement，个人签名拿不到，
//! 实测 send_to 255.255.255.255 和子网广播都是 No route to host）。
//! DNSServiceBrowse 是让系统的 mDNSResponder 去收发组播，App 只是问它要结果，
//! 所以不受这个限制（只需要「本地网络」权限，用户已经给了）。

use std::{
    ffi::{CStr, CString},
    net::ToSocketAddrs,
    os::raw::{c_char, c_int, c_void},
    sync::mpsc,
    time::{Duration, Instant},
};

type SdRef = *mut c_void;
type SdErr = i32;
type SdFlags = u32;

const FLAG_ADD: SdFlags = 0x2;
/// 和电脑端 mobile::MDNS_SERVICE_TYPE 保持一致
const REGTYPE: &str = "_shippingshell._tcp";

#[link(name = "System")]
extern "C" {
    fn DNSServiceBrowse(
        sd_ref: *mut SdRef,
        flags: SdFlags,
        interface_index: u32,
        regtype: *const c_char,
        domain: *const c_char,
        callback: extern "C" fn(SdRef, SdFlags, u32, SdErr, *const c_char, *const c_char, *const c_char, *mut c_void),
        context: *mut c_void,
    ) -> SdErr;

    fn DNSServiceResolve(
        sd_ref: *mut SdRef,
        flags: SdFlags,
        interface_index: u32,
        name: *const c_char,
        regtype: *const c_char,
        domain: *const c_char,
        callback: extern "C" fn(SdRef, SdFlags, u32, SdErr, *const c_char, *const c_char, u16, u16, *const u8, *mut c_void),
        context: *mut c_void,
    ) -> SdErr;

    fn DNSServiceRefSockFD(sd_ref: SdRef) -> c_int;
    fn DNSServiceProcessResult(sd_ref: SdRef) -> SdErr;
    fn DNSServiceRefDeallocate(sd_ref: SdRef);
}

enum Msg {
    Found {
        name: String,
        regtype: String,
        domain: String,
        ifindex: u32,
    },
    Resolved {
        host: String,
        port: u16,
        token: String,
    },
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

extern "C" fn on_found(
    _sd: SdRef,
    flags: SdFlags,
    ifindex: u32,
    err: SdErr,
    name: *const c_char,
    regtype: *const c_char,
    domain: *const c_char,
    ctx: *mut c_void,
) {
    if err != 0 || ctx.is_null() || flags & FLAG_ADD == 0 {
        return;
    }
    let tx = unsafe { &*(ctx as *const mpsc::Sender<Msg>) };
    let _ = tx.send(Msg::Found {
        name: cstr(name),
        regtype: cstr(regtype),
        domain: cstr(domain),
        ifindex,
    });
}

extern "C" fn on_resolved(
    _sd: SdRef,
    _flags: SdFlags,
    _ifindex: u32,
    err: SdErr,
    _fullname: *const c_char,
    host: *const c_char,
    port: u16,
    txt_len: u16,
    txt: *const u8,
    ctx: *mut c_void,
) {
    if err != 0 || ctx.is_null() {
        return;
    }
    let tx = unsafe { &*(ctx as *const mpsc::Sender<Msg>) };
    let _ = tx.send(Msg::Resolved {
        host: cstr(host).trim_end_matches('.').to_string(),
        port: u16::from_be(port),
        token: parse_txt(txt, txt_len).unwrap_or_default(),
    });
}

/// TXT 记录是一串 [长度][key=value]，挑出 token
fn parse_txt(txt: *const u8, len: u16) -> Option<String> {
    if txt.is_null() || len == 0 {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(txt, len as usize) };
    let mut i = 0usize;
    while i < bytes.len() {
        let size = bytes[i] as usize;
        i += 1;
        if i + size > bytes.len() {
            break;
        }
        let text = String::from_utf8_lossy(&bytes[i..i + size]).to_string();
        i += size;
        if let Some(v) = text.strip_prefix("token=") {
            return Some(v.to_string());
        }
    }
    None
}

/// 等 sd_ref 上的事件最多 budget 时间（回调里把结果推进 channel）
fn pump(sd: SdRef, budget: Duration) {
    let fd = unsafe { DNSServiceRefSockFD(sd) };
    if fd < 0 {
        return;
    }
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        let n = unsafe { libc::poll(&mut pfd, 1, 100) };
        if n > 0 {
            unsafe {
                DNSServiceProcessResult(sd);
            }
        }
    }
}

/// 找到电脑就返回 (ip, port, token)
pub fn discover(timeout: Duration) -> Result<(String, u16, String), String> {
    let deadline = Instant::now() + timeout;

    // 1) 浏览服务
    let (tx, rx) = mpsc::channel::<Msg>();
    let ctx = Box::into_raw(Box::new(tx));
    let regtype = CString::new(REGTYPE).map_err(|e| e.to_string())?;
    let mut browse: SdRef = std::ptr::null_mut();
    let err = unsafe {
        DNSServiceBrowse(
            &mut browse,
            0,
            0,
            regtype.as_ptr(),
            std::ptr::null(),
            on_found,
            ctx as *mut c_void,
        )
    };
    if err != 0 {
        unsafe { drop(Box::from_raw(ctx)) };
        return Err(format!("DNSServiceBrowse 失败（{err}）"));
    }

    let mut found = None;
    while found.is_none() && Instant::now() < deadline {
        pump(browse, Duration::from_millis(300));
        while let Ok(msg) = rx.try_recv() {
            if let Msg::Found {
                name,
                regtype,
                domain,
                ifindex,
            } = msg
            {
                found = Some((name, regtype, domain, ifindex));
                break;
            }
        }
    }
    unsafe {
        DNSServiceRefDeallocate(browse);
        drop(Box::from_raw(ctx));
    }
    let (name, regtype, domain, ifindex) =
        found.ok_or_else(|| "Bonjour 没搜到电脑（电脑端服务没开？）".to_string())?;

    // 2) 解析出主机名和 TXT
    let (tx2, rx2) = mpsc::channel::<Msg>();
    let ctx2 = Box::into_raw(Box::new(tx2));
    let name_c = CString::new(name).map_err(|e| e.to_string())?;
    let reg_c = CString::new(regtype).map_err(|e| e.to_string())?;
    let dom_c = CString::new(domain).map_err(|e| e.to_string())?;
    let mut resolve: SdRef = std::ptr::null_mut();
    let err = unsafe {
        DNSServiceResolve(
            &mut resolve,
            0,
            ifindex,
            name_c.as_ptr(),
            reg_c.as_ptr(),
            dom_c.as_ptr(),
            on_resolved,
            ctx2 as *mut c_void,
        )
    };
    if err != 0 {
        unsafe { drop(Box::from_raw(ctx2)) };
        return Err(format!("DNSServiceResolve 失败（{err}）"));
    }

    let mut resolved = None;
    while resolved.is_none() && Instant::now() < deadline {
        pump(resolve, Duration::from_millis(300));
        while let Ok(msg) = rx2.try_recv() {
            if let Msg::Resolved { host, port, token } = msg {
                resolved = Some((host, port, token));
                break;
            }
        }
    }
    unsafe {
        DNSServiceRefDeallocate(resolve);
        drop(Box::from_raw(ctx2));
    }
    let (host, port, token) = resolved.ok_or_else(|| "Bonjour 找到了服务，解析地址超时".to_string())?;
    if token.is_empty() {
        return Err("Bonjour 的 TXT 里没有 token".to_string());
    }

    // 3) 主机名 -> IPv4（.local 由系统 mDNS 解析）
    let addr = format!("{host}:{port}")
        .to_socket_addrs()
        .map_err(|e| format!("解析 {host} 失败：{e}"))?
        .find(|a| a.is_ipv4())
        .ok_or_else(|| format!("{host} 没有 IPv4 地址"))?;

    Ok((addr.ip().to_string(), port, token))
}
