#![cfg_attr(all(windows, not(feature = "console")), windows_subsystem = "windows")]

#[cfg(feature = "console")]
macro_rules! log_info {
    ($($arg:tt)*) => {
        crate::logger::info(&format!($($arg)*));
    };
}

#[cfg(not(feature = "console"))]
macro_rules! log_info {
    ($($arg:tt)*) => {};
}

#[cfg(feature = "console")]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        crate::logger::warn(&format!($($arg)*));
    };
}

#[cfg(not(feature = "console"))]
macro_rules! log_warn {
    ($($arg:tt)*) => {};
}

#[cfg(feature = "console")]
macro_rules! log_error {
    ($($arg:tt)*) => {
        crate::logger::error(&format!($($arg)*));
    };
}

#[cfg(not(feature = "console"))]
macro_rules! log_error {
    ($($arg:tt)*) => {};
}

#[cfg(windows)]
mod lcu;
#[cfg(windows)]
mod logger;
#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    logger::init();
    log_info!(
        "lol_plugin start (version={}, pid={})",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    );

    let overrides = match lcu::LcuOverrides::from_args(std::env::args().skip(1)) {
        Ok(overrides) => overrides,
        Err(_err) => {
            #[cfg(debug_assertions)]
            eprintln!("{_err:?}");
            log_error!("argument parse error: {_err:?}");
            return;
        }
    };

    if let Err(_err) = win::run(overrides) {
        #[cfg(debug_assertions)]
        eprintln!("{_err:?}");
        log_error!("fatal error: {_err:?}");
    }

    log_info!("lol_plugin exit");
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This program only supports Windows.");
}
