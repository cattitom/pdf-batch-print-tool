//! 微信群消息提取工具（微信 4.x Windows 版，独立单文件，不依赖项目其他代码）
//!
//! 数据链路（与 Python 版 wechat-tools 完全同源）：
//!   1. 定位 `Documents\xwechat_files\wxid_*` 数据目录
//!   2. `scan`:  扫描运行中的 Weixin.exe 进程内存，收集 WCDB Cipher 候选密钥
//!      （内存找 "com.Tencent.WCDB.Config.Cipher" needle → XOR 反混淆 → x'hex' 字面量）
//!      → HMAC-SHA512 校验 → AES-256-CBC 逐页解密 db_storage 全部库（SQLCipher4 规范）
//!   3. `list` / `show` / `export`: rusqlite 直接读解密后的 message_*.db
//!      （Name2Id rowid → 昵称/备注；Msg_<md5(群id)> 每会话一张表；zstd 解压引用/文件 XML）
//!
//! 独立运行的 Cargo.toml（空目录 + 本文件放 src/main.rs）：
//! ```toml
//! [package]
//! name = "wechat_msg_tool"
//! version = "0.1.0"
//! edition = "2024"
//!
//! [dependencies]
//! anyhow = "1"
//! rusqlite = { version = "0.32", features = ["bundled"] }
//! aes = "0.8"
//! sha2 = "0.10"
//! hmac = "0.12"
//! pbkdf2 = "0.12"
//! zstd = "0.13"
//! chrono = "0.4"
//! dirs = "5"
//!
//! [target.'cfg(windows)'.dependencies]
//! windows = { version = "0.62", features = [
//!     "Win32_Foundation",
//!     "Win32_System_Threading",
//!     "Win32_System_Memory",
//!     "Win32_System_Diagnostics_Debug",
//! ] }
//! ```
//!
//! 用法：
//! ```text
//!   wechat_msg_tool scan                         # 解密全部库（微信保持登录，约 10 秒/账号）
//!   wechat_msg_tool list                         # 列出所有群（按最后消息时间排）
//!   wechat_msg_tool show  打包数据群 --days 7     # 最近 7 天，默认显示最近 100 条
//!   wechat_msg_tool export 打包数据群 -o out.csv  # 导出 CSV（带 BOM，Excel 直接开）
//! 选项：
//!   --data-dir <路径>   指定账号目录（默认自动挑最近活跃的 wxid_*）
//!   --out-dir  <路径>   解密输出目录（默认 <数据目录同级>/wechat_msg_decrypted/<wxid>）
//!   --days N            只看最近 N 天
//!   --limit N           show 最多显示条数（默认 100）
//!   --type N            按消息类型过滤：1=文本 3=图片 49=文件/引用 10000=系统
//! ```
//!
//! ⚠️ 原始数据库全程只读，解密结果写新目录；微信升级后算法可能失效（4.1.11+ 密钥缓存已变过一次）。
//!
//! `scan` 解完主库后会把 `-wal` 里当前周期的帧 patch 进副本，尽量在微信运行时跑。
//! 发送者：message_0 的 real_sender_id 是 Name2Id rowid，老库（message_1..3）可能是会话内
//! 角色 1/2，直接查 Name2Id 会张冠李戴；导出前建议拿一句双方都确认的话逐库核对。

use anyhow::{anyhow, bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

// ==================== SQLCipher4 / WCDB 常量（与 Python 版一致） ====================

const PAGE: usize = 4096;
const SALT_SZ: usize = 16;
const HMAC_SZ: usize = 64;
const RESERVE: usize = 80; // IV 16 @ 4016 + HMAC-SHA512 64 @ 4032
/// WCDB Config.Cipher 类名字符串，内存中的 needle
const CIPHER_NAME: &[u8] = b"com.Tencent.WCDB.Config.Cipher";
/// 解密 cipher 配置 blob 的 XOR mask（来自 wcdb-key-tool 逆向）
const XOR_MASK: &[u8] = &[
    0xd2, 0xc7, 0x44, 0x24, 0x58, 0x02, 0x00, 0x00, 0x00, 0x48, 0x89, 0x44, 0x24, 0x50, 0x48, 0x8b,
    0x45, 0x00, 0x48, 0x84, 0x4c, 0x24, 0x48, 0x48, 0x89, 0x44, 0x25, 0x40, 0x48, 0x58, 0x4c, 0x24,
];
const BLOB_MAX: u64 = 1024;
const MAX_ADDR: u64 = 0x0000_8000_0000_0000;
const MAX_REGION: usize = 200 * 1024 * 1024;
const CHUNK: usize = 32 * 1024 * 1024; // 大块读取减少 syscall 往返
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

fn pbkdf2_sha512(key: &[u8], salt: &[u8], iters: u32) -> [u8; 32] {
    use hmac::Hmac;
    use sha2::Sha512;
    let mut out = [0u8; 32];
    let _ = pbkdf2::pbkdf2::<Hmac<Sha512>>(key, salt, iters, &mut out);
    out
}

/// HMAC-SHA512 校验 enc_key 是否匹配某库的页 1（SQLCipher4 规范）
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

/// 解一页（SQLCipher 4：page 1 前 16 字节是 salt，其它页没有）。
/// 返回完整的 4096 字节明文页，尾部 80 字节保留区填 0。
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

/// 逐页解密一个 SQLCipher4 库文件，输出明文 sqlite。
/// 流式处理：message_*.db 能到几 GB，整份读进内存会 OOM。
fn decrypt_db_file(src: &Path, dst: &Path, enc_key: &[u8; 32]) -> Result<usize> {
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut input = BufReader::new(std::fs::File::open(src)?);
    let mut first = [0u8; PAGE];
    if input.read_exact(&mut first).is_err() {
        bail!("文件太小: {}", src.display());
    }
    if &first[..15] == b"SQLite format 3" {
        // 已是明文，直接复制
        std::fs::copy(src, dst)?;
        return Ok(0);
    }

    let mut output = BufWriter::new(std::fs::File::create(dst)?);
    let mut page = [0u8; PAGE];
    let mut pages = 0u64;
    loop {
        // 第一页已经读出来了，后面的循环从第二页开始
        if pages > 0 {
            match input.read_exact(&mut page) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
        }
        pages += 1;
        let plain = decrypt_page(enc_key, if pages == 1 { &first } else { &page }, pages);
        output.write_all(&plain)?;
    }
    output.flush()?;
    Ok(pages as usize)
}

/// WAL 头 32 字节；每帧 = 24 字节帧头 + 4096 字节页
const WAL_HDR: usize = 32;
const WAL_FRAME: usize = 24 + PAGE;

/// 把 WAL 里「当前周期」的帧 patch 进已解密的库。
/// 微信的 WAL 是预分配的，里面混着上一周期的旧帧；用 WAL 头的 salt 过滤。
fn patch_wal(src_db: &Path, dst: &Path, enc_key: &[u8; 32]) -> Result<usize> {
    let wal_path = std::path::PathBuf::from(format!("{}-wal", src_db.display()));
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

    let mut out = std::fs::OpenOptions::new().read(true).write(true).open(dst)?;
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
        out.seek(SeekFrom::Start((pgno - 1) * PAGE as u64))?;
        out.write_all(&plain)?;
        patched += 1;
    }
    Ok(patched)
}

// ==================== MD5（RFC 1321 精简实现，只为算 Msg_<md5> 表名，省一个依赖） ====================

fn md5_hex(s: &str) -> String {
    // 轮移位与常量表按标准写死
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
        5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    // K[i] = floor(2^32 * |sin(i+1)|)，const 里没有 sin，运行时算一遍（64 次循环，纳秒级）
    let k: Vec<u32> = (0..64)
        .map(|i| (((i as f64 + 1.0).sin().abs()) * 4_294_967_296.0) as u32)
        .collect();

    let data = s.as_bytes();
    let bitlen = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_le_bytes());

    let (mut a0, mut b0, mut c0, mut d0) =
        (0x6745_2301u32, 0xefcd_ab89u32, 0x98ba_dcfeu32, 0x1032_5476u32);
    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let tmp = f
                .wrapping_add(a)
                .wrapping_add(k[i])
                .wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(tmp.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    [a0, b0, c0, d0]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_known_vectors() {
        assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    /// 测试用：跟 decrypt_page 对称的 CBC 加密
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

    /// 测试用：按 SQLCipher 4 布局封一页
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
    fn page_decrypt_and_key_verify_roundtrip() {
        let key = [0x2bu8; 32];
        let salt = [0x3cu8; 16];
        let mut plain = [0u8; PAGE];
        plain[..16].copy_from_slice(b"SQLite format 3\0");
        for (i, b) in plain[16..PAGE - RESERVE].iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }

        let sealed = seal_page(&key, &salt, &plain, 1);
        assert!(verify_key(&key, &sealed), "HMAC 校验应该通过");
        assert_eq!(&decrypt_page(&key, &sealed, 1)[..], &plain[..], "解密结果要还原");

        // 错一个字节就该被 HMAC 拒绝
        let mut bad = sealed;
        bad[100] ^= 0xff;
        assert!(!verify_key(&key, &bad));
    }

    #[test]
    fn wal_frames_patch_into_decrypted_db() {
        use std::io::Read;

        let key = [0x11u8; 32];
        let salt = [0x22u8; 16];
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join(format!("wechat-wal-test-{}", std::process::id()));
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
        // 上一周期的旧帧：salt 对不上，应该被跳过
        let mut stale = vec![0u8; WAL_FRAME];
        stale[0..4].copy_from_slice(&1u32.to_be_bytes());
        stale[8..12].copy_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
        wal_bytes.extend_from_slice(&stale);
        // 当前周期的有效帧
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
        assert_eq!(&got[PAGE..], &page2[..], "第二页应该被 WAL 帧覆盖");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ==================== 进程内存扫描（Windows） ====================

#[cfg(windows)]
mod memscan {
    use super::*;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::Memory::{
        VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT,
    };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_ACCESS_RIGHTS};

    const QUERY_VM_READ: PROCESS_ACCESS_RIGHTS = PROCESS_ACCESS_RIGHTS(0x0410); // QUERY_INFO | VM_READ

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
            ReadProcessMemory(
                h,
                addr as *const _,
                buf.as_mut_ptr().cast(),
                sz,
                Some(&mut n),
            )
            .ok()?;
        }
        buf.truncate(n);
        if n == 0 { None } else { Some(buf) }
    }

    /// 枚举已提交且可读的内存区（跳过 >200MB 的区，与 Python 版 READABLE 集合一致）
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
            // Protect 可读集合 {2,4,8,16,32,64,128}（PAGE_PROTECTION_FLAGS 是 u32 包装类型）
            let readable = matches!(mbi.Protect.0, 2 | 4 | 8 | 16 | 32 | 64 | 128);
            if mbi.State == MEM_COMMIT && readable && size > 0 && (size as usize) < MAX_REGION {
                regs.push((base, size as usize));
            }
            let next = base + size as u64;
            if next <= addr {
                break;
            }
            addr = next;
        }
        regs
    }

    /// 找 Weixin.exe 的 pid（按工作集内存取最大的那个，即主进程）
    pub fn find_weixin_pid() -> Result<u32> {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq Weixin.exe", "/FO", "CSV", "/NH"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output()?;
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
            .ok_or_else(|| anyhow!("微信未运行（找不到 Weixin.exe），先登录微信再 scan"))
    }

    /// 扫描进程内存，收集全部候选 enc_key（去重）。与 Python 版 wcdb_scan_candidate_keys 逐行对应。
    pub fn scan_candidate_keys(pid: u32) -> Result<Vec<[u8; 32]>> {
        let h = unsafe { OpenProcess(QUERY_VM_READ, false, pid) }
            .map_err(|e| anyhow!("OpenProcess 失败: {e}（试试管理员权限运行）"))?;
        let _keep = Proc(h);

        // 第一遍：全内存找 needle（类名字符串）地址
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
        println!("  [*] needle 地址: {} 个", needle_addrs.len());

        // 第二遍：找 needle 指针节点（addr_le + len_le），顺链取 cipher 配置 blob
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
            let mut _tail_base = base;
            while off < size {
                let cur = CHUNK.min(size - off);
                if let Some(chunk) = read_mem(h, base + off as u64, cur) {
                    let data = [tail.as_slice(), chunk.as_slice()].concat();
                    let dbase = if tail.is_empty() { base + off as u64 } else { _tail_base };
                    for pat in &pat_bytes {
                        let mut from = 0;
                        while let Some(pos) = find(&data, pat, from) {
                            from = pos + 1;
                            let qaddr = dbase + pos as u64;
                            let Some(node) = qaddr.checked_sub(0x10).and_then(|base| read_mem(h, base, 0x50)) else { continue };
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
                            // XOR 反混淆后找 x'hex' 字面量（64..192 位十六进制）
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
                    _tail_base = dbase + (data.len() - keep) as u64;
                } else {
                    tail.clear();
                    _tail_base = base + (off + cur) as u64;
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

    /// 扫描 x'...' / X'...' 十六进制字面量，返回内部 hex 串列表
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

// ==================== 数据目录发现 ====================

/// 自动发现 xwechat_files 下的账号目录（wxid_*）
fn discover_accounts() -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Some(doc) = dirs::document_dir() {
        bases.push(doc.join("xwechat_files"));
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        bases.push(PathBuf::from(home).join("Documents").join("xwechat_files"));
    }
    let mut out = Vec::new();
    for base in bases {
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("wxid_") && e.path().join("db_storage").is_dir() {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out
}

/// 解密输出目录：<数据目录同级>/wechat_msg_decrypted/<wxid>
fn out_dir_for(data_dir: &Path) -> PathBuf {
    let wxid = data_dir.file_name().unwrap().to_string_lossy().to_string();
    data_dir
        .parent()
        .unwrap_or(Path::new("."))
        .join("wechat_msg_decrypted")
        .join(&wxid)
}

/// 按账号活跃度（db_storage/message 的修改时间）排，最近活跃的放最后（方便 pop）
fn sort_by_active(accounts: &mut [PathBuf]) {
    accounts.sort_by_key(|p| {
        std::fs::metadata(p.join("db_storage").join("message"))
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH)
    });
}

// ==================== scan 子命令 ====================

fn cmd_scan(data_dir: Option<PathBuf>, out_dir: Option<PathBuf>) -> Result<()> {
    let mut accounts = match data_dir {
        Some(d) => vec![d],
        None => discover_accounts(),
    };
    if accounts.is_empty() {
        bail!("没找到 xwechat_files 下的账号目录，用 --data-dir 手动指定");
    }
    sort_by_active(&mut accounts);
    let multi = accounts.len() > 1;
    for acc in accounts.iter().rev() {
        let wxid = acc.file_name().unwrap().to_string_lossy().to_string();
        // 多个账号又指定了 --out-dir 时必须分开，否则后一个账号会把前一个覆盖掉
        let out = match &out_dir {
            Some(base) if multi => base.join(&wxid),
            Some(base) => base.clone(),
            None => out_dir_for(acc),
        };
        println!("[*] 账号: {wxid}");
        cmd_scan_one(acc, &out)?;
    }
    Ok(())
}

fn cmd_scan_one(data_dir: &Path, out_dir: &Path) -> Result<()> {
    let storage = data_dir.join("db_storage");
    let db_files = walk_db_files(&storage);
    if db_files.is_empty() {
        bail!("没有 .db 文件: {}", storage.display());
    }
    let pid = memscan::find_weixin_pid()?;
    println!("[*] 扫描进程内存找密钥 (pid={pid}) ...");
    let keys = memscan::scan_candidate_keys(pid)?;
    println!("[*] 候选 enc_key: {} 个", keys.len());
    if keys.is_empty() {
        bail!("没扫到候选密钥（微信是否已登录？换管理员权限再试）");
    }
    let t0 = std::time::Instant::now();
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut wal_frames = 0usize;
    std::fs::create_dir_all(out_dir)?;
    for src in &db_files {
        let rel = src.strip_prefix(&storage).unwrap();
        let dst = out_dir.join(rel);
        let Some(page1) = read_first_page(src) else { continue };
        let Some(key) = keys.iter().find(|k| verify_key(k, &page1)) else {
            println!("  [!] 跳过 {}（无匹配密钥，可能是迁移残留）", rel.display());
            continue;
        };
        if let Err(e) = decrypt_db_file(src, &dst, key) {
            failed += 1;
            eprintln!("  [!] 解密失败 {}：{e}", rel.display());
            continue;
        }
        ok += 1;
        match patch_wal(src, &dst, key) {
            Ok(0) => {}
            Ok(n) => wal_frames += n,
            Err(e) => eprintln!("  [!] WAL patch 失败 {}：{e}", rel.display()),
        }
    }
    if ok == 0 {
        bail!(
            "{} 个库一个都没解开：密钥对不上（微信版本变了？），先 `scan` 或换管理员权限",
            db_files.len()
        );
    }
    println!(
        "[+] 解密完成 {ok}/{}（失败 {failed}，WAL 补 {wal_frames} 帧）({:.1?}) -> {}",
        db_files.len(),
        t0.elapsed(),
        out_dir.display()
    );
    Ok(())
}

fn read_first_page(p: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut buf = vec![0u8; PAGE];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
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
            } else if p.extension().map_or(false, |x| x == "db")
                && !p.to_string_lossy().ends_with("-wal")
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

// ==================== 消息读取（rusqlite） ====================

struct Msg {
    ts: i64,
    sender: String,
    cat: String,
    text: String,
}

struct Store {
    /// 解密输出根目录（一个账号一个）
    root: PathBuf,
    /// user_name -> 显示名（备注 > 昵称），整个账号共用 contact.db
    nicks: HashMap<String, String>,
}

impl Store {
    fn open(root: PathBuf) -> Result<Self> {
        let contact = root.join("contact").join("contact.db");
        let mut nicks = HashMap::new();
        if contact.is_file() {
            let con = rusqlite::Connection::open_with_flags(
                &contact,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            let mut st = con.prepare("SELECT username, nick_name, remark FROM Contact")?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?;
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

    /// 遍历 message_0..3.db（新库在前），对会话表执行 f；表名 = Msg_<md5(chat_id)>
    fn for_each_msg_table(
        &self,
        chat_id: &str,
        mut f: impl FnMut(&rusqlite::Connection, &str) -> Result<()>,
    ) -> Result<()> {
        let table = format!("Msg_{}", md5_hex(chat_id));
        for n in 0..4 {
            let db = self.root.join("message").join(format!("message_{n}.db"));
            if !db.is_file() {
                continue;
            }
            let con = rusqlite::Connection::open_with_flags(
                &db,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            let exists: bool = con.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [&table],
                |r| r.get::<_, i64>(0),
            )? > 0;
            if exists {
                f(&con, &table)?;
            }
        }
        Ok(())
    }

    /// 列出全部群聊（跨 message_0..3 合并计数，按最后消息时间排序）
    fn list_rooms(&self) -> Result<Vec<(String, String, i64, i64)>> {
        // wxid -> (总消息数, 最后时间)
        let mut rooms: HashMap<String, (i64, i64)> = HashMap::new();
        for n in 0..4 {
            let db = self.root.join("message").join(format!("message_{n}.db"));
            if !db.is_file() {
                continue;
            }
            let con = rusqlite::Connection::open_with_flags(
                &db,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            let mut st = con.prepare("SELECT user_name FROM Name2Id")?;
            let names: Vec<String> = st
                .query_map([], |r| r.get::<_, String>(0))?
                .flatten()
                .collect();
            drop(st);
            for user in names {
                if !user.ends_with("@chatroom") {
                    continue;
                }
                let table = format!("Msg_{}", md5_hex(&user));
                let exists: bool = con.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [&table],
                    |r| r.get::<_, i64>(0),
                )? > 0;
                if !exists {
                    continue;
                }
                let (cnt, last): (i64, Option<i64>) = con.query_row(
                    &format!("SELECT COUNT(*), MAX(create_time) FROM {table}"),
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                let e = rooms.entry(user).or_insert((0, 0));
                e.0 += cnt;
                e.1 = e.1.max(last.unwrap_or(0));
            }
        }
        let mut out: Vec<(String, String, i64, i64)> = rooms
            .into_iter()
            .map(|(wxid, (cnt, last))| (wxid.clone(), self.nick(&wxid), cnt, last))
            .collect();
        out.sort_by(|a, b| b.3.cmp(&a.3).then(b.2.cmp(&a.2)));
        Ok(out)
    }

    /// 拉一个群的消息（时间升序）；real_sender_id 用各库自己的 Name2Id rowid 解析
    fn load_msgs(
        &self,
        chat_id: &str,
        since: Option<i64>,
        type_filter: Option<u32>,
    ) -> Result<Vec<Msg>> {
        let mut msgs: Vec<Msg> = Vec::new();
        let mut seen: HashSet<i64> = HashSet::new(); // server_id 去重
        self.for_each_msg_table(chat_id, |con, table| {
            // 本库的 Name2Id（rowid -> user_name）
            let mut idmap: HashMap<i64, String> = HashMap::new();
            let mut st = con.prepare("SELECT rowid, user_name FROM Name2Id")?;
            for (rid, user) in st
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
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
            let mut st = con.prepare(&sql)?;
            let mut missing = 0usize;
            let mut rows = if since.is_some() {
                st.query([since_val])?
            } else {
                st.query([])?
            };
            while let Some(row) = rows.next()? {
                let server_id: i64 = row.get(0)?;
                let ts: i64 = row.get(1)?;
                let ltype: i64 = row.get(2)?;
                let rid: i64 = row.get(3)?;
                let content = row.get_ref(4)?;
                if let Some(tf) = type_filter {
                    if (ltype & 0xFFFF_FFFF) as u32 != tf {
                        continue;
                    }
                }
                if server_id > 0 && !seen.insert(server_id) {
                    continue; // 旧库与 message_0 重叠的行
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
                        if rid != 0 {
                            missing += 1;
                        }
                        if rid == 0 {
                            "系统".into()
                        } else {
                            format!("?{rid}")
                        }
                    }
                };
                let (cat, text) = render(ltype, &bytes, self);
                msgs.push(Msg { ts, sender, cat, text });
            }
            if missing > 0 {
                eprintln!(
                    "[!] {chat_id}：{missing} 条消息的 real_sender_id 在 Name2Id 里查不到（老库可能是会话内角色 1/2），发送者名可能不准"
                );
            }
            Ok(())
        })?;
        // message_0..3 是分片，追加完必须按时间归并，
        // 否则 show 取「最近 N 条」会拿到旧库里的消息
        msgs.sort_by_key(|m| m.ts);
        Ok(msgs)
    }
}

// ==================== 内容渲染（引用 / 文件 / zstd XML） ====================

/// 剥掉 "wxid_xxx: " 前缀并映射成昵称（引用回复的原始形态），返回 (昵称, 正文)
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
            // 引用/文件/链接：zstd 压缩的 XML
            let raw: Vec<u8> = if content.len() >= 4 && content[..4] == ZSTD_MAGIC {
                zstd::decode_all(content).unwrap_or_default()
            } else {
                content.to_vec()
            };
            let t = text_of(&raw);
            let body = strip_wxid_prefix(&t, store)
                .map(|(_, after)| after)
                .unwrap_or(t);
            parse_appmsg(&body)
        }
        other => (format!("类型{other}"), String::new()),
    }
}

/// 从 appmsg XML 里抠出（文件名 / 引用原文 / 链接标题），字符串搜索就够，不引 xml 库
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
        _ => ("消息".into(), if title.is_empty() { "(未识别)".into() } else { title }),
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

fn fmt_ts(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

// ==================== CLI ====================

/// 参数解析：flags 带值的有 --data-dir --out-dir --days --limit --type --out(-o)，
/// 其余第一个不带 -- 的参数是群名关键词。
struct Args {
    flags: HashMap<String, String>,
    positional: Option<String>,
}

impl Args {
    /// raw[0] 是子命令（scan/list/show/export），从 raw[1] 开始解析
    fn parse() -> Self {
        let raw: Vec<String> = std::env::args().skip(2).collect();
        const WITH_VALUE: &[&str] = &[
            "--data-dir", "--out-dir", "--days", "--limit", "--type", "-o", "--out",
        ];
        let mut flags = HashMap::new();
        let mut positional = None;
        let mut i = 0;
        while i < raw.len() {
            let a = &raw[i];
            if WITH_VALUE.contains(&a.as_str()) {
                if let Some(v) = raw.get(i + 1) {
                    let key = if a == "-o" { "--out".to_string() } else { a.clone() };
                    flags.insert(key, v.clone());
                }
                i += 2;
            } else if a.starts_with("--") {
                flags.insert(a.clone(), String::new());
                i += 1;
            } else if positional.is_none() {
                positional = Some(a.clone());
                i += 1;
            } else {
                i += 1;
            }
        }
        Self { flags, positional }
    }
    fn val(&self, name: &str) -> Option<String> {
        self.flags.get(name).cloned().filter(|s| !s.is_empty())
    }
}

fn usage() {
    println!(
        "微信群消息提取工具（微信 4.x Windows）\n\
         用法:\n\
           wechat_msg_tool scan                        解密全部库（微信需保持登录）\n\
           wechat_msg_tool list                        列出所有群\n\
           wechat_msg_tool show  <群关键词> [--days N] [--limit N] [--type N]\n\
           wechat_msg_tool export <群关键词> [--days N] -o out.csv\n\
         选项:\n\
           --data-dir <路径>  指定账号目录（默认自动发现，多账号挑最近活跃的）\n\
           --out-dir  <路径>  解密输出目录（默认 <数据目录同级>/wechat_msg_decrypted/<wxid>）\n\
           --days N           只看最近 N 天    --limit N  最多显示条数\n\
           --type N           按类型过滤: 1=文本 3=图片 49=文件/引用 10000=系统"
    );
}

fn pick_store(args: &Args) -> Result<Store> {
    let account = match args.val("--data-dir") {
        Some(d) => PathBuf::from(d),
        None => {
            let mut accs = discover_accounts();
            if accs.is_empty() {
                bail!("没找到 xwechat_files 账号目录，先 scan 或用 --data-dir 指定");
            }
            sort_by_active(&mut accs);
            // 优先挑已经 scan 过（有解密目录）的账号，否则 list/show 会莫名报「解密库不存在」
            let decrypted = accs
                .iter()
                .rev()
                .find(|p| out_dir_for(p).join("message").is_dir())
                .cloned();
            decrypted.unwrap_or_else(|| accs.pop().unwrap())
        }
    };
    let wxid = account.file_name().unwrap().to_string_lossy().to_string();
    let root = args
        .val("--out-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| out_dir_for(&account));
    if !root.join("message").is_dir() {
        bail!(
            "解密库不存在: {}\n先跑一次: wechat_msg_tool scan",
            root.display()
        );
    }
    eprintln!("[*] 账号 {wxid} / 库 {}", root.display());
    Store::open(root)
}

fn match_room(store: &Store, keyword: &str) -> Result<(String, String)> {
    let rooms = store.list_rooms()?;
    let hits: Vec<_> = rooms
        .iter()
        .filter(|(wxid, name, _, _)| wxid.contains(keyword) || name.contains(keyword))
        .collect();
    match hits.len() {
        0 => bail!("没找到群: {keyword}"),
        1 => {
            let (wxid, name, _, _) = hits[0];
            Ok((wxid.clone(), name.clone()))
        }
        _ => {
            eprintln!("[!] 匹配到多个群，换更精确的关键词:");
            for (wxid, name, cnt, _) in &hits {
                eprintln!("    {cnt:>7}  {name}  {wxid}");
            }
            std::process::exit(1);
        }
    }
}

fn days_to_since(days: Option<i64>) -> Option<i64> {
    days.map(|d| chrono::Local::now().timestamp() - d * 86400)
}

fn main() -> Result<()> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.is_empty() {
        usage();
        return Ok(());
    }
    let args = Args::parse();
    let days = args.val("--days").and_then(|s| s.parse().ok());
    let since = days_to_since(days);
    let tf = args.val("--type").and_then(|s| s.parse().ok());

    match raw[0].as_str() {
        "scan" => cmd_scan(
            args.val("--data-dir").map(PathBuf::from),
            args.val("--out-dir").map(PathBuf::from),
        )?,
        "list" => {
            let store = pick_store(&args)?;
            println!("{:>8}  {:<10}  群名", "消息数", "最后消息");
            for (wxid, name, cnt, last) in store.list_rooms()? {
                let l = if last > 0 { fmt_ts(last).get(..10).unwrap_or("-").to_string() } else { "-".into() };
                println!("{cnt:>8}  {l:<10}  {name}  {wxid}");
            }
        }
        "show" => {
            let kw = args
                .positional
                .clone()
                .context("缺群名关键词，例: wechat_msg_tool show 打包数据群")?;
            let store = pick_store(&args)?;
            let (wxid, name) = match_room(&store, &kw)?;
            let msgs = store.load_msgs(&wxid, since, tf)?;
            let limit = args.val("--limit").and_then(|s| s.parse().ok()).unwrap_or(100);
            let start = msgs.len().saturating_sub(limit);
            println!(
                "[*] 群: {name}  范围内 {} 条，显示最近 {} 条\n",
                msgs.len(),
                msgs.len() - start
            );
            for m in &msgs[start..] {
                match m.cat.as_str() {
                    "图片" | "语音" | "视频" | "表情" | "撤回" => {
                        println!("{}  {}: [{}]", fmt_ts(m.ts), m.sender, m.cat)
                    }
                    _ => println!("{}  {}: {}", fmt_ts(m.ts), m.sender, m.text.replace('\n', " ")),
                }
            }
        }
        "export" => {
            let kw = args
                .positional
                .clone()
                .context("缺群名关键词，例: wechat_msg_tool export 打包数据群 -o out.csv")?;
            let out = args
                .val("--out")
                .context("缺 -o 输出路径，例: -o out.csv")?;
            let store = pick_store(&args)?;
            let (wxid, name) = match_room(&store, &kw)?;
            let msgs = store.load_msgs(&wxid, since, tf)?;
            let mut f = std::fs::File::create(&out)?;
            f.write_all(b"\xef\xbb\xbf")?; // UTF-8 BOM，Excel 直接开不乱码
            writeln!(f, "时间,发送者,类别,内容")?;
            for m in &msgs {
                writeln!(
                    f,
                    "{},{},{},{}",
                    fmt_ts(m.ts),
                    csv_escape(&m.sender),
                    csv_escape(&m.cat),
                    csv_escape(&m.text)
                )?;
            }
            println!("[+] 导出 {} 条 -> {out}（群: {name}）", msgs.len());
        }
        _ => usage(),
    }
    Ok(())
}

fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
