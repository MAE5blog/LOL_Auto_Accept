use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::Context;

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
                    std::thread::sleep(Duration::from_secs(1));
                    send_chat_command("/fullmute all").context("send /fullmute all")?;
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

    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        match liveclient_activeplayername(client) {
            Ok(Some(_name)) => {
                #[cfg(any(feature = "console", feature = "logging"))]
                log_info!(
                    "ingame fullmute: liveclient ready (activeplayername={})",
                    _name
                );
                #[cfg(not(any(feature = "console", feature = "logging")))]
                log_info!("ingame fullmute: liveclient ready");
                return Ok(true);
            }
            Ok(None) => {}
            Err(_err) => {
                last_err = Some(format!("{_err:?}"));
            }
        }

        if last_log.elapsed() >= Duration::from_secs(3) {
            if let Some(_err) = last_err.as_deref() {
                log_info!("ingame fullmute: waiting for liveclient... (last_err={_err})");
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

fn liveclient_activeplayername(
    client: &reqwest::blocking::Client,
) -> anyhow::Result<Option<String>> {
    let bases = ["https://127.0.0.1:2999", "http://127.0.0.1:2999"];
    for base in bases {
        let url = format!("{base}/liveclientdata/activeplayername");
        let response = match client.get(&url).send() {
            Ok(r) => r,
            Err(_err) => continue,
        };

        if !response.status().is_success() {
            continue;
        }

        let body = response.text().unwrap_or_default();
        if let Ok(name) = serde_json::from_str::<String>(&body) {
            if !name.trim().is_empty() {
                return Ok(Some(name));
            }
        }

        let name = body.trim().trim_matches('"').to_string();
        if !name.is_empty() {
            return Ok(Some(name));
        }
    }

    Ok(None)
}

struct ForegroundProcessInfo {
    exe_name: String,
    #[cfg(any(feature = "console", feature = "logging"))]
    pid: u32,
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
        let _tid = GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
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
            #[cfg(any(feature = "console", feature = "logging"))]
            pid,
            #[cfg(any(feature = "console", feature = "logging"))]
            exe_path,
        }))
    }
}

fn send_chat_command(text: &str) -> anyhow::Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_RETURN;

    log_info!("ingame fullmute: send Enter");
    send_key(VK_RETURN)?;
    std::thread::sleep(Duration::from_millis(200));
    log_info!("ingame fullmute: send text");
    send_text(text)?;
    std::thread::sleep(Duration::from_millis(100));
    log_info!("ingame fullmute: send Enter");
    send_key(VK_RETURN)?;
    Ok(())
}

fn send_text(text: &str) -> anyhow::Result<()> {
    for ch in text.chars() {
        send_char(ch)?;
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn send_char(ch: char) -> anyhow::Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VkKeyScanW, VK_CONTROL, VK_MENU, VK_SHIFT};

    let code = unsafe { VkKeyScanW(ch as u16) };
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
        send_key_down(VK_SHIFT)?;
    }
    if needs_ctrl {
        send_key_down(VK_CONTROL)?;
    }
    if needs_alt {
        send_key_down(VK_MENU)?;
    }

    send_key_press(windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(vk))?;

    if needs_alt {
        send_key_up(VK_MENU)?;
    }
    if needs_ctrl {
        send_key_up(VK_CONTROL)?;
    }
    if needs_shift {
        send_key_up(VK_SHIFT)?;
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

fn send_key(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> anyhow::Result<()> {
    send_key_press(vk)
}

fn send_key_press(
    vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
) -> anyhow::Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP;
    use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_SCANCODE;

    if let Some(scan) = scancode_for_vk(vk) {
        send_inputs(&[
            keyboard_input(
                windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0),
                scan,
                KEYEVENTF_SCANCODE,
            ),
            keyboard_input(
                windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0),
                scan,
                KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP,
            ),
        ])
    } else {
        send_inputs(&[
            keyboard_input(
                vk,
                0,
                windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0),
            ),
            keyboard_input(vk, 0, KEYEVENTF_KEYUP),
        ])
    }
}

fn send_key_down(
    vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
) -> anyhow::Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_SCANCODE;

    if let Some(scan) = scancode_for_vk(vk) {
        send_inputs(&[keyboard_input(
            windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0),
            scan,
            KEYEVENTF_SCANCODE,
        )])
    } else {
        send_inputs(&[keyboard_input(
            vk,
            0,
            windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0),
        )])
    }
}

fn send_key_up(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> anyhow::Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP;
    use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_SCANCODE;

    if let Some(scan) = scancode_for_vk(vk) {
        send_inputs(&[keyboard_input(
            windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0),
            scan,
            KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP,
        )])
    } else {
        send_inputs(&[keyboard_input(vk, 0, KEYEVENTF_KEYUP)])
    }
}

fn scancode_for_vk(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> Option<u16> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VK_TO_VSC};
    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) };
    u16::try_from(scan).ok().filter(|&s| s != 0)
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
