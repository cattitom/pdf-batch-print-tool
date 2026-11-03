//! 微信 4.x 本地消息：从 `Weixin.exe` 内存取 WCDB 密钥 → 解密 `db_storage` →
//! 按群读取消息 / 导出 CSV。
//!
//! 核心链路和 `tools/wechat/wechat_msg_tool.rs` 同源，这里改成给 Tauri 命令用的库：
//! - 原始库只读，解密结果写到 `xwechat_files/wechat_msg_decrypted/<wxid>/`
//! - 解密主库后会把 `-wal` 当前周期的帧 patch 进副本
//! - 文本/引用/文件等消息用 ruzstd 解压（不引 zstd-sys，避免 C 构建）

use std::collections::{HashMap, HashSet};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use pdfimpose_core::rusqlite;
use regex::Regex;
use serde::Serialize;

// ==================== SQLCipher4 / WCDB 常量 ====================

const PAGE: usize = 4096;
const SALT_SZ: usize = 16;
const HMAC_SZ: usize = 64;
const RESERVE: usize = 80; // IV 16 @ 4016 + HMAC-SHA512 64 @ 4032
const CIPHER_NAME: &[u8] = b"com.Tencent.WCDB.Config.Cipher";
const XOR_MASK: &[u8] = &[
    0xd2, 0xc7, 0x44, 0x24, 0x58, 0x02, 0x00, 0x00, 0x00, 0x48, 0x89, 0x44, 0x24, 0x50, 0x48, 0x8b,
    0x45, 0x00, 0x48, 0x84, 0x4c, 0x24, 0x48, 0x48, 0x89, 0x44, 0x25, 0x40, 0x48, 0x58, 0x4c, 0x24,
];
const BLOB_MAX: u64 = 1024;
const MAX_ADDR: u64 = 0x0000_8000_0000_0000;
const MAX_REGION: usize = 200 * 1024 * 1024;
const CHUNK: usize = 32 * 1024 * 1024;
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

// ==================== DTO ====================

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDto {
    pub data_dir: String,
    pub wxid: String,
    pub decrypted: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    pub accounts: Vec<AccountDto>,
    /// 默认选中的账号目录（解密过的优先）
    pub selected: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanDto {
    pub ok: usize,
    pub failed: usize,
    pub skipped: usize,
    pub wal_frames: usize,
    pub ms: u128,
    pub out_dir: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomDto {
    pub id: String,
    pub name: String,
    pub count: i64,
    pub last: i64,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Extracted {
    pub customer: String,
    pub sku: String,
    pub qty: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MsgDto {
    pub ts: i64,
    pub time: String,
    pub sender: String,
    pub cat: String,
    pub text: String,
    pub fields: Extracted,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportDto {
    pub rows: usize,
    pub path: String,
}

// ==================== 账号 / 状态 ====================

fn docs_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let home = PathBuf::from(home);
        out.push(home.join("Documents").join("xwechat_files"));
        out.push(home.join("OneDrive").join("Documents").join("xwechat_files"));
    }
    out
}

/// 数据目录下的账号（wxid_*，且带 db_storage）
fn discover_accounts() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for base in docs_candidates() {
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("wxid_") && e.path().join("db_storage").is_dir() {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 解密输出目录：<数据目录同级>/wechat_msg_decrypted/<wxid>
fn out_dir_for(data_dir: &Path) -> PathBuf {
    let wxid = data_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    data_dir
        .parent()
        .unwrap_or(Path::new("."))
        .join("wechat_msg_decrypted")
        .join(wxid)
}

fn is_decrypted(data_dir: &Path) -> bool {
    out_dir_for(data_dir).join("message").is_dir()
}

fn sort_by_active(accounts: &mut [PathBuf]) {
    accounts.sort_by_key(|p| {
        std::fs::metadata(p.join("db_storage").join("message"))
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH)
    });
}

/// 用户手选的目录 → 账号目录。支持三种选法：
/// `xwechat_files` 根（下面有 wxid_*）、`wxid_*` 账号目录、账号目录里的 `db_storage`。
fn accounts_from_path(p: &Path) -> Vec<PathBuf> {
    let p = if p
        .file_name()
        .map(|n| n.to_string_lossy().eq_ignore_ascii_case("db_storage"))
        .unwrap_or(false)
    {
        p.parent().unwrap_or(p).to_path_buf()
    } else {
        p.to_path_buf()
    };

    let is_account = |dir: &Path| {
        dir.file_name()
            .map(|n| n.to_string_lossy().starts_with("wxid_"))
            .unwrap_or(false)
            && dir.join("db_storage").is_dir()
    };
    if is_account(&p) {
        return vec![p];
    }

    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&p) {
        for e in rd.flatten() {
            let dir = e.path();
            if dir.is_dir() && is_account(&dir) {
                out.push(dir);
            }
        }
    }
    out.sort();
    out
}

/// 自动发现 + 用户手选的目录，去重后按活跃度排序
fn collect_accounts(extra: &[String]) -> Vec<PathBuf> {
    let mut accs = discover_accounts();
    for raw in extra {
        for dir in accounts_from_path(Path::new(raw)) {
            if !accs.contains(&dir) {
                accs.push(dir);
            }
        }
    }
    sort_by_active(&mut accs);
    accs
}

fn account_dto(p: &Path) -> AccountDto {
    AccountDto {
        data_dir: p.to_string_lossy().to_string(),
        wxid: p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        decrypted: is_decrypted(p),
    }
}

pub fn status(extra: &[String]) -> StatusDto {
    let accs = collect_accounts(extra);
    let accounts: Vec<AccountDto> = accs.iter().map(|p| account_dto(p)).collect();
    let selected = accs
        .iter()
        .rev()
        .find(|p| is_decrypted(p))
        .or_else(|| accs.last())
        .map(|p| p.to_string_lossy().to_string());
    StatusDto { accounts, selected }
}

/// 校验用户选的目录，返回里面能认出的账号（给「选目录」按钮用）。
pub fn check_dir(path: &str) -> Vec<AccountDto> {
    accounts_from_path(Path::new(path))
        .iter()
        .map(|p| account_dto(p))
        .collect()
}

// ==================== 密钥缓存（监控轮询用） ====================

struct CachedKeys {
    keys: Vec<[u8; 32]>,
    at: std::time::Instant,
}

static KEY_CACHE: OnceLock<Mutex<HashMap<String, CachedKeys>>> = OnceLock::new();

fn key_cache() -> &'static Mutex<HashMap<String, CachedKeys>> {
    KEY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn store_keys(data_dir: &Path, keys: &[[u8; 32]]) {
    if let Ok(mut cache) = key_cache().lock() {
        cache.insert(
            data_dir.to_string_lossy().to_string(),
            CachedKeys {
                keys: keys.to_vec(),
                at: std::time::Instant::now(),
            },
        );
    }
}

/// 监控轮询用：优先拿 scan 时缓存的密钥；没有或过期就重新扫一次内存。
fn cached_keys(data_dir: &Path) -> Result<Vec<[u8; 32]>, String> {
    let key = data_dir.to_string_lossy().to_string();
    {
        let cache = key_cache().lock().map_err(|_| "密钥缓存锁被占用")?;
        if let Some(c) = cache.get(&key) {
            if c.at.elapsed() < std::time::Duration::from_secs(600) {
                return Ok(c.keys.clone());
            }
        }
    }

    let pid = memscan::find_weixin_pid()?;
    let keys = memscan::scan_candidate_keys(pid)?;
    if keys.is_empty() {
        return Err("没扫到密钥（微信是否已登录？可以试试管理员权限）".to_string());
    }
    store_keys(data_dir, &keys);
    Ok(keys)
}

// ==================== 消息字段提取 ====================

struct Rules {
    qty: Regex,
    sku_line: Regex,
    customer_label: Regex,
    sku_label: Regex,
    code: Regex,
}

fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| Rules {
        // 数量：共（10 件）/ 3件 / 20 箱
        qty: Regex::new(r"[（(]?\s*(\d{1,5})\s*[件箱个套包]").expect("数量正则"),
        // 「白锁钢丝钳，60/件」→ 逗号前是品名
        sku_line: Regex::new(r"^(.{1,40}?)[，,]\s*\d+\s*/\s*件").expect("SKU 行正则"),
        // 客户：九宸 / 收货人 张三
        customer_label: Regex::new(r"(?:客户|收货人|公司|客户名)\s*[:：]\s*([^\s,，。；;]{1,16})")
            .expect("客户正则"),
        // SKU：CX-GG-60PCS / 品名：白锁钢丝钳（后面跟数量就截掉）
        sku_label: Regex::new(
            r"(?:SKU|sku|品名|商品)\s*[:：]\s*([^\n]{1,50}?)(?:\s+\d{1,5}\s*[件箱个套包]|$)",
        )
        .expect("SKU 正则"),
        // 独立编码：IB064261025RX / ZX-WYB-LF
        code: Regex::new(r"(?i)(?:[A-Z]{2}\d{6,}[A-Z]{0,3}|[A-Z]{2,4}(?:-[A-Z0-9]{1,8}){2,})")
            .expect("编码正则"),
    })
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}')
}

fn looks_like_spec(line: &str) -> bool {
    line.contains('×')
        || line.contains('=')
        || (line.contains('/') && line.chars().any(|c| c.is_ascii_digit()))
        || line.starts_with('实')
}

pub fn extract_fields(text: &str) -> Extracted {
    let r = rules();
    let mut out = Extracted::default();

    if let Some(c) = r.qty.captures(text) {
        out.qty = c[1].to_string();
    }

    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();

    // 客户：标签优先；否则在「有数量、至少两行」的记录里，第一行短、没数字/规格符的当客户（「翰林」）
    if let Some(c) = r.customer_label.captures(text) {
        out.customer = c[1].trim().to_string();
    } else if !out.qty.is_empty() && lines.len() >= 2 {
        if let Some(line) = lines.first() {
            let short = line.chars().count() <= 12;
            let plain = !line.chars().any(|c| c.is_ascii_digit())
                && !line.contains('×')
                && !line.contains('/')
                && !line.contains('=')
                && !line.contains('，')
                && !line.contains(',')
                && !line.contains('件');
            if short && plain {
                out.customer = (*line).to_string();
            }
        }
    }

    // SKU：标签优先；其次「名称，60/件」；再其次独立编码；最后含中文且不像规格的行
    if let Some(c) = r.sku_label.captures(text) {
        out.sku = c[1].trim().to_string();
    } else if !out.qty.is_empty() {
        for line in &lines {
            if let Some(c) = r.sku_line.captures(line) {
                out.sku = c[1].trim().to_string();
                break;
            }
        }
        if out.sku.is_empty() {
            if let Some(m) = r.code.find(text) {
                out.sku = m.as_str().to_uppercase();
            }
        }
        if out.sku.is_empty() {
            for line in &lines {
                if *line == out.customer || looks_like_spec(line) {
                    continue;
                }
                if line.chars().any(is_cjk) {
                    out.sku = (*line).to_string();
                    break;
                }
            }
        }
    }

    out
}

// ==================== SQLCipher ====================

fn pbkdf2_sha512(key: &[u8], salt: &[u8], iters: u32) -> [u8; 32] {
    use hmac::Hmac;
    use sha2::Sha512;
    let mut out = [0u8; 32];
    let _ = pbkdf2::pbkdf2::<Hmac<Sha512>>(key, salt, iters, &mut out);
    out
}

fn verify_key(enc_key: &[u8; 32], page1: &[u8]) -> bool {
    use hmac::{Hmac, Mac};
    use sha2::Sha512;
    if page1.len() < PAGE {
        return false;
    }
    let salt = &page1[..SALT_SZ];
    let mac_salt: Vec<u8> = salt.iter().map(|b| b ^ 0x3A).collect();
    let mac_key = pbkdf2_sha512(enc_key, &mac_salt, 2);
    let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(&mac_key).unwrap();
    mac.update(&page1[SALT_SZ..PAGE - RESERVE + 16]);
    mac.update(&1u32.to_le_bytes());
    mac.verify_slice(&page1[PAGE - HMAC_SZ..]).is_ok()
}

/// 解一页（SQLCipher 4：page 1 前 16 字节是 salt）。
fn decrypt_page(enc_key: &[u8; 32], page: &[u8; PAGE], pgno: u64) -> [u8; PAGE] {
    use aes::cipher::{BlockDecrypt, KeyInit};
    use aes::Aes256;

    let cipher = Aes256::new(enc_key.into());
    let iv: [u8; 16] = page[PAGE - RESERVE..PAGE - RESERVE + 16]
        .try_into()
        .expect("IV 固定 16 字节");
    let body: &[u8] = if pgno == 1 {
        &page[SALT_SZ..PAGE - RESERVE]
    } else {
        &page[..PAGE - RESERVE]
    };
    let start = if pgno == 1 { SALT_SZ } else { 0 };
    let mut out = [0u8; PAGE];
    if pgno == 1 {
        out[..SALT_SZ].copy_from_slice(b"SQLite format 3\0");
    }

    let mut prev = iv;
    for (i, chunk) in body.chunks_exact(16).enumerate() {
        let ct: [u8; 16] = chunk.try_into().expect("按 16 字节切块");
        let mut plain = ct;
        cipher.decrypt_block((&mut plain).into());
        for j in 0..16 {
            out[start + i * 16 + j] = plain[j] ^ prev[j];
        }
        prev = ct;
    }
    out
}

/// 逐页解密，流式写盘（message 库可能几 GB）。
fn decrypt_db_file(src: &Path, dst: &Path, enc_key: &[u8; 32]) -> Result<usize, String> {
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败：{e}"))?;
    }
    let mut input =
        BufReader::new(std::fs::File::open(src).map_err(|e| format!("打开 {} 失败：{e}", src.display()))?);
    let mut first = [0u8; PAGE];
    if input.read_exact(&mut first).is_err() {
        return Err(format!("文件太小: {}", src.display()));
    }
    if &first[..15] == b"SQLite format 3" {
        std::fs::copy(src, dst).map_err(|e| format!("复制 {} 失败：{e}", src.display()))?;
        return Ok(0);
    }

    let mut output = BufWriter::new(
        std::fs::File::create(dst).map_err(|e| format!("创建 {} 失败：{e}", dst.display()))?,
    );
    let mut page = [0u8; PAGE];
    let mut pages = 0u64;
    loop {
        if pages > 0 {
            match input.read_exact(&mut page) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(format!("读 {} 失败：{e}", src.display())),
            }
        }
        pages += 1;
        let plain = decrypt_page(enc_key, if pages == 1 { &first } else { &page }, pages);
        output
            .write_all(&plain)
            .map_err(|e| format!("写 {} 失败：{e}", dst.display()))?;
    }
    output.flush().map_err(|e| format!("写 {} 失败：{e}", dst.display()))?;
    Ok(pages as usize)
}

const WAL_HDR: usize = 32;
const WAL_FRAME: usize = 24 + PAGE;

/// 把 WAL 当前周期的帧 patch 进已解密的库。
fn patch_wal(src_db: &Path, dst: &Path, enc_key: &[u8; 32]) -> Result<usize, String> {
    let wal_path = PathBuf::from(format!("{}-wal", src_db.display()));
    let mut wal = match std::fs::File::open(&wal_path) {
        Ok(f) => f,
        Err(_) => return Ok(0),
    };
    let mut hdr = [0u8; WAL_HDR];
    if wal.read_exact(&mut hdr).is_err() {
        return Ok(0);
    }
    let salt1 = u32::from_be_bytes(hdr[16..20].try_into().expect("4 字节"));
    let salt2 = u32::from_be_bytes(hdr[20..24].try_into().expect("4 字节"));

    let mut out = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dst)
        .map_err(|e| format!("打开 {} 失败：{e}", dst.display()))?;
    let mut frame = [0u8; WAL_FRAME];
    let mut patched = 0usize;
    while wal.read_exact(&mut frame).is_ok() {
        let pgno = u32::from_be_bytes(frame[0..4].try_into().expect("4 字节")) as u64;
        if pgno == 0 || pgno > 1_000_000 {
            continue;
        }
        if u32::from_be_bytes(frame[8..12].try_into().expect("4 字节")) != salt1
            || u32::from_be_bytes(frame[12..16].try_into().expect("4 字节")) != salt2
        {
            continue;
        }
        let mut page = [0u8; PAGE];
        page.copy_from_slice(&frame[24..]);
        let plain = decrypt_page(enc_key, &page, pgno);
        out.seek(SeekFrom::Start((pgno - 1) * PAGE as u64))
            .map_err(|e| format!("定位 WAL 页失败：{e}"))?;
        out.write_all(&plain)
            .map_err(|e| format!("写 WAL 页失败：{e}"))?;
        patched += 1;
    }
    Ok(patched)
}

fn walk_db_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().map_or(false, |x| x == "db") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn read_first_page(p: &Path) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(p).ok()?;
    let mut buf = vec![0u8; PAGE];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// 扫描并解密一个账号。返回解锁的库数 / 失败数 / WAL 帧数。
pub fn scan(data_dir: &Path) -> Result<ScanDto, String> {
    let storage = data_dir.join("db_storage");
    let db_files = walk_db_files(&storage);
    if db_files.is_empty() {
        return Err(format!(
            "没找到 .db 文件：{}（确认这是 xwechat_files 下的 wxid_* 账号目录）",
            storage.display()
        ));
    }
    let pid = memscan::find_weixin_pid()?;
    log::info!("微信扫描：pid={pid}，库 {} 个", db_files.len());
    let keys = memscan::scan_candidate_keys(pid)?;
    log::info!("微信扫描：候选密钥 {} 个", keys.len());
    if keys.is_empty() {
        return Err("没扫到候选密钥（微信是否已登录？可以试试管理员权限运行）".to_string());
    }
    store_keys(data_dir, &keys);

    let out_dir = out_dir_for(data_dir);
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("创建解密目录失败：{e}"))?;
    let started = std::time::Instant::now();
    let (mut ok, mut failed, mut skipped, mut wal_frames) = (0usize, 0usize, 0usize, 0usize);
    for src in &db_files {
        let rel = src.strip_prefix(&storage).unwrap_or(src);
        let dst = out_dir.join(rel);
        let Some(page1) = read_first_page(src) else {
            skipped += 1;
            continue;
        };
        let Some(key) = keys.iter().find(|k| verify_key(k, &page1)) else {
            skipped += 1;
            continue;
        };
        if let Err(e) = decrypt_db_file(src, &dst, key) {
            failed += 1;
            log::warn!("微信解密失败 {}：{e}", rel.display());
            continue;
        }
        ok += 1;
        match patch_wal(src, &dst, key) {
            Ok(n) => wal_frames += n,
            Err(e) => log::warn!("微信 WAL patch 失败 {}：{e}", rel.display()),
        }
    }
    if ok == 0 {
        return Err(format!(
            "{} 个库一个都没解开：密钥对不上（微信版本变了？），或微信没登录",
            db_files.len()
        ));
    }
    log::info!(
        "微信解密完成 {ok}/{}（失败 {failed}，WAL {wal_frames} 帧，{}ms）",
        db_files.len(),
        started.elapsed().as_millis()
    );
    Ok(ScanDto {
        ok,
        failed,
        skipped,
        wal_frames,
        ms: started.elapsed().as_millis(),
        out_dir: out_dir.to_string_lossy().to_string(),
    })
}

// ==================== 进程内存扫描（Windows） ====================

#[cfg(windows)]
mod memscan {
    use super::*;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_ACCESS_RIGHTS};

    const QUERY_VM_READ: PROCESS_ACCESS_RIGHTS = PROCESS_ACCESS_RIGHTS(0x0410);

    struct Proc(HANDLE);
    impl Drop for Proc {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn read_mem(h: HANDLE, addr: u64, sz: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; sz];
        let mut n: usize = 0;
        unsafe {
            ReadProcessMemory(h, addr as *const _, buf.as_mut_ptr().cast(), sz, Some(&mut n)).ok()?;
        }
        buf.truncate(n);
        if n == 0 { None } else { Some(buf) }
    }

    fn enum_regions(h: HANDLE) -> Vec<(u64, usize)> {
        let mut regs = Vec::new();
        let mut addr: u64 = 0;
        loop {
            let mut mbi = MEMORY_BASIC_INFORMATION::default();
            let ret = unsafe {
                VirtualQueryEx(
                    h,
                    Some(addr as *const _),
                    &mut mbi,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if ret == 0 {
                break;
            }
            let base = mbi.BaseAddress as u64;
            let size = mbi.RegionSize;
            let readable = matches!(mbi.Protect.0, 2 | 4 | 8 | 16 | 32 | 64 | 128);
            if mbi.State == MEM_COMMIT && readable && size > 0 && size < MAX_REGION {
                regs.push((base, size));
            }
            let next = base + size as u64;
            if next <= addr {
                break;
            }
            addr = next;
        }
        regs
    }

    pub fn find_weixin_pid() -> Result<u32, String> {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq Weixin.exe", "/FO", "CSV", "/NH"])
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| format!("调用 tasklist 失败：{e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut best: Option<(u64, u32)> = None;
        for line in text.lines() {
            let cols: Vec<&str> = line.trim_matches('"').split("\",\"").collect();
            if cols.len() >= 5 {
                if let (Ok(pid), Ok(mem)) = (
                    cols[1].parse::<u32>(),
                    cols[4].replace([',', ' ', 'K'], "").parse::<u64>(),
                ) {
                    if best.map_or(true, |(m, _)| mem > m) {
                        best = Some((mem, pid));
                    }
                }
            }
        }
        best.map(|(_, pid)| pid)
            .ok_or_else(|| "微信未运行（找不到 Weixin.exe），先登录微信再扫描".to_string())
    }

    pub fn scan_candidate_keys(pid: u32) -> Result<Vec<[u8; 32]>, String> {
        let h = unsafe { OpenProcess(QUERY_VM_READ, false, pid) }
            .map_err(|e| format!("OpenProcess 失败：{e}（试试管理员权限）"))?;
        let _keep = Proc(h);

        let mut needle_addrs: HashSet<u64> = HashSet::new();
        for (base, size) in enum_regions(h) {
            let mut off = 0usize;
            let mut tail: Vec<u8> = Vec::new();
            let mut tail_base = base;
            while off < size {
                let cur = CHUNK.min(size - off);
                if let Some(chunk) = read_mem(h, base + off as u64, cur) {
                    let data = [tail.as_slice(), chunk.as_slice()].concat();
                    let dbase = if tail.is_empty() { base + off as u64 } else { tail_base };
                    let mut from = 0;
                    while let Some(pos) = find(&data, CIPHER_NAME, from) {
                        needle_addrs.insert(dbase + pos as u64);
                        from = pos + 1;
                    }
                    let keep = 32.min(data.len());
                    tail = data[data.len() - keep..].to_vec();
                    tail_base = dbase + (data.len() - keep) as u64;
                } else {
                    tail.clear();
                    tail_base = base + (off + cur) as u64;
                }
                off += cur;
            }
        }
        if needle_addrs.is_empty() {
            return Ok(vec![]);
        }

        let mut pat_bytes: Vec<[u8; 16]> = Vec::new();
        for &a in &needle_addrs {
            let mut p = [0u8; 16];
            p[..8].copy_from_slice(&a.to_le_bytes());
            p[8..].copy_from_slice(&(CIPHER_NAME.len() as u64).to_le_bytes());
            pat_bytes.push(p);
        }
        pat_bytes.sort();

        let mut cands: Vec<[u8; 32]> = Vec::new();
        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        for (base, size) in enum_regions(h) {
            let mut off = 0usize;
            let mut tail: Vec<u8> = Vec::new();
            let mut tail_base = base;
            while off < size {
                let cur = CHUNK.min(size - off);
                if let Some(chunk) = read_mem(h, base + off as u64, cur) {
                    let data = [tail.as_slice(), chunk.as_slice()].concat();
                    let dbase = if tail.is_empty() { base + off as u64 } else { tail_base };
                    for pat in &pat_bytes {
                        let mut from = 0;
                        while let Some(pos) = find(&data, pat, from) {
                            from = pos + 1;
                            let qaddr = dbase + pos as u64;
                            let Some(node) =
                                qaddr.checked_sub(0x10).and_then(|base| read_mem(h, base, 0x50))
                            else {
                                continue;
                            };
                            if node.len() < 0x40 {
                                continue;
                            }
                            let u64at = |off: usize| -> u64 {
                                u64::from_le_bytes(node[off..off + 8].try_into().unwrap())
                            };
                            if !needle_addrs.contains(&u64at(0x10))
                                || u64at(0x18) != CIPHER_NAME.len() as u64
                            {
                                continue;
                            }
                            let cfg = u64at(0x28);
                            if !(0x10000..MAX_ADDR).contains(&cfg) {
                                continue;
                            }
                            let Some(obj) = read_mem(h, cfg + 0x88, 0x28) else { continue };
                            if obj.len() < 0x18 {
                                continue;
                            }
                            let dp = u64::from_le_bytes(obj[0x08..0x10].try_into().unwrap());
                            let dl = u64::from_le_bytes(obj[0x10..0x18].try_into().unwrap());
                            if dl == 0 || dl > BLOB_MAX || !(0x10000..MAX_ADDR).contains(&dp) {
                                continue;
                            }
                            let Some(blob) = read_mem(h, dp, dl as usize) else { continue };
                            if blob.len() as u64 != dl {
                                continue;
                            }
                            let decoded: Vec<u8> = blob
                                .iter()
                                .enumerate()
                                .map(|(i, v)| v ^ XOR_MASK[i % XOR_MASK.len()])
                                .collect();
                            for run in scan_hex_literals(&decoded) {
                                let starts: Box<dyn Iterator<Item = usize>> = if run.len() > 96 {
                                    Box::new((0..run.len() - 63).step_by(32))
                                } else {
                                    Box::new(std::iter::once(0))
                                };
                                for s in starts {
                                    if s + 64 > run.len() {
                                        continue;
                                    }
                                    let Some(ek) = hex32(&run[s..s + 64]) else { continue };
                                    let distinct = ek.iter().collect::<HashSet<_>>().len();
                                    if distinct < 15 {
                                        continue;
                                    }
                                    if seen.insert(ek) {
                                        cands.push(ek);
                                    }
                                }
                            }
                        }
                    }
                    let keep = 0x80.min(data.len());
                    tail = data[data.len() - keep..].to_vec();
                    tail_base = dbase + (data.len() - keep) as u64;
                } else {
                    tail.clear();
                    tail_base = base + (off + cur) as u64;
                }
                off += cur;
            }
        }
        Ok(cands)
    }

    fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
        if needle.is_empty() || hay.len() < needle.len() || from >= hay.len() {
            return None;
        }
        hay[from..]
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|p| p + from)
    }

    fn scan_hex_literals(data: &[u8]) -> Vec<&[u8]> {
        let mut out = Vec::new();
        let mut i = 0;
        while i + 2 < data.len() {
            if (data[i] == b'x' || data[i] == b'X') && data[i + 1] == b'\'' {
                let mut j = i + 2;
                while j < data.len() && data[j].is_ascii_hexdigit() {
                    j += 1;
                }
                if j < data.len() && data[j] == b'\'' && j - i - 2 >= 64 {
                    out.push(&data[i + 2..j]);
                    i = j + 1;
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    fn hex32(s: &[u8]) -> Option<[u8; 32]> {
        if s.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for k in 0..32 {
            let hi = (s[2 * k] as char).to_digit(16)?;
            let lo = (s[2 * k + 1] as char).to_digit(16)?;
            out[k] = (hi * 16 + lo) as u8;
        }
        Some(out)
    }
}

#[cfg(not(windows))]
mod memscan {
    pub fn find_weixin_pid() -> Result<u32, String> {
        Err("只有 Windows 版微信支持自动取密钥".to_string())
    }
    pub fn scan_candidate_keys(_pid: u32) -> Result<Vec<[u8; 32]>, String> {
        Err("只有 Windows 版微信支持自动取密钥".to_string())
    }
}

// ==================== 消息读取 ====================

struct Msg {
    ts: i64,
    sender: String,
    cat: String,
    text: String,
}

pub struct Store {
    root: PathBuf,
    nicks: HashMap<String, String>,
}

fn md5_hex(s: &str) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

impl Store {
    pub fn open(root: PathBuf) -> Result<Self, String> {
        let contact = root.join("contact").join("contact.db");
        let mut nicks = HashMap::new();
        if contact.is_file() {
            let con = rusqlite::Connection::open_with_flags(
                &contact,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| format!("打开 contact.db 失败：{e}"))?;
            let mut st = con
                .prepare("SELECT username, nick_name, remark FROM Contact")
                .map_err(|e| format!("读联系人失败：{e}"))?;
            let rows = st
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(|e| format!("读联系人失败：{e}"))?;
            for (u, nick, remark) in rows.flatten() {
                let disp = remark
                    .filter(|s| !s.is_empty())
                    .or_else(|| nick.filter(|s| !s.is_empty()))
                    .unwrap_or_else(|| u.clone());
                nicks.insert(u, disp);
            }
        }
        Ok(Self { root, nicks })
    }

    fn nick(&self, user: &str) -> String {
        self.nicks
            .get(user)
            .cloned()
            .unwrap_or_else(|| user.to_string())
    }

    fn for_each_msg_table(
        &self,
        chat_id: &str,
        mut f: impl FnMut(&rusqlite::Connection, &str) -> Result<(), String>,
    ) -> Result<(), String> {
        let table = format!("Msg_{}", md5_hex(chat_id));
        for n in 0..4 {
            let db = self.root.join("message").join(format!("message_{n}.db"));
            if !db.is_file() {
                continue;
            }
            let con = rusqlite::Connection::open_with_flags(
                &db,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| format!("打开 {} 失败：{e}", db.display()))?;
            let exists: bool = con
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [&table],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(0)
                > 0;
            if exists {
                f(&con, &table)?;
            }
        }
        Ok(())
    }

    pub fn rooms(&self) -> Result<Vec<RoomDto>, String> {
        let mut rooms: HashMap<String, (i64, i64)> = HashMap::new();
        for n in 0..4 {
            let db = self.root.join("message").join(format!("message_{n}.db"));
            if !db.is_file() {
                continue;
            }
            let con = rusqlite::Connection::open_with_flags(
                &db,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| format!("打开 {} 失败：{e}", db.display()))?;
            let mut st = con
                .prepare("SELECT user_name FROM Name2Id")
                .map_err(|e| format!("读 Name2Id 失败：{e}"))?;
            let names: Vec<String> = st
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| format!("读 Name2Id 失败：{e}"))?
                .flatten()
                .collect();
            drop(st);
            for user in names {
                if !user.ends_with("@chatroom") {
                    continue;
                }
                let table = format!("Msg_{}", md5_hex(&user));
                let exists: bool = con
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                        [&table],
                        |r| r.get::<_, i64>(0),
                    )
                    .unwrap_or(0)
                    > 0;
                if !exists {
                    continue;
                }
                let (cnt, last): (i64, Option<i64>) = con
                    .query_row(
                        &format!("SELECT COUNT(*), MAX(create_time) FROM {table}"),
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .map_err(|e| format!("统计群消息失败：{e}"))?;
                let e = rooms.entry(user).or_insert((0, 0));
                e.0 += cnt;
                e.1 = e.1.max(last.unwrap_or(0));
            }
        }
        let mut out: Vec<RoomDto> = rooms
            .into_iter()
            .map(|(id, (cnt, last))| RoomDto {
                name: self.nick(&id),
                id,
                count: cnt,
                last,
            })
            .collect();
        out.sort_by(|a, b| b.last.cmp(&a.last).then(b.count.cmp(&a.count)));
        Ok(out)
    }

    fn load_msgs(
        &self,
        chat_id: &str,
        since: Option<i64>,
        kind: Option<u32>,
    ) -> Result<Vec<Msg>, String> {
        let mut msgs: Vec<Msg> = Vec::new();
        let mut seen: HashSet<i64> = HashSet::new();
        self.for_each_msg_table(chat_id, |con, table| {
            let mut idmap: HashMap<i64, String> = HashMap::new();
            let mut st = con
                .prepare("SELECT rowid, user_name FROM Name2Id")
                .map_err(|e| format!("读 Name2Id 失败：{e}"))?;
            for (rid, user) in st
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .map_err(|e| format!("读 Name2Id 失败：{e}"))?
                .flatten()
            {
                idmap.insert(rid, user);
            }
            drop(st);

            let mut sql = format!(
                "SELECT server_id, create_time, local_type, real_sender_id, message_content FROM {table}"
            );
            let mut since_val = 0i64;
            if since.is_some() {
                sql.push_str(" WHERE create_time >= ?1");
                since_val = since.unwrap();
            }
            sql.push_str(" ORDER BY create_time");
            let mut st = con
                .prepare(&sql)
                .map_err(|e| format!("读消息表失败：{e}"))?;
            let mut rows = if since.is_some() {
                st.query([since_val])
                    .map_err(|e| format!("读消息表失败：{e}"))?
            } else {
                st.query([]).map_err(|e| format!("读消息表失败：{e}"))?
            };
            while let Some(row) = rows.next().map_err(|e| format!("读消息失败：{e}"))? {
                let server_id: i64 = row.get(0).unwrap_or(0);
                let ts: i64 = row.get(1).unwrap_or(0);
                let ltype: i64 = row.get(2).unwrap_or(0);
                let rid: i64 = row.get(3).unwrap_or(0);
                let content = row.get_ref(4).map_err(|e| format!("读消息失败：{e}"))?;
                if let Some(tf) = kind {
                    if (ltype & 0xFFFF_FFFF) as u32 != tf {
                        continue;
                    }
                }
                if server_id > 0 && !seen.insert(server_id) {
                    continue;
                }
                let bytes: Vec<u8> = match content {
                    rusqlite::types::ValueRef::Text(t) => t.to_vec(),
                    rusqlite::types::ValueRef::Blob(b) => b.to_vec(),
                    rusqlite::types::ValueRef::Integer(i) => i.to_le_bytes().to_vec(),
                    _ => Vec::new(),
                };
                let sender = match idmap.get(&rid) {
                    Some(u) => self.nick(u),
                    None => {
                        if rid == 0 {
                            "系统".into()
                        } else {
                            format!("成员{rid}")
                        }
                    }
                };
                let (cat, text) = render(ltype, &bytes, self);
                msgs.push(Msg { ts, sender, cat, text });
            }
            Ok(())
        })?;
        msgs.sort_by_key(|m| m.ts);
        Ok(msgs)
    }
}

fn fmt_ts(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

fn zstd_decode(data: &[u8]) -> Option<Vec<u8>> {
    let mut dec = ruzstd::decoding::StreamingDecoder::new(data).ok()?;
    let mut out = Vec::new();
    dec.read_to_end(&mut out).ok()?;
    Some(out)
}

fn strip_wxid_prefix(text: &str, store: &Store) -> Option<(String, String)> {
    for name in ["wxid_", "gh_"] {
        if let Some(rest) = text.strip_prefix(name) {
            if let Some(colon) = rest.find(':') {
                let user = format!("{name}{}", &rest[..colon]);
                let after = rest[colon + 1..].trim_start_matches([' ', '\n', '\r']);
                return Some((store.nick(&user), after.to_string()));
            }
        }
    }
    None
}

fn render(ltype: i64, content: &[u8], store: &Store) -> (String, String) {
    let real = (ltype & 0xFFFF_FFFF) as u32;
    // 微信 4.x 的长文本/引用/文件消息会用 zstd 压缩，但 local_type 仍是原来的类型；
    // 不先解压就会按 UTF-8 解出满屏 U+FFFD（截图里的乱码）。
    let decoded = if content.len() >= 4 && content[..4] == ZSTD_MAGIC {
        zstd_decode(content)
    } else {
        None
    };
    let content = decoded.as_deref().unwrap_or(content);
    let text_of = |c: &[u8]| -> String { String::from_utf8_lossy(c).into_owned() };
    match real {
        1 => {
            let t = text_of(content);
            match strip_wxid_prefix(&t, store) {
                Some((who, after)) => ("引用".into(), format!("{who}: {after}")),
                None => ("文本".into(), t.trim().to_string()),
            }
        }
        3 => ("图片".into(), String::new()),
        34 => ("语音".into(), String::new()),
        43 => ("视频".into(), String::new()),
        47 => ("表情".into(), String::new()),
        10002 => ("撤回".into(), String::new()),
        10000 => ("系统".into(), strip_tags(&text_of(content))),
        49 => {
            // 引用/文件/链接：XML（上面已经统一解过 zstd）
            let t = text_of(content);
            let body = strip_wxid_prefix(&t, store).map(|(_, after)| after).unwrap_or(t);
            parse_appmsg(&body)
        }
        other => (format!("类型{other}"), String::new()),
    }
}

fn parse_appmsg(xml: &str) -> (String, String) {
    let tag_val = |tag: &str| -> Option<String> {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        let a = xml.find(&open)? + open.len();
        let rel = xml[a..].find(&close)?;
        Some(xml[a..a + rel].trim().to_string())
    };
    let atype = tag_val("type").unwrap_or_default();
    let title = tag_val("title").unwrap_or_default();
    match atype.as_str() {
        "6" => ("文件".into(), title),
        "57" => {
            let refer = xml.split("<refermsg>").nth(1).unwrap_or("");
            let dname = refer
                .split("<displayname>")
                .nth(1)
                .and_then(|s| s.split("</displayname>").next())
                .unwrap_or("")
                .trim()
                .to_string();
            let rcontent = refer
                .split("<content>")
                .nth(1)
                .and_then(|s| s.split("</content>").next())
                .unwrap_or("");
            let rtitle = rcontent
                .split("<title>")
                .nth(1)
                .and_then(|s| s.split("</title>").next())
                .unwrap_or("")
                .trim()
                .to_string();
            if rtitle.is_empty() {
                ("引用".into(), title)
            } else {
                ("引用".into(), format!("{dname}: {rtitle} |> {title}"))
            }
        }
        "5" => ("链接".into(), title),
        _ => (
            "消息".into(),
            if title.is_empty() { "(未识别)".into() } else { title },
        ),
    }
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().chars().take(120).collect()
}

// ==================== 对外：房间 / 消息 / 导出 ====================

fn open_store(data_dir: &Path) -> Result<Store, String> {
    if !is_decrypted(data_dir) {
        return Err("这个账号还没解密：先点「扫描 / 解密」".to_string());
    }
    Store::open(out_dir_for(data_dir))
}

pub fn rooms(data_dir: &Path) -> Result<Vec<RoomDto>, String> {
    open_store(data_dir)?.rooms()
}

fn msg_to_dto(m: Msg) -> MsgDto {
    MsgDto {
        ts: m.ts,
        time: fmt_ts(m.ts),
        sender: m.sender,
        cat: m.cat,
        fields: extract_fields(&m.text),
        text: m.text,
    }
}

pub fn messages(
    data_dir: &Path,
    chat_id: &str,
    days: Option<i64>,
    limit: Option<usize>,
    kind: Option<u32>,
) -> Result<Vec<MsgDto>, String> {
    let store = open_store(data_dir)?;
    let since = days.map(|d| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|x| x.as_secs() as i64)
            .unwrap_or(0);
        now - d * 86400
    });
    let mut msgs = store.load_msgs(chat_id, since, kind)?;
    let take = limit.unwrap_or(200).max(1);
    if msgs.len() > take {
        msgs = msgs.split_off(msgs.len() - take);
    }
    Ok(msgs.into_iter().map(msg_to_dto).collect())
}

/// 监控轮询：把新 WAL 帧补进副本，再取 `after_ts` 之后的新消息。
pub fn poll(data_dir: &Path, chat_id: &str, after_ts: i64) -> Result<Vec<MsgDto>, String> {
    if !is_decrypted(data_dir) {
        return Err("这个账号还没解密：先点「扫描 / 解密」".to_string());
    }
    let keys = cached_keys(data_dir)?;
    let storage = data_dir.join("db_storage");
    let out = out_dir_for(data_dir);
    for n in 0..4 {
        let src = storage.join("message").join(format!("message_{n}.db"));
        if !src.is_file() {
            continue;
        }
        let dst = out.join("message").join(format!("message_{n}.db"));
        let Some(page1) = read_first_page(&src) else { continue };
        let Some(key) = keys.iter().find(|k| verify_key(k, &page1)) else {
            continue;
        };
        // 主库比解密副本新（checkpoint 过）就整库重解；否则只补 WAL
        let src_m = std::fs::metadata(&src).and_then(|m| m.modified()).ok();
        let dst_m = std::fs::metadata(&dst).and_then(|m| m.modified()).ok();
        if let (Some(s), Some(d)) = (src_m, dst_m) {
            if s > d {
                if let Err(e) = decrypt_db_file(&src, &dst, key) {
                    log::warn!("微信监控：重解 {} 失败：{e}", src.display());
                }
            }
        }
        if wal_changed(&src) {
            match patch_wal(&src, &dst, key) {
                Ok(_) => mark_wal_seen(&src),
                Err(e) => log::warn!("微信监控：补 WAL {} 失败：{e}", src.display()),
            }
        }
    }
    let store = Store::open(out)?;
    let msgs = store.load_msgs(chat_id, Some(after_ts), None)?;
    Ok(msgs
        .into_iter()
        .filter(|m| m.ts > after_ts)
        .map(msg_to_dto)
        .collect())
}

/// WAL 的 mtime 缓存：没变化就别每 2 秒重写一遍 4MB 帧
static WAL_SEEN: OnceLock<Mutex<HashMap<String, std::time::SystemTime>>> = OnceLock::new();

fn wal_seen() -> &'static Mutex<HashMap<String, std::time::SystemTime>> {
    WAL_SEEN.get_or_init(|| Mutex::new(HashMap::new()))
}

fn wal_path(src_db: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", src_db.display()))
}

fn wal_changed(src_db: &Path) -> bool {
    let path = wal_path(src_db);
    let Ok(meta) = std::fs::metadata(&path) else {
        return false;
    };
    let Ok(mtime) = meta.modified() else {
        return false;
    };
    let key = path.to_string_lossy().to_string();
    let Ok(cache) = wal_seen().lock() else {
        return true;
    };
    match cache.get(&key) {
        Some(old) if *old == mtime => false,
        _ => true,
    }
}

fn mark_wal_seen(src_db: &Path) {
    let path = wal_path(src_db);
    if let Ok(meta) = std::fs::metadata(&path) {
        if let Ok(mtime) = meta.modified() {
            if let Ok(mut cache) = wal_seen().lock() {
                cache.insert(path.to_string_lossy().to_string(), mtime);
            }
        }
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn export_csv(
    data_dir: &Path,
    chat_id: &str,
    days: Option<i64>,
    kind: Option<u32>,
    out_path: &Path,
) -> Result<ExportDto, String> {
    let store = open_store(data_dir)?;
    let since = days.map(|d| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|x| x.as_secs() as i64)
            .unwrap_or(0);
        now - d * 86400
    });
    let msgs = store.load_msgs(chat_id, since, kind)?;
    let mut f = std::fs::File::create(out_path)
        .map_err(|e| format!("创建 {} 失败：{e}", out_path.display()))?;
    f.write_all(&[0xef, 0xbb, 0xbf])
        .map_err(|e| format!("写 CSV 失败：{e}"))?;
    writeln!(f, "时间,发送者,类别,内容").map_err(|e| format!("写 CSV 失败：{e}"))?;
    for m in &msgs {
        writeln!(
            f,
            "{},{},{},{}",
            fmt_ts(m.ts),
            csv_escape(&m.sender),
            csv_escape(&m.cat),
            csv_escape(&m.text)
        )
        .map_err(|e| format!("写 CSV 失败：{e}"))?;
    }
    Ok(ExportDto {
        rows: msgs.len(),
        path: out_path.to_string_lossy().to_string(),
    })
}

// ==================== 测试 ====================

#[cfg(test)]
mod tests {
    use super::*;

    fn cbc_encrypt(key: &[u8; 32], iv: &[u8; 16], plain: &[u8]) -> Vec<u8> {
        use aes::cipher::{BlockEncrypt, KeyInit};
        use aes::Aes256;

        let cipher = Aes256::new(key.into());
        let mut prev = *iv;
        let mut out = Vec::with_capacity(plain.len());
        for chunk in plain.chunks_exact(16) {
            let mut block = [0u8; 16];
            for j in 0..16 {
                block[j] = chunk[j] ^ prev[j];
            }
            let mut enc = block;
            cipher.encrypt_block((&mut enc).into());
            out.extend_from_slice(&enc);
            prev = enc;
        }
        out
    }

    fn seal_page(key: &[u8; 32], salt: &[u8; 16], plain: &[u8; PAGE], pgno: u32) -> [u8; PAGE] {
        use hmac::{Hmac, Mac};
        use sha2::Sha512;

        let iv = [7u8; 16];
        let body = if pgno == 1 {
            &plain[SALT_SZ..PAGE - RESERVE]
        } else {
            &plain[..PAGE - RESERVE]
        };
        let ct = cbc_encrypt(key, &iv, body);
        let mut page = [0u8; PAGE];
        if pgno == 1 {
            page[..SALT_SZ].copy_from_slice(salt);
            page[SALT_SZ..PAGE - RESERVE].copy_from_slice(&ct);
        } else {
            page[..PAGE - RESERVE].copy_from_slice(&ct);
        }
        page[PAGE - RESERVE..PAGE - RESERVE + 16].copy_from_slice(&iv);
        let mac_salt: Vec<u8> = salt.iter().map(|b| b ^ 0x3a).collect();
        let mac_key = pbkdf2_sha512(key, &mac_salt, 2);
        let mut mac = Hmac::<Sha512>::new_from_slice(&mac_key).unwrap();
        mac.update(&page[SALT_SZ..PAGE - RESERVE + 16]);
        mac.update(&pgno.to_le_bytes());
        page[PAGE - HMAC_SZ..].copy_from_slice(&mac.finalize().into_bytes());
        page
    }

    #[test]
    fn render_decodes_zstd_text_messages() {
        // Python zstandard 压的「你好，测试消息」
        let hex = "28b52ffd2015a90000e4bda0e5a5bdefbc8ce6b58be8af95e6b688e681af";
        let data: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let store = Store {
            root: PathBuf::new(),
            nicks: HashMap::new(),
        };
        let (cat, text) = render(1, &data, &store);
        assert_eq!(cat, "文本");
        assert_eq!(text, "你好，测试消息");
    }

    #[test]
    fn extract_packing_record_from_multiline_message() {
        let f = extract_fields(
            "翰林
白锁钢丝钳，60/件
57×33×32/6000=10.032
实 28.7，共（10 件）",
        );
        assert_eq!(f.customer, "翰林");
        assert_eq!(f.sku, "白锁钢丝钳");
        assert_eq!(f.qty, "10");
    }

    #[test]
    fn extract_labeled_fields_on_one_line() {
        let f = extract_fields("客户：九宸 SKU：CX-GG-60PCS 3件");
        assert_eq!(f.customer, "九宸");
        assert_eq!(f.sku, "CX-GG-60PCS");
        assert_eq!(f.qty, "3");
    }

    #[test]
    fn extract_fields_blank_when_nothing_matches() {
        let f = extract_fields("今晚吃啥");
        assert!(f.customer.is_empty() && f.sku.is_empty() && f.qty.is_empty());
    }

    #[test]
    fn accounts_from_path_accepts_root_account_and_db_storage() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("wechat-accounts-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("xwechat_files");
        for id in ["wxid_a", "wxid_b"] {
            std::fs::create_dir_all(root.join(id).join("db_storage")).unwrap();
        }
        // 不是 wxid_ 开头的目录不认
        std::fs::create_dir_all(root.join("other").join("db_storage")).unwrap();

        assert_eq!(accounts_from_path(&root).len(), 2, "选根目录要列出两个账号");
        assert_eq!(
            accounts_from_path(&root.join("wxid_a")).len(),
            1,
            "选账号目录要认"
        );
        assert_eq!(
            accounts_from_path(&root.join("wxid_a").join("db_storage")).len(),
            1,
            "选到 db_storage 也能回退到账号目录"
        );
        assert!(accounts_from_path(&root.join("other")).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn page_decrypt_and_key_verify_roundtrip() {
        let key = [0x2bu8; 32];
        let salt = [0x3cu8; 16];
        let mut plain = [0u8; PAGE];
        plain[..16].copy_from_slice(b"SQLite format 3\0");
        for (i, b) in plain[16..PAGE - RESERVE].iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let sealed = seal_page(&key, &salt, &plain, 1);
        assert!(verify_key(&key, &sealed));
        assert_eq!(&decrypt_page(&key, &sealed, 1)[..], &plain[..]);
    }

    #[test]
    fn wal_frames_patch_into_decrypted_db() {
        let key = [0x11u8; 32];
        let salt = [0x22u8; 16];
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("wechat-wal-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let src = dir.join("message_0.db");
        let dst = dir.join("out.db");
        let wal = dir.join("message_0.db-wal");
        std::fs::write(&dst, vec![0u8; PAGE * 2]).unwrap();

        let mut page2 = [0u8; PAGE];
        for (i, b) in page2[..PAGE - RESERVE].iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7);
        }
        let sealed = seal_page(&key, &salt, &page2, 2);

        let mut wal_bytes = vec![0u8; WAL_HDR];
        wal_bytes[16..20].copy_from_slice(&0xA1B2C3D4u32.to_be_bytes());
        wal_bytes[20..24].copy_from_slice(&0x01020304u32.to_be_bytes());
        let mut stale = vec![0u8; WAL_FRAME];
        stale[0..4].copy_from_slice(&1u32.to_be_bytes());
        stale[8..12].copy_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
        wal_bytes.extend_from_slice(&stale);
        let mut frame = vec![0u8; WAL_FRAME];
        frame[0..4].copy_from_slice(&2u32.to_be_bytes());
        frame[8..12].copy_from_slice(&0xA1B2C3D4u32.to_be_bytes());
        frame[12..16].copy_from_slice(&0x01020304u32.to_be_bytes());
        frame[24..].copy_from_slice(&sealed);
        wal_bytes.extend_from_slice(&frame);
        std::fs::write(&wal, &wal_bytes).unwrap();

        assert_eq!(patch_wal(&src, &dst, &key).unwrap(), 1);
        let mut got = vec![0u8; PAGE * 2];
        std::fs::File::open(&dst)
            .unwrap()
            .read_exact(&mut got)
            .unwrap();
        assert_eq!(&got[PAGE..], &page2[..]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
