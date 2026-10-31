//! 外壳：启动、托盘菜单、全局快捷键、「已经在运行」的提示。

use crate::plat::{self, MenuItem, Point};
use crate::{card, overlay, pin, settings, settings_ui};

const CMD_CAPTURE: usize = 1;
const CMD_EXIT: usize = 2;
const CMD_CLOSE_PINS: usize = 3;
const CMD_CLOSE_CARDS: usize = 4;
const CMD_SETTINGS: usize = 5;

pub const APP: plat::App = plat::App { start, capture: overlay::show, tray_menu, again, #[cfg(target_os = "macos")] notify };

fn start() {
    settings::load();
    let hk = settings::get().hotkey;
    set_hotkey(hk);
    plat::add_tray(&tray_tip());
    plat::warm_up();
    if !plat::hotkey_ok() {
        let title = format!("快捷键 {} 没有生效", hk.name("+"));
        plat::notify(&title, "另一个程序已经占用了这个组合键。点这里换一个；换好之前可以单击托盘图标截图。", Some(|| settings_ui::show(true)));
    }
}

/// 换全局快捷键；返回是否注册成功
pub fn set_hotkey(hk: settings::Hotkey) -> bool {
    plat::set_hotkey(hk.mods, hk.vk)
}

fn tray_tip() -> String {
    if plat::hotkey_ok() { format!("Jie · 截图 {}", settings::get().hotkey.name("+")) } else { "Jie · 快捷键没有生效，单击截图".into() }
}

/// 快捷键换了以后更新托盘提示
pub fn refresh_tray() {
    plat::set_tray_tip(&tray_tip());
}

fn tray_menu(at: Point) {
    let (pins, cards) = (pin::count(), card::count());
    let n = |c: usize| if c > 0 { c.to_string() } else { String::new() };
    let (np, nc, key) = (n(pins), n(cards), settings::get().hotkey.name("+"));
    let items = [
        MenuItem { default: true, ..MenuItem::new(CMD_CAPTURE, "截图", if plat::hotkey_ok() { &key } else { "" }) },
        MenuItem::sep(),
        MenuItem { enabled: pins > 0, ..MenuItem::new(CMD_CLOSE_PINS, "关闭所有钉图", &np) },
        MenuItem { enabled: cards > 0, ..MenuItem::new(CMD_CLOSE_CARDS, "关闭所有文字卡片", &nc) },
        MenuItem::sep(),
        MenuItem::new(CMD_SETTINGS, "设置", ""),
        MenuItem::new(CMD_EXIT, "退出", ""),
    ];
    match plat::tray_menu(&items, at) {
        CMD_CAPTURE => overlay::show(),
        CMD_CLOSE_PINS => pin::close_all(),
        CMD_CLOSE_CARDS => card::close_all(),
        CMD_SETTINGS => settings_ui::show(false),
        CMD_EXIT => plat::quit(),
        _ => {}
    }
}

/// 又启动了一个 Jie，已在运行的这个提示一下（点了通知不做什么）
fn again() {
    let how = if plat::hotkey_ok() { format!("按 {} 截图", settings::get().hotkey.name("+")) } else { "单击托盘图标截图".into() };
    #[cfg(target_os = "macos")]
    let where_ = "右键菜单栏图标打开菜单";
    #[cfg(not(target_os = "macos"))]
    let where_ = "右键托盘图标打开菜单。图标在任务栏右下角，可能收在「^」里";
    plat::notify("Jie 已经在运行", &format!("{how}，{where_}。"), None);
}

#[cfg(target_os = "macos")]
fn notify(title: &str, text: &str, click: Option<fn()>) {
    let mon=plat::monitor_at(plat::cursor_pos());
    crate::toast::show_action(&mon,crate::toast::Kind::Err,&format!("{title}：{text}"),click);
}
