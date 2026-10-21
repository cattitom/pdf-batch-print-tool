//! 文件日志。
//!
//! release 版是 Windows 子系统程序（没有控制台），`eprintln!` 写进虚无——用户
//! 报障时能给的只有一句「报错了」。这里把日志落到
//! `%APPDATA%\<identifier>\logs\app.log`，超过 2MB 轮转一份 `.1`。
//!
//! 级别用环境变量 `PDFPRINT_LOG` 控制（trace/debug/info/warn/error/off，默认 info）。

use log::{LevelFilter, Log, Metadata, Record};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const LOG_NAME: &str = "app.log";

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// 当前日志文件路径（未初始化时为空）。
pub fn log_path() -> PathBuf {
    LOG_PATH.get().cloned().unwrap_or_default()
}

struct FileLogger {
    file: Mutex<Option<std::fs::File>>,
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:<5} {}: {}\n",
            timestamp(),
            record.level(),
            record.target(),
            record.args()
        );

        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        // 写之前先看体积，超限就地轮转：关闭 → 改名 → 重开
        if let Some(file) = guard.as_mut() {
            if file.metadata().map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
                *guard = None;
                let path = log_path();
                rotate(&path);
                *guard = open_append(&path);
            }
        }
        if let Some(file) = guard.as_mut() {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }

    fn flush(&self) {
        if let Some(file) = self.file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = file.flush();
        }
    }
}

fn open_append(path: &Path) -> Option<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

fn rotate(path: &Path) {
    let mut backup = path.as_os_str().to_owned();
    backup.push(".1");
    let _ = std::fs::rename(path, PathBuf::from(backup));
}

/// 初始化日志（幂等，重复调用不会重复注册）。返回日志文件路径。
pub fn init(dir: &Path) -> PathBuf {
    let log_dir = dir.join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let path = log_dir.join(LOG_NAME);

    if path.metadata().map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        rotate(&path);
    }
    let _ = LOG_PATH.set(path.clone());

    let logger = FileLogger { file: Mutex::new(open_append(&path)) };
    // set_boxed_logger 只能成功一次；重复 init 时忽略
    let _ = log::set_boxed_logger(Box::new(logger));
    log::set_max_level(level_from_env());
    path
}

/// 记录 panic 后再交给默认钩子。release 是 `panic = "abort"`，这行往往是唯一线索。
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        default_hook(info);
    }));
}

fn level_from_env() -> LevelFilter {
    match std::env::var("PDFPRINT_LOG")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "trace" => LevelFilter::Trace,
        "debug" => LevelFilter::Debug,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        "off" => LevelFilter::Off,
        _ => LevelFilter::Info,
    }
}

/// 本地时间戳（不引 chrono：只有 Windows 需要，直接用 Win32）。
#[cfg(windows)]
fn timestamp() -> String {
    #[repr(C)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        millisecond: u16,
    }
    unsafe extern "system" {
        fn GetLocalTime(out: *mut SystemTime);
    }
    let mut st = SystemTime {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        millisecond: 0,
    };
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        st.year, st.month, st.day, st.hour, st.minute, st.second, st.millisecond
    )
}

#[cfg(not(windows))]
fn timestamp() -> String {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => format!("epoch+{}.{:03}s", d.as_secs(), d.subsec_millis()),
        Err(_) => "epoch?".to_string(),
    }
}

#[cfg(test)]
mod tests {
    /// 真的落盘：级别、目标、中文消息、时间戳前缀都要在文件里。
    #[test]
    fn writes_records_to_file() {
        let dir = std::env::temp_dir().join(format!("pdfprint_log_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = super::init(&dir);
        log::info!("测试日志 hello");
        log::logger().flush();

        let text = std::fs::read_to_string(&path).expect("日志文件应存在");
        assert!(text.contains("测试日志 hello"), "内容缺失: {text}");
        assert!(text.contains("INFO"), "级别缺失: {text}");
        assert!(text.contains("logging::tests"), "target 缺失: {text}");
        // 时间戳前缀形如 2026-10-21 20:54:01.123
        assert!(
            text.len() > 20 && text.as_bytes()[4] == b'-' && text.as_bytes()[7] == b'-',
            "时间戳格式异常: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}