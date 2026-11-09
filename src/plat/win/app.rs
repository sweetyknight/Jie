//! 进程级的东西：单实例、隐藏主窗口和消息循环、托盘、全局快捷键、系统通知、把活交回界面线程。

use crate::plat::App;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::time::Duration;
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const WM_TRAY: u32 = WM_APP + 1;
/// 工作线程交回来的活：lParam 是 Box<Job>
const WM_RUN: u32 = WM_APP + 2;
/// 又启动了一个 Jie，新进程发给已在运行的这个
const WM_AGAIN: u32 = WM_APP + 3;
pub(super) const TRAY_CLASS: PCWSTR = w!("Jie.Tray");
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;

type Job = Box<dyn FnOnce() + Send>;

static APP: OnceLock<App> = OnceLock::new();
/// Explorer 重启后会广播这条消息，收到就重新添加托盘图标
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static MAIN_WINDOW: AtomicIsize = AtomicIsize::new(0);
/// 各个全局快捷键（按编号 1–4）现在是否注册成功
static HOTKEY_OK: [AtomicBool; 4] = [const { AtomicBool::new(false) }; 4];
/// 托盘图标加上了没有（以低完整性运行时 Explorer 不让加，这时 Jie 看不见也点不着）
static TRAY_OK: AtomicBool = AtomicBool::new(false);
/// 点了正显示的通知要做的事
static NOTE_CLICK: Mutex<Option<fn()>> = Mutex::new(None);
/// 托盘提示：Explorer 重启后重新加图标时要用
static TIP: Mutex<String> = Mutex::new(String::new());

/// 托盘用的隐藏主窗口：剪贴板的所有者、工作线程交回活的收件人；还没建时无效
pub(crate) fn main_window() -> HWND {
    HWND(MAIN_WINDOW.load(Ordering::Relaxed) as _)
}

fn app() -> &'static App {
    APP.get().expect("plat::run 还没调")
}

/// 程序入口：单实例检查、建隐藏主窗口和托盘、调 `app.start`，然后跑消息循环直到退出
pub fn run(app: App) {
    unsafe {
        // 单实例：同一登录会话里已经有 Jie 在跑，由 take_over 决定谁留下；句柄不关，进程退出时系统回收。
        // 拒绝访问说明已在跑的那个完整性级别更高（以管理员身份运行，或者这边是低完整性），也算已在运行
        let running = match CreateMutexW(None, false, w!("Local\\Jie.SingleInstance")) {
            Ok(_) => GetLastError() == ERROR_ALREADY_EXISTS,
            Err(e) => e.code() == ERROR_ACCESS_DENIED.to_hresult(),
        };
        if running && !take_over() {
            return;
        }
        let _ = APP.set(app);
        // WIC、对话框要用 COM
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let hinst = GetModuleHandleW(None).unwrap().into();
        RegisterClassExW(&WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hIconSm: app_icon(GetSystemMetrics(SM_CXSMICON)),
            lpszClassName: TRAY_CLASS,
            ..Default::default()
        });
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
        // 不显示的顶层窗口：收托盘、快捷键消息和 TaskbarCreated 广播（仅消息窗口收不到广播）
        let hwnd =
            CreateWindowExW(WINDOW_EX_STYLE::default(), TRAY_CLASS, w!("Jie"), WINDOW_STYLE::default(), 0, 0, 0, 0, None, None, Some(hinst), None).unwrap();
        MAIN_WINDOW.store(hwnd.0 as isize, Ordering::Relaxed);
        (self::app().start)();
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 退出：删托盘图标，结束消息循环
pub fn quit() {
    let _ = unsafe { DestroyWindow(main_window()) };
}

/// 已经有 Jie 在运行：问它一声。它的托盘正常，它就提示「已经在运行」，这边退出（返回 false）；
/// 它的托盘图标没加上（看不见也点不着），它就自己退出，这边等它退完接着启动（返回 true）
unsafe fn take_over() -> bool {
    unsafe {
        let Ok(old) = FindWindowW(TRAY_CLASS, None) else { return false };
        let mut r = 0usize;
        // 发不过去（对方完整性级别更高，或者卡死了）就照旧退出
        if SendMessageTimeoutW(old, WM_AGAIN, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 2000, Some(&mut r)).0 == 0 || r != 0 {
            return false;
        }
        for _ in 0..40 {
            if FindWindowW(TRAY_CLASS, None).is_err() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }
}

/// 在界面线程上跑 f（工作线程交回结果用）；程序正在退出时丢掉
pub fn post(f: impl FnOnce() + Send + 'static) {
    let p = Box::into_raw(Box::new(Box::new(f) as Job));
    unsafe {
        if PostMessageW(Some(main_window()), WM_RUN, WPARAM(0), LPARAM(p as isize)).is_err() {
            drop(Box::from_raw(p));
        }
    }
}

/// 资源里的应用图标（方案 C），按给定边长取最合适的一档
pub(crate) fn app_icon(cx: i32) -> HICON {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        let icon = LoadImageW(Some(hinst.into()), PCWSTR(1 as _), IMAGE_ICON, cx, cx, LR_DEFAULTCOLOR);
        HICON(icon.unwrap_or_default().0)
    }
}

/// 换编号 `id`（1–4）的全局快捷键（先注销旧的）；`mods` 是 MOD_ALT 这类位，`vk` 是虚拟键码。返回是否注册成功。
/// 按下时调 `App::hotkey(id)`
pub fn set_hotkey(id: usize, mods: u32, vk: u32) -> bool {
    let Some(slot) = HOTKEY_OK.get(id.wrapping_sub(1)) else { return false };
    let hwnd = main_window();
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), id as i32);
        let ok = RegisterHotKey(Some(hwnd), id as i32, HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT, vk).is_ok();
        slot.store(ok, Ordering::Relaxed);
        ok
    }
}

pub fn hotkey_ok(id: usize) -> bool {
    HOTKEY_OK.get(id.wrapping_sub(1)).is_some_and(|s| s.load(Ordering::Relaxed))
}

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: 1, ..Default::default() }
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let n = dst.len() - 1; // 留一个 0 结尾
    dst.iter_mut().take(n).zip(s.encode_utf16()).for_each(|(d, c)| *d = c);
}

/// 加托盘图标；Explorer 重启后会自己再加一次
pub fn add_tray(tip: &str) {
    *TIP.lock().unwrap() = tip.into();
    let hwnd = main_window();
    let mut nid = tray_data(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = unsafe { HICON(GetClassLongPtrW(hwnd, GCLP_HICONSM) as _) };
    copy_wide(&mut nid.szTip, tip);
    unsafe {
        TRAY_OK.store(Shell_NotifyIconW(NIM_ADD, &nid).as_bool(), Ordering::Relaxed);
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
    }
}

/// 改托盘提示
pub fn set_tray_tip(tip: &str) {
    *TIP.lock().unwrap() = tip.into();
    let mut nid = tray_data(main_window());
    nid.uFlags = NIF_TIP | NIF_SHOWTIP;
    copy_wide(&mut nid.szTip, tip);
    let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

/// 系统通知（托盘气泡，Win10/11 上显示成通知）；`click` 是点了通知要做的事
pub fn notify(title: &str, text: &str, click: Option<fn()>) {
    *NOTE_CLICK.lock().unwrap() = click;
    let mut nid = tray_data(main_window());
    nid.uFlags = NIF_INFO;
    copy_wide(&mut nid.szInfoTitle, title);
    copy_wide(&mut nid.szInfo, text);
    let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // NOTIFYICON_VERSION_4：lParam 低位是事件，wParam 是弹菜单的锚点坐标
            WM_TRAY => match lp.0 as u32 & 0xFFFF {
                NIN_SELECT | NIN_KEYSELECT => (app().tray_click)(),
                WM_CONTEXTMENU => (app().tray_menu)(POINT { x: wp.0 as i16 as i32, y: (wp.0 >> 16) as i16 as i32 }),
                NIN_BALLOONUSERCLICK => {
                    // 先取出来再调，f 里可能又弹通知
                    let f = *NOTE_CLICK.lock().unwrap();
                    if let Some(f) = f {
                        f();
                    }
                }
                _ => {}
            },
            WM_HOTKEY => (app().hotkey)(wp.0),
            WM_RUN => (Box::from_raw(lp.0 as *mut Job))(),
            // 又启动了一个 Jie：托盘正常就提示、回 1，新的那个退出；托盘没加上就关掉自己、回 0，让新的接替
            WM_AGAIN => {
                if !TRAY_OK.load(Ordering::Relaxed) {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    return LRESULT(0);
                }
                (app().again)();
                return LRESULT(1);
            }
            WM_DESTROY => {
                let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data(hwnd));
                PostQuitMessage(0);
            }
            _ if msg == TASKBAR_CREATED.load(Ordering::Relaxed) => {
                let tip = TIP.lock().unwrap().clone();
                add_tray(&tip);
            }
            _ => return DefWindowProcW(hwnd, msg, wp, lp),
        }
        LRESULT(0)
    }
}
