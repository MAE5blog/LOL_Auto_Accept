#![cfg_attr(
    all(windows, not(debug_assertions), not(feature = "console")),
    windows_subsystem = "windows"
)]

#[cfg(windows)]
mod lcu;
#[cfg(windows)]
mod logger;
#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    logger::init();
    logger::info(&format!(
        "lol_plugin start (version={}, pid={})",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    ));

    let overrides = match lcu::LcuOverrides::from_args(std::env::args().skip(1)) {
        Ok(overrides) => overrides,
        Err(_err) => {
            #[cfg(debug_assertions)]
            eprintln!("{_err:?}");
            logger::error(&format!("argument parse error: {_err:?}"));
            return;
        }
    };

    if let Err(_err) = win::run(overrides) {
        #[cfg(debug_assertions)]
        eprintln!("{_err:?}");
        logger::error(&format!("fatal error: {_err:?}"));
    }

    logger::info("lol_plugin exit");
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This program only supports Windows.");
}
