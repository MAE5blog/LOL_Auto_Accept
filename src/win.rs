use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};

use windows::{
    core::w,
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Shell::{
                Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
                NOTIFYICONDATAW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
                DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW, PostQuitMessage,
                RegisterClassW, SetForegroundWindow, TrackPopupMenu, TranslateMessage, HICON,
                HMENU, HWND_MESSAGE, IDI_APPLICATION, MF_STRING, MSG, TPM_BOTTOMALIGN,
                TPM_LEFTALIGN, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WM_APP, WM_COMMAND,
                WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_RBUTTONUP, WNDCLASSW, WS_OVERLAPPED,
            },
        },
    },
};

static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();

const WM_TRAYICON: u32 = WM_APP + 1;
const TRAY_UID: u32 = 1;
const ID_TRAY_EXIT: usize = 1001;

pub fn run(overrides: crate::lcu::LcuOverrides) -> anyhow::Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let _ = STOP.set(stop.clone());

    let worker_stop = stop.clone();
    let worker_overrides = overrides.clone();
    let worker = std::thread::spawn(move || crate::lcu::run(worker_stop, worker_overrides));

    let mut notify_data = unsafe { init_tray()? };
    unsafe { message_loop() };

    stop.store(true, Ordering::SeqCst);
    let _ = worker.join();

    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &mut notify_data);
    }

    Ok(())
}

unsafe fn init_tray() -> anyhow::Result<NOTIFYICONDATAW> {
    let hinstance = GetModuleHandleW(None)?;

    let class_name = w!("lol_plugin.TrayWindow");
    let wc = WNDCLASSW {
        hInstance: hinstance.into(),
        lpszClassName: class_name,
        lpfnWndProc: Some(wndproc),
        ..Default::default()
    };
    RegisterClassW(&wc);

    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class_name,
        w!("lol_plugin"),
        WS_OVERLAPPED,
        0,
        0,
        0,
        0,
        HWND_MESSAGE,
        HMENU::default(),
        hinstance,
        None,
    )?;

    let icon = LoadIconW(None, IDI_APPLICATION).unwrap_or(HICON::default());

    let mut notify_data = NOTIFYICONDATAW::default();
    notify_data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    notify_data.hWnd = hwnd;
    notify_data.uID = TRAY_UID;
    notify_data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    notify_data.uCallbackMessage = WM_TRAYICON;
    notify_data.hIcon = icon;

    set_tray_tip(&mut notify_data, "LOL Auto Accept");

    if !Shell_NotifyIconW(NIM_ADD, &mut notify_data).as_bool() {
        anyhow::bail!("Shell_NotifyIconW(NIM_ADD) failed");
    }

    Ok(notify_data)
}

unsafe fn message_loop() {
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, HWND::default(), 0, 0).into() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

unsafe fn show_context_menu(hwnd: HWND) {
    let menu = CreatePopupMenu().unwrap_or_default();
    let _ = AppendMenuW(menu, MF_STRING, ID_TRAY_EXIT, w!("退出"));

    let mut point = POINT::default();
    let _ = GetCursorPos(&mut point);

    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_LEFTALIGN,
        point.x,
        point.y,
        0,
        hwnd,
        None,
    );

    let _ = DestroyMenu(menu);
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TRAYICON => {
                let event = lparam.0 as u32;
                if event == WM_RBUTTONUP || event == WM_CONTEXTMENU || event == WM_LBUTTONDBLCLK {
                    show_context_menu(hwnd);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xffff) as usize;
                if id == ID_TRAY_EXIT {
                    if let Some(stop) = STOP.get() {
                        stop.store(true, Ordering::SeqCst);
                    }
                    PostQuitMessage(0);
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn set_tray_tip(data: &mut NOTIFYICONDATAW, tip: &str) {
    let mut tip_utf16: Vec<u16> = tip.encode_utf16().collect();
    tip_utf16.push(0);

    let len = tip_utf16.len().min(data.szTip.len());
    data.szTip[..len].copy_from_slice(&tip_utf16[..len]);
}
