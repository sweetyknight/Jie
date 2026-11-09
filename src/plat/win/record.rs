//! 录屏截屏：录制线程按帧率对区域做 BitBlt，画上鼠标指针，交给回调。
//! 不带 CAPTUREBLT：Win10/11 的 DWM 下不带它也截得到分层窗口（实测），带上它可能让真实的鼠标闪烁。
//! 只截区域里的像素：录屏自己的边框和控制条放在区域外面，截不进来。不得不放进区域的窗口（全屏录制时的控制条）
//! 不用 WDA_EXCLUDEFROMCAPTURE——它让窗口从所有截屏里消失，远程桌面也看不到——而是每帧用放大镜 API 把它从那一小块里
//! 排除掉再截（系统放大镜就是这样不把自己截进去的），贴回帧里，那块照样是活的画面；放大镜用不了时贴它显示前截下的那张。

use super::Bitmap;
use crate::plat::{OnFrame, Rect};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Magnification::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, w};

/// 正在录：`stop` 或者丢掉它就停，等录制线程退出后才返回，之后不会再调回调
pub struct Recording {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Recording {
    pub fn stop(self) {}
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 不录进录屏的窗口：(窗口, 桌面上的位置, 它出现之前那块的 BGRA 像素)
static COVERS: Mutex<Vec<(isize, Rect, Vec<u8>)>> = Mutex::new(Vec::new());

fn covers() -> MutexGuard<'static, Vec<(isize, Rect, Vec<u8>)>> {
    COVERS.lock().unwrap_or_else(|e| e.into_inner())
}

/// 窗口 `hwnd` 将显示在 `r`：先截下那块屏幕（放大镜用不了时贴它），之后开始的录屏每帧把它盖回去。要在窗口显示之前调
pub(super) fn cover(hwnd: isize, r: Rect) -> bool {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let Some(bmp) = Bitmap::new(w, h) else { return false };
    unsafe {
        // 等刚关掉的遮罩从屏幕上消失
        let _ = DwmFlush();
        let screen = GetDC(None);
        let ok = BitBlt(bmp.dc, 0, 0, w, h, Some(screen), r.left, r.top, SRCCOPY).is_ok();
        ReleaseDC(None, screen);
        if !ok {
            return false;
        }
    }
    covers().push((hwnd, r, bmp.pixels().to_vec()));
    true
}

/// 窗口没了，不用再盖
pub(super) fn uncover(hwnd: isize) {
    covers().retain(|c| c.0 != hwnd);
}

/// 把 `px`（区域 `r` 的一帧）里 `c` 那块换成 `src`（`c` 那么大的 BGRA）
fn paste(px: &mut [u8], r: &Rect, c: &Rect, src: &[u8]) {
    let w = r.right - r.left;
    let (l, t, rr, b) = (c.left.max(r.left), c.top.max(r.top), c.right.min(r.right), c.bottom.min(r.bottom));
    let cw = c.right - c.left;
    for y in t..b {
        let s = (((y - c.top) * cw + l - c.left) * 4) as usize;
        let d = (((y - r.top) * w + l - r.left) * 4) as usize;
        let n = ((rr - l) * 4) as usize;
        px[d..d + n].copy_from_slice(&src[s..s + n]);
    }
}

thread_local! {
    /// 放大镜回调交出来的那一块（BGRA，一行宽 × 4）
    static GRABBED: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

unsafe extern "system" fn on_image(_h: HWND, src: *mut core::ffi::c_void, sh: MAGIMAGEHEADER, _d: *mut core::ffi::c_void, _dh: MAGIMAGEHEADER, _u: RECT, _c: RECT, _r: HRGN) -> BOOL {
    // 字节顺序实测是 BGRA（头里的格式写的是 RGBA）；一行可能比宽 × 4 长
    let (w, h, stride) = (sh.width as usize * 4, sh.height as usize, sh.stride as usize);
    if !src.is_null() && stride >= w && sh.cbSize >= stride * h {
        let all = unsafe { std::slice::from_raw_parts(src as *const u8, stride * h) };
        let mut px = Vec::with_capacity(w * h);
        for row in all.chunks(stride) {
            px.extend_from_slice(&row[..w]);
        }
        GRABBED.set(Some(px));
    }
    BOOL(1)
}

unsafe extern "system" fn host_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, m, wp, lp) }
}

/// 放大镜：藏着的宿主窗口里放一个放大镜控件，排除掉那几个窗口截屏幕上的一块。只在建它的线程上用
struct Magnifier {
    host: HWND,
    mag: HWND,
    size: (i32, i32),
}

impl Magnifier {
    fn new(exclude: &[isize]) -> Option<Magnifier> {
        unsafe {
            if !MagInitialize().as_bool() {
                return None;
            }
            let made = Self::windows(exclude);
            if made.is_none() {
                let _ = MagUninitialize();
            }
            made
        }
    }

    unsafe fn windows(exclude: &[isize]) -> Option<Magnifier> {
        unsafe {
            let hinst = GetModuleHandleW(None).ok()?.into();
            // 第二次录屏时类已经注册过了，失败也不要紧
            RegisterClassExW(&WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(host_proc), hInstance: hinst, lpszClassName: w!("Jie.MagHost"), ..Default::default() });
            let host = CreateWindowExW(WS_EX_LAYERED | WS_EX_TOOLWINDOW, w!("Jie.MagHost"), w!(""), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst), None).ok()?;
            let mag = CreateWindowExW(WINDOW_EX_STYLE(0), WC_MAGNIFIER, w!(""), WS_CHILD | WS_VISIBLE, 0, 0, 0, 0, Some(host), None, Some(hinst), None).ok();
            let mut list: Vec<HWND> = exclude.iter().map(|h| HWND(*h as _)).collect();
            let ready = mag.is_some_and(|m| {
                MagSetImageScalingCallback(m, Some(on_image)).as_bool() && MagSetWindowFilterList(m, MW_FILTERMODE_EXCLUDE, list.len() as i32, list.as_mut_ptr()).as_bool()
            });
            match mag {
                Some(mag) if ready => Some(Magnifier { host, mag, size: (0, 0) }),
                _ => {
                    // 控件是宿主的子窗口，跟着一起销毁
                    let _ = DestroyWindow(host);
                    None
                }
            }
        }
    }

    /// 截屏幕上 `r` 这块（排除掉那几个窗口），BGRA；失败是 None
    fn grab(&mut self, r: Rect) -> Option<Vec<u8>> {
        let size = (r.right - r.left, r.bottom - r.top);
        unsafe {
            // 宿主窗口要和放大镜控件一样大：宿主比它小时，截大块会在系统里崩掉（实测 1920×1080）
            if size != self.size {
                for h in [self.host, self.mag] {
                    SetWindowPos(h, None, 0, 0, size.0, size.1, SWP_NOZORDER | SWP_NOACTIVATE).ok()?;
                }
                self.size = size;
            }
            GRABBED.set(None);
            MagSetWindowSource(self.mag, r).as_bool().then_some(())?;
        }
        GRABBED.take().filter(|px| px.len() == (size.0 * size.1 * 4) as usize)
    }
}

impl Drop for Magnifier {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.host);
            let _ = MagUninitialize();
        }
    }
}

/// 开始录区域 `r`（全局桌面物理像素），每秒 `fps` 帧，`cursor`：把鼠标指针画进帧里。截屏失败的帧直接跳过，所以 `on_error` 不会被调（签名和 mac 一样）
pub fn record(r: Rect, fps: u32, cursor: bool, mut on_frame: OnFrame, _on_error: impl FnOnce(String) + Send + 'static) -> Result<Recording, String> {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || h <= 0 {
        return Err("录制区域是空的".into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    // 录的这会儿要盖住的：和区域相交的那些
    let over = |c: &Rect| c.left < r.right && r.left < c.right && c.top < r.bottom && r.top < c.bottom;
    let list: Vec<(isize, Rect, Vec<u8>)> = covers().iter().filter(|c| over(&c.1)).cloned().collect();
    let interval = Duration::from_secs_f64(1.0 / fps.clamp(1, 60) as f64);
    let thread = std::thread::Builder::new()
        .name("jie-record".into())
        .spawn(move || {
            let Some(mut bmp) = Bitmap::new(w, h) else { return };
            let screen = unsafe { GetDC(None) };
            // 有要盖住的窗口才开放大镜；开不出来就一直贴显示前截下的那张
            let mut mag = if list.is_empty() { None } else { Magnifier::new(&list.iter().map(|c| c.0).collect::<Vec<_>>()) };
            let start = Instant::now();
            // 第一帧至少等 50 ms（好几次屏幕刷新）：刚关掉的遮罩、倒计时可能还留在屏幕上
            let mut next = start + interval.max(Duration::from_millis(50));
            loop {
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                }
                if flag.load(Ordering::Acquire) {
                    break;
                }
                let t = start.elapsed();
                if unsafe { BitBlt(bmp.dc, 0, 0, w, h, Some(screen), r.left, r.top, SRCCOPY) }.is_ok() {
                    for (_, c, snap) in &list {
                        let live = mag.as_mut().and_then(|m| m.grab(*c));
                        // 有一次截不到就不再用放大镜，免得每帧都出错
                        if live.is_none() {
                            mag = None;
                        }
                        paste(bmp.pixels_mut(), &r, c, live.as_deref().unwrap_or(snap));
                    }
                    // 指针画在盖回去的画面上面
                    if cursor {
                        unsafe { draw_cursor(bmp.dc, r.left, r.top) };
                    }
                    on_frame(bmp.pixels(), w as usize * 4, t);
                }
                // 这个线程有放大镜的窗口：把发给它们的消息处理掉，别让广播消息的程序等着
                if mag.is_some() {
                    let mut msg = MSG::default();
                    while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                        unsafe { DispatchMessageW(&msg) };
                    }
                }
                // 编码慢时跳过错过的帧，不补：GIF 的帧延迟按时间戳算，回放速度不受影响
                let after = Instant::now();
                while next <= after {
                    next += interval;
                }
            }
            drop(mag);
            unsafe { ReleaseDC(None, screen) };
        })
        .map_err(|e| e.to_string())?;
    Ok(Recording { stop, thread: Some(thread) })
}

/// 把当前的鼠标指针画进帧里（截屏不带指针）；指针藏着时不画
unsafe fn draw_cursor(dc: HDC, ox: i32, oy: i32) {
    unsafe {
        let mut ci = CURSORINFO { cbSize: size_of::<CURSORINFO>() as u32, ..Default::default() };
        if GetCursorInfo(&mut ci).is_err() || ci.flags.0 & CURSOR_SHOWING.0 == 0 || ci.hCursor.is_invalid() {
            return;
        }
        let icon = HICON(ci.hCursor.0);
        let mut ii = ICONINFO::default();
        if GetIconInfo(icon, &mut ii).is_err() {
            return;
        }
        let POINT { x, y } = ci.ptScreenPos;
        let _ = DrawIconEx(dc, x - ox - ii.xHotspot as i32, y - oy - ii.yHotspot as i32, icon, 0, 0, 0, None, DI_NORMAL);
        // GetIconInfo 交出来的两张位图归调用方
        for b in [ii.hbmMask, ii.hbmColor] {
            if !b.is_invalid() {
                let _ = DeleteObject(b.into());
            }
        }
    }
}

