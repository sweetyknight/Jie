//! 菜单：系统 TrackPopupMenu + 自绘菜单项（悬停浅灰，不出现系统的蓝色高亮）。

use super::{Canvas, Face, RectF, Text};
use crate::plat::MenuItem;
use std::cell::RefCell;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, DeleteObject};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_DISABLED, ODS_GRAYED, ODS_SELECTED, ODT_MENU};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

/// 正在弹出的菜单：(文字, 快捷键, 加粗, 圆点) 和缩放；菜单是模态的，同一时间只有一个
struct MenuState {
    items: Vec<(String, String, bool, bool)>,
    scale: f32,
}

thread_local! {
    static MENU: RefCell<MenuState> = const { RefCell::new(MenuState { items: Vec::new(), scale: 1.0 }) };
}

const MENU_BG: u32 = 0xF9F9F9;

fn rgb(c: u32) -> D2D1_COLOR_F {
    let ch = |s: u32| ((c >> s) & 0xFF) as f32 / 255.0;
    D2D1_COLOR_F { r: ch(16), g: ch(8), b: ch(0), a: 1.0 }
}

/// 在屏幕坐标 (x, y) 弹出菜单，返回选中的 id，没选为 0。
/// `owner` 的窗口过程要把 WM_MEASUREITEM、WM_DRAWITEM 交给 `menu_measure`、`menu_draw`。
pub fn popup_menu(owner: HWND, items: &[MenuItem], x: i32, y: i32) -> usize {
    let scale = super::monitor_at(POINT { x, y }).scale();
    MENU.with_borrow_mut(|m| {
        m.items = items.iter().map(|i| (i.text.to_string(), i.key.to_string(), i.default, i.checked)).collect();
        m.scale = scale;
    });
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        for (i, it) in items.iter().enumerate() {
            let mut flags = MF_OWNERDRAW;
            if it.id == 0 {
                // 真正的分组线：方向键会跳过它
                flags |= MF_SEPARATOR;
            } else if !it.enabled {
                flags |= MF_GRAYED;
            }
            // 自绘项的最后一个参数是 itemData：这里放下标
            let _ = AppendMenuW(menu, flags, it.id, PCWSTR(i as *const u16));
        }
        let bg = CreateSolidBrush(COLORREF(MENU_BG));
        let info = MENUINFO { cbSize: size_of::<MENUINFO>() as u32, fMask: MIM_BACKGROUND, hbrBack: bg, ..Default::default() };
        let _ = SetMenuInfo(menu, &info);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, x, y, None, owner, None);
        let _ = DestroyMenu(menu);
        let _ = DeleteObject(bg.into());
        MENU.with_borrow_mut(|m| m.items.clear());
        cmd.0 as usize
    }
}

/// 菜单项高 30、分组线 9（1 + 上下各 4）；宽度 = 左 32 + 文字 + 32 + 快捷键 + 右 14，至少 200
pub(crate) fn menu_measure(lp: LPARAM) -> bool {
    let mi = unsafe { &mut *(lp.0 as *mut MEASUREITEMSTRUCT) };
    if mi.CtlType != ODT_MENU {
        return false;
    }
    MENU.with_borrow(|m| {
        let s = m.scale;
        let Some((text, key, bold, _)) = m.items.get(mi.itemData) else { return };
        if text.is_empty() {
            mi.itemHeight = (9.0 * s).round() as u32;
            mi.itemWidth = 1;
            return;
        }
        let tw = |t: &str, b: bool| Text::new(Face::Segoe, 12.0 * s, b, t).map_or(0.0, |l| l.width());
        let w = 32.0 * s + tw(text, *bold) + if key.is_empty() { 0.0 } else { 32.0 * s + tw(key, false) } + 14.0 * s;
        mi.itemWidth = w.max(200.0 * s).ceil() as u32;
        mi.itemHeight = (30.0 * s).round() as u32;
    });
    true
}

pub(crate) fn menu_draw(lp: LPARAM) -> bool {
    let di = unsafe { &*(lp.0 as *const DRAWITEMSTRUCT) };
    if di.CtlType != ODT_MENU {
        return false;
    }
    MENU.with_borrow(|m| {
        let s = m.scale;
        let Some((text, key, bold, checked)) = m.items.get(di.itemData) else { return };
        let r = di.rcItem;
        let c = Canvas::dc(di.hDC, r, false);
        let (l, t, rr, b) = (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
        c.clear(rgb(MENU_BG));
        if text.is_empty() {
            let y = ((t + b) / 2.0).floor();
            c.fill_rect(RectF { left: l, top: y, right: rr, bottom: y + s.round().max(1.0) }, rgb(0xE2E2E2));
            return;
        }
        let grayed = di.itemState.0 & (ODS_GRAYED.0 | ODS_DISABLED.0) != 0;
        if di.itemState.0 & ODS_SELECTED.0 != 0 && !grayed {
            c.fill_round(RectF { left: l, top: t, right: rr, bottom: b }, 4.0 * s, rgb(0xEAEAEA));
        }
        let put = |t2: &str, b2: bool, right: bool, color: u32| {
            if let Some(lay) = Text::new(Face::Segoe, 12.0 * s, b2, t2) {
                let x = if right { rr - 14.0 * s - lay.width() } else { l + 32.0 * s };
                c.text(&lay, x, t + (b - t - lay.height()) / 2.0, rgb(color));
            }
        };
        put(text, *bold, false, if grayed { 0xA0A0A0 } else { 0x1A1A1A });
        if *checked {
            c.fill_ellipse(l + 13.0 * s, (t + b) / 2.0, 3.0 * s, 3.0 * s, rgb(0x1A1A1A));
        }
        if !key.is_empty() {
            put(key, false, true, if grayed { 0xA0A0A0 } else { 0x707070 });
        }
    });
    true
}

/// 测试用：另开一个线程等 `owner` 弹出的菜单出现，（给了路径就）把窗口连同菜单截图存下来，再按 `keys`
#[cfg(test)]
pub fn answer_menu(owner: super::Win, shot: Option<std::path::PathBuf>, keys: &'static [crate::plat::Key]) -> std::thread::JoinHandle<()> {
    use crate::plat::Key;
    use windows::Win32::Foundation::{RECT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP};
    use windows::core::w;
    let (target, win) = (owner.hwnd().0 as isize, owner.rect());
    std::thread::spawn(move || unsafe {
        let t0 = std::time::Instant::now();
        let menu = loop {
            if let Ok(m) = FindWindowW(w!("#32768"), None)
                && IsWindowVisible(m).as_bool()
            {
                break m;
            }
            assert!(t0.elapsed() < std::time::Duration::from_secs(20), "菜单没弹出来");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        std::thread::sleep(std::time::Duration::from_millis(300));
        if let Some(p) = shot {
            let mut m = RECT::default();
            let _ = GetWindowRect(menu, &mut m);
            let u = RECT { left: win.left.min(m.left), top: win.top.min(m.top), right: win.right.max(m.right), bottom: win.bottom.max(m.bottom) };
            let (mon, bmp) = super::capture_cursor_monitor().unwrap();
            let o = mon.rect;
            let r = RECT { left: (u.left - o.left).max(0), top: (u.top - o.top).max(0), right: (u.right - o.left).min(bmp.w), bottom: (u.bottom - o.top).min(bmp.h) };
            std::fs::write(p, super::encode(&bmp.crop(&r), false).unwrap()).unwrap();
        }
        for k in keys {
            let vk = match k {
                Key::Up => VK_UP,
                Key::Down => VK_DOWN,
                Key::Esc => VK_ESCAPE,
                _ => VK_RETURN,
            };
            let _ = PostMessageW(Some(HWND(target as _)), WM_KEYDOWN, WPARAM(vk.0 as usize), LPARAM(0));
        }
    })
}
