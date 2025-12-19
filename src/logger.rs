#[cfg(feature = "console")]
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(feature = "console")]
static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();

#[cfg(feature = "console")]
pub fn init() {
    let _ = LOG_FILE.get_or_init(|| Mutex::new(open_default_log_file()));
}

#[cfg(not(feature = "console"))]
pub fn init() {}

#[cfg(feature = "console")]
pub fn info(message: &str) {
    write_line("INFO", message);
}

#[cfg(feature = "console")]
pub fn warn(message: &str) {
    write_line("WARN", message);
}

#[cfg(feature = "console")]
pub fn error(message: &str) {
    write_line("ERROR", message);
}

#[cfg(feature = "console")]
fn write_line(level: &str, message: &str) {
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let line = format!("[{ts_ms}] {level} {message}\n");

    if let Some(file) = LOG_FILE.get() {
        if let Ok(mut file) = file.lock() {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }

    if level == "ERROR" {
        eprint!("{line}");
    } else {
        print!("{line}");
    }
}

#[cfg(feature = "console")]
fn open_default_log_file() -> File {
    let candidates = [
        log_path_next_to_exe(),
        log_path_in_local_app_data(),
        Some(std::env::temp_dir().join("lol_plugin.log")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if let Ok(file) = open_log_file(&candidate) {
            return file;
        }
    }

    open_log_file(&std::env::temp_dir().join("lol_plugin.log")).expect("failed to create log file")
}

#[cfg(feature = "console")]
fn open_log_file(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    OpenOptions::new().create(true).append(true).open(path)
}

#[cfg(feature = "console")]
fn log_path_next_to_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join("lol_plugin.log"))
}

#[cfg(feature = "console")]
fn log_path_in_local_app_data() -> Option<PathBuf> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(local_app_data)
            .join("lol_plugin")
            .join("lol_plugin.log"),
    )
}
