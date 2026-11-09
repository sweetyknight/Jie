//! 外壳：启动、托盘面板、全局快捷键、「已经在运行」的提示。把各功能接起来，功能模块之间不互相调用外壳。

use crate::overlay::{self, Mode};
use crate::plat::{self, Point};
use crate::{card, hotkeys, longshot, pin, record, settings_ui, tray};

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

/// 长截图时按键交给它（↑↓ Esc 和别的不一样）；录制中按哪个快捷键、点托盘都是停止
fn hotkey(id: usize) {
    if longshot::active() {
        longshot::key(id);
    } else if id > hotkeys::RECORD {
        // 长截图刚结束、还在路上的 ↑↓ Esc Enter
    } else if record::active() {
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

/// 托盘面板（右键托盘图标）：数据从各模块现取，点了什么回到 `tray_cmd`
fn tray_menu(at: Point) {
    let model = tray::Model {
        shot_key: hotkeys::label(hotkeys::CAPTURE),
        rec_key: hotkeys::label(hotkeys::RECORD),
        recording: record::active(),
        rec_since: record::started_at(),
        pins: pin::count(),
        cards: card::count(),
    };
    tray::show(at, model, tray_cmd);
}

fn tray_cmd(c: tray::Cmd) {
    match c {
        tray::Cmd::Capture => hotkey(hotkeys::CAPTURE),
        tray::Cmd::Record => hotkey(hotkeys::RECORD),
        tray::Cmd::ClosePins => pin::close_all(),
        tray::Cmd::CloseCards => card::close_all(),
        tray::Cmd::Settings => settings_ui::show(false),
        tray::Cmd::Quit => {
            // 正在录的先放弃，临时文件下次启动清掉
            record::discard();
            longshot::cancel();
            plat::quit();
        }
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
