#[cfg(any(feature = "console", feature = "logging"))]
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(any(feature = "console", feature = "logging"))]
struct LogState {
    file: File,
    path: PathBuf,
}

#[cfg(any(feature = "console", feature = "logging"))]
static LOG_STATE: OnceLock<Mutex<LogState>> = OnceLock::new();

#[cfg(any(feature = "console", feature = "logging"))]
pub fn init() {
    let _ = LOG_STATE.get_or_init(|| Mutex::new(open_default_log_state()));
}

#[cfg(not(any(feature = "console", feature = "logging")))]
pub fn init() {}

#[cfg(any(feature = "console", feature = "logging"))]
pub fn info(message: &str) {
    write_line("INFO", message);
}

#[cfg(any(feature = "console", feature = "logging"))]
pub fn warn(message: &str) {
    write_line("WARN", message);
}

#[cfg(any(feature = "console", feature = "logging"))]
pub fn error(message: &str) {
    write_line("ERROR", message);
}

#[cfg(any(feature = "console", feature = "logging"))]
fn write_line(level: &str, message: &str) {
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let line = format!("[{ts_ms}] {level} {message}\n");

    if let Some(state) = LOG_STATE.get() {
        if let Ok(mut state) = state.lock() {
            let _ = state.file.write_all(line.as_bytes());
            let _ = state.file.flush();
        }
    }

    #[cfg(feature = "console")]
    {
        if level == "ERROR" {
            eprint!("{line}");
        } else {
            print!("{line}");
        }
    }
}

#[cfg(any(feature = "console", feature = "logging"))]
pub fn log_path() -> Option<PathBuf> {
    LOG_STATE
        .get()
        .and_then(|state| state.lock().ok().map(|state| state.path.clone()))
}
#[cfg(any(feature = "console", feature = "logging"))]
fn open_default_log_state() -> LogState {
    let candidates = [
        log_path_next_to_exe(),
        log_path_in_local_app_data(),
        Some(std::env::temp_dir().join("lol_plugin.log")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if let Ok(file) = open_log_file(&candidate) {
            return LogState {
                file,
                path: candidate,
            };
        }
    }

    let fallback = std::env::temp_dir().join("lol_plugin.log");
    let file = open_log_file(&fallback).expect("failed to create log file");
    LogState {
        file,
        path: fallback,
    }
}

#[cfg(any(feature = "console", feature = "logging"))]
fn open_log_file(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    OpenOptions::new().create(true).append(true).open(path)
}

#[cfg(any(feature = "console", feature = "logging"))]
fn log_path_next_to_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join("lol_plugin.log"))
}

#[cfg(any(feature = "console", feature = "logging"))]
fn log_path_in_local_app_data() -> Option<PathBuf> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(local_app_data)
            .join("lol_plugin")
            .join("lol_plugin.log"),
    )
}
