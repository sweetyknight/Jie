//! 设置窗口里的系统输入框（EDIT）和快捷键框（msctls_hotkey32）：去掉系统边框，外框由窗口自己画。
//! Tab、Enter、Esc 交给窗口（`Event::FieldKey`），内容变了发 `Event::FieldChanged`，焦点进出时窗口整个重画（外框颜色跟着变）。

use crate::plat::FieldKind;
use crate::plat::Rect;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

/// 输入框里按的 Tab、Enter、Esc 转给窗口：wParam 是键码，lParam 为 1 表示按着 Shift
pub(super) const WM_FIELDKEY: u32 = WM_APP + 0x101;

/// 一个窗口里的输入框和它们用的字体
#[derive(Default)]
pub(super) struct Fields {
    list: Vec<(usize, HWND, FieldKind)>,
    font: HFONT,
    mono: HFONT,
    scale: f32,
    /// 系统输入框里一行字的高度（像素）
    line_h: i32,
    /// 窗口失去激活时焦点所在的输入框，回来时还给它
    pub(super) last_focus: Option<HWND>,
}

impl Drop for Fields {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.font.into());
            let _ = DeleteObject(self.mono.into());
        }
    }
}

impl Fields {
    pub(super) fn get(&self, id: usize) -> Option<HWND> {
        self.list.iter().find(|f| f.0 == id).map(|f| f.1)
    }

    pub(super) fn id_of(&self, h: HWND) -> Option<usize> {
        self.list.iter().find(|f| f.1 == h).map(|f| f.0)
    }

    pub(super) fn add(&mut self, parent: HWND, id: usize, kind: FieldKind, cue: &str) {
        unsafe {
            let hinst = GetModuleHandleW(None).ok().map(|h| h.into());
            let (class, extra) = match kind {
                FieldKind::Hotkey => (HOTKEY_CLASSW, 0),
                FieldKind::ReadOnly => (w!("EDIT"), ES_AUTOHSCROLL | ES_READONLY),
                FieldKind::Password => (w!("EDIT"), ES_AUTOHSCROLL | ES_PASSWORD),
                FieldKind::Text | FieldKind::Hex => (w!("EDIT"), ES_AUTOHSCROLL),
            };
            let style = WS_CHILD | WS_TABSTOP | WINDOW_STYLE(extra as u32);
            let Ok(h) = CreateWindowExW(WINDOW_EX_STYLE::default(), class, w!(""), style, 0, 0, 0, 0, Some(parent), Some(HMENU(id as _)), hinst, None) else { return };
            // 快捷键框创建时自己加了边框，去掉，外框统一自己画
            let st = GetWindowLongPtrW(h, GWL_STYLE) & !(WS_BORDER.0 as isize);
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) & !((WS_EX_CLIENTEDGE.0 | WS_EX_STATICEDGE.0) as isize);
            SetWindowLongPtrW(h, GWL_STYLE, st);
            SetWindowLongPtrW(h, GWL_EXSTYLE, ex);
            let _ = SetWindowSubclass(h, Some(child_proc), 0, 0);
            match kind {
                // 不带修饰键、只带 Shift 的组合自动补上 Ctrl
                FieldKind::Hotkey => _ = SendMessageW(h, HKM_SETRULES, Some(WPARAM((HKCOMB_NONE | HKCOMB_S) as usize)), Some(LPARAM(HOTKEYF_CONTROL as isize))),
                // 色值「#9C44EC」
                FieldKind::Hex => _ = SendMessageW(h, EM_LIMITTEXT, Some(WPARAM(7)), Some(LPARAM(0))),
                _ => {}
            }
            if !cue.is_empty() {
                let cue = HSTRING::from(cue);
                SendMessageW(h, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(cue.as_ptr() as isize)));
            }
            if !self.font.is_invalid() {
                SendMessageW(h, WM_SETFONT, Some(WPARAM(if kind == FieldKind::Hex { self.mono } else { self.font }.0 as usize)), Some(LPARAM(1)));
            }
            self.list.push((id, h, kind));
        }
    }

    /// 按缩放比例建字体：Segoe UI 12，色值框用 Consolas
    fn fonts(&mut self, parent: HWND, scale: f32) {
        if scale == self.scale && !self.font.is_invalid() {
            return;
        }
        unsafe {
            let _ = DeleteObject(self.font.into());
            let _ = DeleteObject(self.mono.into());
            let make = |face: PCWSTR| {
                CreateFontW(-(12.0 * scale).round() as i32, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0, face)
            };
            (self.font, self.mono, self.scale) = (make(w!("Segoe UI")), make(w!("Consolas")), scale);
            let dc = GetDC(Some(parent));
            let old = SelectObject(dc, self.font.into());
            let mut tm = TEXTMETRICW::default();
            let _ = GetTextMetricsW(dc, &mut tm);
            SelectObject(dc, old);
            ReleaseDC(Some(parent), dc);
            self.line_h = tm.tmHeight.max(1);
            for (_, h, kind) in &self.list {
                SendMessageW(*h, WM_SETFONT, Some(WPARAM(if *kind == FieldKind::Hex { self.mono } else { self.font }.0 as usize)), Some(LPARAM(1)));
            }
        }
    }

    /// 放进自绘外框 `frame` 里：左右留 8，一行字的高度上下居中
    pub(super) fn place(&mut self, parent: HWND, id: usize, frame: Rect, scale: f32) {
        self.fonts(parent, scale);
        let Some(h) = self.get(id) else { return };
        let pad = (8.0 * scale).round() as i32;
        let y = frame.top + (frame.bottom - frame.top - self.line_h) / 2;
        let _ = unsafe { SetWindowPos(h, None, frame.left + pad, y, frame.right - frame.left - 2 * pad, self.line_h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED) };
    }
}

pub(super) fn text(h: HWND) -> String {
    unsafe {
        let mut buf = vec![0u16; GetWindowTextLengthW(h) as usize + 1];
        let n = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

pub(super) fn set_text(h: HWND, t: &str) {
    let _ = unsafe { SetWindowTextW(h, &HSTRING::from(t)) };
}

/// 快捷键框里的组合：(MOD_* 修饰键位, 虚拟键码)
pub(super) fn hotkey(h: HWND) -> (u32, u32) {
    let v = unsafe { SendMessageW(h, HKM_GETHOTKEY, Some(WPARAM(0)), Some(LPARAM(0))) }.0 as u32;
    let (vk, f) = (v & 0xFF, (v >> 8) & 0xFF);
    let mods = [(HOTKEYF_SHIFT, MOD_SHIFT), (HOTKEYF_CONTROL, MOD_CONTROL), (HOTKEYF_ALT, MOD_ALT)].iter().filter(|(hf, _)| f & hf != 0).fold(0, |m, (_, md)| m | md.0);
    (mods, vk)
}

pub(super) fn set_hotkey(h: HWND, mods: u32, vk: u32) {
    let mut f = [(MOD_SHIFT, HOTKEYF_SHIFT), (MOD_CONTROL, HOTKEYF_CONTROL), (MOD_ALT, HOTKEYF_ALT)].iter().filter(|(m, _)| mods & m.0 != 0).fold(0, |f, (_, hf)| f | hf);
    if super::extended(vk) {
        f |= HOTKEYF_EXT;
    }
    unsafe { SendMessageW(h, HKM_SETHOTKEY, Some(WPARAM((vk | (f << 8)) as usize)), Some(LPARAM(0))) };
}

/// 系统控件的底色和字色：白底、深灰字
pub(super) unsafe fn ctl_color(dc: HDC) -> LRESULT {
    unsafe {
        SetTextColor(dc, COLORREF(0x1A1A1A));
        SetBkColor(dc, COLORREF(0xFFFFFF));
        LRESULT(GetStockObject(WHITE_BRUSH).0 as isize)
    }
}

/// 输入框和快捷键框的子类：Tab、Enter、Esc 交给窗口处理；焦点变了让窗口重画外框
unsafe extern "system" fn child_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        match msg {
            WM_KEYDOWN => {
                let vk = VIRTUAL_KEY(wp.0 as u16);
                let held = |k: VIRTUAL_KEY| GetKeyState(k.0 as i32) < 0;
                // 快捷键框要能录 Ctrl+Enter 这类组合，带 Ctrl、Alt 的不拦
                if matches!(vk, VK_TAB | VK_RETURN | VK_ESCAPE) && !held(VK_CONTROL) && !held(VK_MENU) {
                    if let Ok(parent) = GetParent(h) {
                        let _ = PostMessageW(Some(parent), WM_FIELDKEY, WPARAM(vk.0 as usize), LPARAM(isize::from(held(VK_SHIFT))));
                    }
                    return LRESULT(0);
                }
            }
            // 上面拦掉的键对应的字符，不拦的话输入框会响一声
            WM_CHAR if matches!(wp.0, 9 | 13 | 27) => return LRESULT(0),
            WM_SETFOCUS | WM_KILLFOCUS => {
                if let Ok(parent) = GetParent(h) {
                    let _ = InvalidateRect(Some(parent), None, false);
                }
            }
            _ => {}
        }
        DefSubclassProc(h, msg, wp, lp)
    }
}
