//! 外壳：启动、托盘菜单、全局快捷键、「已经在运行」的提示。把各功能接起来，功能模块之间不互相调用外壳。

use crate::overlay::{self, Mode};
use crate::plat::{self, MenuItem, Point};
use crate::{card, hotkeys, pin, record, settings_ui};

const CMD_CAPTURE: usize = 1;
const CMD_EXIT: usize = 2;
const CMD_CLOSE_PINS: usize = 3;
const CMD_CLOSE_CARDS: usize = 4;
const CMD_SETTINGS: usize = 5;
const CMD_RECORD: usize = 6;

pub const APP: plat::App = plat::App { start, hotkey, tray_click, tray_menu, again, #[cfg(target_os = "macos")] notify };

fn start() {
    crate::settings::load();
    record::cleanup();
    let failed = hotkeys::apply_all();
    plat::add_tray(&hotkeys::tray_tip());
    plat::warm_up();
    if !failed.is_empty() {
        let title = format!("快捷键 {} 没有生效", failed.join("、"));
        plat::notify(&title, "另一个程序已经占用了这个组合键。点这里换一个；换好之前可以单击托盘图标截图。", Some(|| settings_ui::show(true)));
    }
}

/// 录制中按哪个快捷键、点托盘都是停止
fn hotkey(id: usize) {
    if record::active() {
        record::stop();
    } else if id == hotkeys::RECORD {
        overlay::show(Mode::Record);
    } else {
        overlay::show(Mode::Shot);
    }
}

fn tray_click() {
    hotkey(hotkeys::CAPTURE);
}

fn tray_menu(at: Point) {
    let (pins, cards) = (pin::count(), card::count());
    let n = |c: usize| if c > 0 { c.to_string() } else { String::new() };
    let (np, nc) = (n(pins), n(cards));
    let (shot, rec) = (hotkeys::label(hotkeys::CAPTURE), hotkeys::label(hotkeys::RECORD));
    let recording = record::active();
    let items = [
        MenuItem { default: true, ..MenuItem::new(CMD_CAPTURE, "截图", &shot) },
        MenuItem::new(CMD_RECORD, if recording { "停止录屏" } else { "录屏" }, &rec),
        MenuItem::sep(),
        MenuItem { enabled: pins > 0, ..MenuItem::new(CMD_CLOSE_PINS, "关闭所有钉图", &np) },
        MenuItem { enabled: cards > 0, ..MenuItem::new(CMD_CLOSE_CARDS, "关闭所有文字卡片", &nc) },
        MenuItem::sep(),
        MenuItem::new(CMD_SETTINGS, "设置", ""),
        MenuItem::new(CMD_EXIT, "退出", ""),
    ];
    match plat::tray_menu(&items, at) {
        CMD_CAPTURE => hotkey(hotkeys::CAPTURE),
        CMD_RECORD => hotkey(hotkeys::RECORD),
        CMD_CLOSE_PINS => pin::close_all(),
        CMD_CLOSE_CARDS => card::close_all(),
        CMD_SETTINGS => settings_ui::show(false),
        CMD_EXIT => {
            // 正在录的先放弃，临时文件下次启动清掉
            record::discard();
            plat::quit();
        }
        _ => {}
    }
}

/// 又启动了一个 Jie，已在运行的这个提示一下（点了通知不做什么）
fn again() {
    let shot = hotkeys::label(hotkeys::CAPTURE);
    let how = if shot.is_empty() { "单击托盘图标截图".into() } else { format!("按 {shot} 截图") };
    #[cfg(target_os = "macos")]
    let where_ = "右键菜单栏图标打开菜单";
    #[cfg(not(target_os = "macos"))]
    let where_ = "右键托盘图标打开菜单。图标在任务栏右下角，可能收在「^」里";
    plat::notify("Jie 已经在运行", &format!("{how}，{where_}。"), None);
}

#[cfg(target_os = "macos")]
fn notify(title: &str, text: &str, click: Option<fn()>) {
    let mon = plat::monitor_at(plat::cursor_pos());
    crate::toast::show_action(&mon, crate::toast::Kind::Err, &format!("{title}：{text}"), click);
}
