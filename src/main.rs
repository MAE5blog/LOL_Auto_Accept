#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod lcu;
#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    let overrides = match lcu::LcuOverrides::from_args(std::env::args().skip(1)) {
        Ok(overrides) => overrides,
        Err(_err) => {
            #[cfg(debug_assertions)]
            eprintln!("{_err:?}");
            return;
        }
    };

    if let Err(_err) = win::run(overrides) {
        #[cfg(debug_assertions)]
        eprintln!("{_err:?}");
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This program only supports Windows.");
}
