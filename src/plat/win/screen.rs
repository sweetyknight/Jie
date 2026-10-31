//! 显示器、截屏（GDI BitBlt，物理像素）、窗口列表（吸附用）、鼠标位置。

use super::Bitmap;
use crate::plat::Monitor;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::BOOL;

/// 点所在的显示器（不在任何显示器上时取最近的）
pub fn monitor_at(pt: POINT) -> Monitor {
    unsafe {
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let (mut dpi, mut dy) = (96, 96);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi, &mut dy);
        Monitor { rect: mi.rcMonitor, work: mi.rcWork, dpi }
    }
}

/// 鼠标在全局桌面上的位置
pub fn cursor_pos() -> POINT {
    let mut p = POINT::default();
    let _ = unsafe { GetCursorPos(&mut p) };
    p
}

/// 截鼠标所在的显示器
pub fn capture_cursor_monitor() -> Option<(Monitor, Bitmap)> {
    unsafe {
        let m = monitor_at(cursor_pos());
        let r = m.rect;
        let bmp = Bitmap::new(r.right - r.left, r.bottom - r.top)?;
        let screen = GetDC(None);
        // CAPTUREBLT 才能截到分层窗口（胶囊、别的程序的半透明窗口）
        let ok = BitBlt(bmp.dc, 0, 0, bmp.w, bmp.h, Some(screen), r.left, r.top, SRCCOPY | CAPTUREBLT);
        ReleaseDC(None, screen);
        ok.ok()?;
        let _ = GdiFlush();
        Some((m, bmp))
    }
}

/// 吸附用的窗口列表：从上到下的可见顶层窗口，矩形换成相对显示器左上角并裁到显示器里
pub fn window_rects(mon: &Monitor) -> Vec<RECT> {
    struct Acc {
        mon: RECT,
        out: Vec<RECT>,
    }
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
        unsafe {
            let acc = &mut *(lp.0 as *mut Acc);
            if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
                return TRUE;
            }
            // 鼠标穿透的多是全屏覆盖层（显卡、录屏软件的浮层），吸上去没意义
            if GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0 != 0 {
                return TRUE;
            }
            // 被系统藏起来的：别的虚拟桌面上的、挂起的商店应用
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut _ as _, 4);
            if cloaked != 0 {
                return TRUE;
            }
            // 实际边框，不含 Win10/11 窗口外面那圈看不见的拖拽边
            let mut r = RECT::default();
            if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as _, size_of::<RECT>() as u32).is_err() {
                let _ = GetWindowRect(h, &mut r);
            }
            let mut c = RECT::default();
            if IntersectRect(&mut c, &r, &acc.mon).as_bool() {
                let (dx, dy) = (acc.mon.left, acc.mon.top);
                acc.out.push(RECT { left: c.left - dx, top: c.top - dy, right: c.right - dx, bottom: c.bottom - dy });
            }
            TRUE
        }
    }
    let mut acc = Acc { mon: mon.rect, out: Vec::new() };
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut acc as *mut _ as isize)) };
    acc.out
}
