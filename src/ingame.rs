use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use anyhow::Context;

const MIN_GAME_TIME_SECS: f64 = 2.0;

pub fn try_fullmute_all_after_enter_game(stop: Arc<AtomicBool>) -> anyhow::Result<()> {
    log_info!("ingame fullmute: start");

    let client = build_local_http_client()?;

    let deadline = Instant::now() + Duration::from_secs(180);
    if !wait_for_liveclient(&stop, &client, deadline)? {
        return Ok(());
    }

    log_info!("ingame fullmute: liveclient ready, waiting for game foreground");
    let mut last_foreground_name: Option<String> = None;
    let mut last_log = Instant::now() - Duration::from_secs(10);

    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        match foreground_process() {
            Ok(Some(info)) => {
                if info.exe_name.eq_ignore_ascii_case("League of Legends.exe") {
                    #[cfg(any(feature = "console", feature = "logging"))]
                    log_info!(
                        "ingame fullmute: detected game foreground (pid={}, exe={})",
                        info.pid,
                        info.exe_path.display()
                    );
                    #[cfg(not(any(feature = "console", feature = "logging")))]
                    log_info!("ingame fullmute: detected game foreground");
                    std::thread::sleep(Duration::from_millis(750));
                    #[cfg(any(feature = "console", feature = "logging"))]
                    if let Ok(path) = capture_screen_bmp("fullmute_before") {
                        log_info!("ingame fullmute: screenshot(before) {}", path.display());
                    }
                    match send_chat_command("/fullmute all", info.pid, info.tid)
                        .context("send /fullmute all")
                    {
                        Ok(()) => {}
                        Err(_err) => {
                            log_warn!("ingame fullmute: send failed, will retry: {_err:?}");
                            continue;
                        }
                    }
                    #[cfg(any(feature = "console", feature = "logging"))]
                    if let Ok(path) = capture_screen_bmp("fullmute_after") {
                        log_info!("ingame fullmute: screenshot(after) {}", path.display());
                    }
                    log_info!("ingame fullmute: sent /fullmute all");
                    return Ok(());
                }

                if last_foreground_name.as_deref() != Some(info.exe_name.as_str())
                    || last_log.elapsed() >= Duration::from_secs(2)
                {
                    log_info!(
                        "ingame fullmute: waiting (foreground pid={}, exe_name={}, exe={})",
                        info.pid,
                        info.exe_name,
                        info.exe_path.display()
                    );
                    last_foreground_name = Some(info.exe_name);
                    last_log = Instant::now();
                }
            }
            Ok(None) => {}
            Err(_err) => {
                log_warn!("ingame fullmute: foreground query failed: {_err:?}");
                last_log = Instant::now();
            }
        }

        std::thread::sleep(Duration::from_millis(250));
    }

    log_warn!("ingame fullmute: gave up (timeout or stop requested)");
    Ok(())
}

fn build_local_http_client() -> anyhow::Result<reqwest::blocking::Client> {
    reqwest::blocking::ClientBuilder::new()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .timeout(Duration::from_secs(2))
        .build()
        .context("build local http client")
}

fn wait_for_liveclient(
    stop: &AtomicBool,
    client: &reqwest::blocking::Client,
    deadline: Instant,
) -> anyhow::Result<bool> {
    let mut last_log = Instant::now() - Duration::from_secs(10);
    let mut last_err: Option<String> = None;
    let mut last_time: Option<i64> = None;

    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        match liveclient_game_time_seconds(client) {
            Ok(Some(game_time)) if game_time >= MIN_GAME_TIME_SECS => {
                #[cfg(any(feature = "console", feature = "logging"))]
                log_info!(
                    "ingame fullmute: liveclient ready (gameTime={:.1}s)",
                    game_time
                );
                #[cfg(not(any(feature = "console", feature = "logging")))]
                log_info!("ingame fullmute: liveclient ready");
                return Ok(true);
            }
            Ok(Some(game_time)) => {
                last_time = Some(game_time.floor() as i64);
                last_err = None;
            }
            Ok(None) => {}
            Err(_err) => {
                last_err = Some(format!("{_err:?}"));
            }
        }

        if last_log.elapsed() >= Duration::from_secs(3) {
            if let Some(_err) = last_err.as_deref() {
                log_info!("ingame fullmute: waiting for liveclient... (last_err={_err})");
            } else if let Some(_sec) = last_time {
                log_info!(
                    "ingame fullmute: waiting for game start... (gameTime={}s)",
                    _sec
                );
            } else {
                log_info!("ingame fullmute: waiting for liveclient...");
            }
            last_log = Instant::now();
        }

        std::thread::sleep(Duration::from_secs(1));
    }

    log_warn!("ingame fullmute: liveclient not ready before timeout");
    Ok(false)
}

fn liveclient_game_time_seconds(client: &reqwest::blocking::Client) -> anyhow::Result<Option<f64>> {
    let bases = ["http://127.0.0.1:2999", "https://127.0.0.1:2999"];
    for base in bases {
        let url = format!("{base}/liveclientdata/gamestats");
        let response = match client.get(&url).send() {
            Ok(r) => r,
            Err(_err) => continue,
        };

        if !response.status().is_success() {
            continue;
        }

        let body = response.text().unwrap_or_default();
        let value: serde_json::Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(_err) => continue,
        };
        let Some(game_time) = value.get("gameTime").and_then(|v| v.as_f64()) else {
            continue;
        };
        if game_time.is_finite() && game_time >= 0.0 {
            return Ok(Some(game_time));
        }
    }

    Ok(None)
}

#[cfg(any(feature = "console", feature = "logging"))]
fn capture_screen_bmp(prefix: &str) -> anyhow::Result<std::path::PathBuf> {
    use std::time::SystemTime;

    use windows::Win32::{
        Foundation::HWND,
        Graphics::Gdi::{
            BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
            GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
            DIB_RGB_COLORS, SRCCOPY,
        },
        UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN},
    };

    let out_dir = debug_output_dir();
    let ts_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = out_dir.join(format!("{prefix}_{ts_ms}.bmp"));

    unsafe {
        let width = GetSystemMetrics(SM_CXSCREEN);
        let height = GetSystemMetrics(SM_CYSCREEN);

        if width <= 0 || height <= 0 {
            anyhow::bail!("GetSystemMetrics returned invalid size: {width}x{height}");
        }

        let screen_dc = GetDC(HWND::default());
        if screen_dc.0.is_null() {
            anyhow::bail!("GetDC returned null");
        }

        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.0.is_null() {
            let _ = ReleaseDC(HWND::default(), screen_dc);
            anyhow::bail!("CreateCompatibleDC returned null");
        }

        let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
        if bitmap.0.is_null() {
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(HWND::default(), screen_dc);
            anyhow::bail!("CreateCompatibleBitmap returned null");
        }

        let old = SelectObject(mem_dc, bitmap);
        let ok = BitBlt(mem_dc, 0, 0, width, height, screen_dc, 0, 0, SRCCOPY).is_ok();
        let _ = ReleaseDC(HWND::default(), screen_dc);

        if !ok {
            let _ = SelectObject(mem_dc, old);
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(mem_dc);
            anyhow::bail!("BitBlt failed");
        }

        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: (width * height * 4) as u32,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };

        let pixel_bytes = (width * height * 4) as usize;
        let mut pixels = vec![0u8; pixel_bytes];
        let lines = GetDIBits(
            mem_dc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut info as *mut _,
            DIB_RGB_COLORS,
        );

        let _ = SelectObject(mem_dc, old);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(mem_dc);

        if lines == 0 {
            anyhow::bail!("GetDIBits returned 0");
        }

        let _ = std::fs::create_dir_all(&out_dir);
        let mut file = std::fs::File::create(&path).context("create bmp file")?;
        write_bmp_32bpp(&mut file, width, height, &pixels).context("write bmp")?;

        Ok(path)
    }
}

#[cfg(any(feature = "console", feature = "logging"))]
fn debug_output_dir() -> std::path::PathBuf {
    crate::logger::log_path()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|d| d.to_path_buf()))
        })
        .unwrap_or_else(|| std::env::temp_dir())
}

#[cfg(any(feature = "console", feature = "logging"))]
fn write_bmp_32bpp(
    mut w: impl std::io::Write,
    width: i32,
    height: i32,
    pixels_bgra: &[u8],
) -> anyhow::Result<()> {
    let width_u32: u32 = width.try_into().context("width to u32")?;
    let height_u32: u32 = height.try_into().context("height to u32")?;
    let header_size = 14u32 + 40u32;
    let pixel_size = width_u32
        .checked_mul(height_u32)
        .and_then(|v| v.checked_mul(4))
        .context("pixel size overflow")?;
    let file_size = header_size
        .checked_add(pixel_size)
        .context("file size overflow")?;
    if pixels_bgra.len() != pixel_size as usize {
        anyhow::bail!(
            "unexpected pixel buffer size: got {}, expected {}",
            pixels_bgra.len(),
            pixel_size
        );
    }

    w.write_all(b"BM")?;
    w.write_all(&file_size.to_le_bytes())?;
    w.write_all(&0u16.to_le_bytes())?;
    w.write_all(&0u16.to_le_bytes())?;
    w.write_all(&header_size.to_le_bytes())?;

    w.write_all(&40u32.to_le_bytes())?;
    w.write_all(&width.to_le_bytes())?;
    w.write_all(&(-height).to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?;
    w.write_all(&32u16.to_le_bytes())?;
    w.write_all(&0u32.to_le_bytes())?;
    w.write_all(&pixel_size.to_le_bytes())?;
    w.write_all(&0i32.to_le_bytes())?;
    w.write_all(&0i32.to_le_bytes())?;
    w.write_all(&0u32.to_le_bytes())?;
    w.write_all(&0u32.to_le_bytes())?;

    w.write_all(pixels_bgra)?;
    Ok(())
}

struct ForegroundProcessInfo {
    exe_name: String,
    pid: u32,
    tid: u32,
    #[cfg(any(feature = "console", feature = "logging"))]
    exe_path: std::path::PathBuf,
}

fn foreground_process() -> anyhow::Result<Option<ForegroundProcessInfo>> {
    use windows::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
        UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
    };

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return Ok(None);
        }

        let mut pid = 0u32;
        let tid = GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
        if tid == 0 {
            return Ok(None);
        }
        if pid == 0 {
            return Ok(None);
        }

        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .with_context(|| format!("OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION) pid={pid}"))?;

        let mut buf = vec![0u16; 32 * 1024];
        let mut len: u32 = buf.len().try_into().unwrap_or(u32::MAX);
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .with_context(|| format!("QueryFullProcessImageNameW pid={pid}"))?;

        let _ = CloseHandle(process);

        let exe = String::from_utf16_lossy(&buf[..len as usize]);
        let exe_path = std::path::PathBuf::from(exe);
        let exe_name = exe_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        Ok(Some(ForegroundProcessInfo {
            exe_name,
            pid,
            tid,
            #[cfg(any(feature = "console", feature = "logging"))]
            exe_path,
        }))
    }
}

fn send_chat_command(text: &str, foreground_pid: u32, foreground_tid: u32) -> anyhow::Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, VK_RETURN};

    ensure_foreground_pid(foreground_pid)?;
    let hkl = unsafe { GetKeyboardLayout(foreground_tid) };
    let hkl = if hkl.is_invalid() {
        unsafe { GetKeyboardLayout(0) }
    } else {
        hkl
    };

    #[cfg(any(feature = "console", feature = "logging"))]
    log_info!("ingame fullmute: hkl={:?}", hkl);

    log_info!("ingame fullmute: send Enter");
    send_key(VK_RETURN, hkl)?;
    std::thread::sleep(Duration::from_millis(350));
    #[cfg(any(feature = "console", feature = "logging"))]
    if let Ok(path) = capture_screen_bmp("fullmute_chat_open") {
        log_info!("ingame fullmute: screenshot(chat_open) {}", path.display());
    }
    log_info!("ingame fullmute: send text");
    ensure_foreground_pid(foreground_pid)?;
    send_text(text, hkl, foreground_pid)?;
    std::thread::sleep(Duration::from_millis(250));
    log_info!("ingame fullmute: send Enter");
    ensure_foreground_pid(foreground_pid)?;
    send_key(VK_RETURN, hkl)?;
    Ok(())
}

fn send_text(
    text: &str,
    hkl: windows::Win32::UI::Input::KeyboardAndMouse::HKL,
    foreground_pid: u32,
) -> anyhow::Result<()> {
    for ch in text.chars() {
        ensure_foreground_pid(foreground_pid)?;
        send_char(ch, hkl)?;
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

fn send_char(
    ch: char,
    hkl: windows::Win32::UI::Input::KeyboardAndMouse::HKL,
) -> anyhow::Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        VkKeyScanExW, VK_CONTROL, VK_MENU, VK_SHIFT,
    };

    let code = unsafe { VkKeyScanExW(ch as u16, hkl) };
    if code == -1 {
        return send_unicode_char(ch);
    }

    let vk = (code & 0xff) as u16;
    let shift_state = ((code >> 8) & 0xff) as u8;

    let needs_shift = (shift_state & 1) != 0;
    let needs_ctrl = (shift_state & 2) != 0;
    let needs_alt = (shift_state & 4) != 0;

    if needs_ctrl || needs_alt {
        return send_unicode_char(ch);
    }

    if needs_shift {
        send_key_down(VK_SHIFT, hkl)?;
    }
    if needs_ctrl {
        send_key_down(VK_CONTROL, hkl)?;
    }
    if needs_alt {
        send_key_down(VK_MENU, hkl)?;
    }

    send_key_press(
        windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(vk),
        hkl,
    )?;

    if needs_alt {
        send_key_up(VK_MENU, hkl)?;
    }
    if needs_ctrl {
        send_key_up(VK_CONTROL, hkl)?;
    }
    if needs_shift {
        send_key_up(VK_SHIFT, hkl)?;
    }

    Ok(())
}

fn send_unicode_char(ch: char) -> anyhow::Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{KEYEVENTF_KEYUP, KEYEVENTF_UNICODE};

    let scan = ch as u32;
    let Ok(scan) = u16::try_from(scan) else {
        return Ok(());
    };

    send_inputs(&[
        keyboard_input(
            windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0),
            scan,
            KEYEVENTF_UNICODE,
        ),
        keyboard_input(
            windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0),
            scan,
            KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
        ),
    ])
}

fn send_key(
    vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    hkl: windows::Win32::UI::Input::KeyboardAndMouse::HKL,
) -> anyhow::Result<()> {
    send_key_press(vk, hkl)
}

fn send_key_press(
    vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    hkl: windows::Win32::UI::Input::KeyboardAndMouse::HKL,
) -> anyhow::Result<()> {
    send_key_down(vk, hkl)?;
    std::thread::sleep(Duration::from_millis(30));
    send_key_up(vk, hkl)?;
    Ok(())
}

fn send_key_down(
    vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    hkl: windows::Win32::UI::Input::KeyboardAndMouse::HKL,
) -> anyhow::Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_SCANCODE,
    };

    if let Some(scan) = scancode_for_vk(vk, hkl) {
        let mut flags = KEYEVENTF_SCANCODE;
        if scan.extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        send_inputs(&[keyboard_input(
            windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0),
            scan.code,
            flags,
        )])
    } else {
        send_inputs(&[keyboard_input(
            vk,
            0,
            windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0),
        )])
    }
}

fn send_key_up(
    vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    hkl: windows::Win32::UI::Input::KeyboardAndMouse::HKL,
) -> anyhow::Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    };

    if let Some(scan) = scancode_for_vk(vk, hkl) {
        let mut flags = KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP;
        if scan.extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        send_inputs(&[keyboard_input(
            windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0),
            scan.code,
            flags,
        )])
    } else {
        send_inputs(&[keyboard_input(vk, 0, KEYEVENTF_KEYUP)])
    }
}

#[derive(Clone, Copy)]
struct ScanCode {
    code: u16,
    extended: bool,
}

fn scancode_for_vk(
    vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    hkl: windows::Win32::UI::Input::KeyboardAndMouse::HKL,
) -> Option<ScanCode> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyExW, MAPVK_VK_TO_VSC_EX};
    let scan = unsafe { MapVirtualKeyExW(vk.0 as u32, MAPVK_VK_TO_VSC_EX, hkl) };
    if scan == 0 {
        return None;
    }

    let code = (scan & 0xff) as u16;
    if code == 0 {
        return None;
    }
    let prefix = ((scan >> 8) & 0xff) as u8;
    let extended = prefix == 0xe0 || prefix == 0xe1;
    Some(ScanCode { code, extended })
}

fn keyboard_input(
    vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    scan: u16,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS,
) -> windows::Win32::UI::Input::KeyboardAndMouse::INPUT {
    use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT};

    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_inputs(
    inputs: &[windows::Win32::UI::Input::KeyboardAndMouse::INPUT],
) -> anyhow::Result<()> {
    use windows::Win32::{
        Foundation::GetLastError,
        UI::Input::KeyboardAndMouse::{SendInput, INPUT},
    };

    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        let last = unsafe { GetLastError() };
        anyhow::bail!(
            "SendInput sent {sent}/{} (GetLastError={:?})",
            inputs.len(),
            last
        );
    }
    Ok(())
}

fn ensure_foreground_pid(expected_pid: u32) -> anyhow::Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            anyhow::bail!("no foreground window");
        }

        let mut pid = 0u32;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
        if pid == 0 {
            anyhow::bail!("foreground pid unavailable");
        }
        if pid != expected_pid {
            anyhow::bail!("foreground changed (expected pid={expected_pid}, got pid={pid})");
        }
        Ok(())
    }
}
