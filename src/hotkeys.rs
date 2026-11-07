//! 全局快捷键：编号、按设置注册、托盘提示。外壳（app.rs）和设置窗口都用这里，彼此不互相依赖。

use crate::plat;
use crate::settings::{self, Hotkey};

/// 注册给平台层的编号；按下时 `App::hotkey` 收到的就是它
pub const CAPTURE: usize = 1;
pub const RECORD: usize = 2;

/// 注册编号 `id` 的快捷键（换掉这个编号原来的）；返回是否成功
pub fn apply(id: usize, hk: Hotkey) -> bool {
    plat::set_hotkey(id, hk.mods, hk.vk)
}

/// 按现在的设置注册两个快捷键；返回没注册上的那些的名字，比如「截图 Alt+Shift+A」
pub fn apply_all() -> Vec<String> {
    let s = settings::get();
    [(CAPTURE, s.hotkey, "截图"), (RECORD, s.record_hotkey, "录屏")]
        .into_iter()
        .filter(|(id, hk, _)| !apply(*id, *hk))
        .map(|(_, hk, what)| format!("{what} {}", hk.name("+")))
        .collect()
}

/// 菜单、提示里显示的快捷键：没注册上时为空
pub fn label(id: usize) -> String {
    let s = settings::get();
    let hk = if id == RECORD { s.record_hotkey } else { s.hotkey };
    if plat::hotkey_ok(id) { hk.name("+") } else { String::new() }
}

pub fn tray_tip() -> String {
    match (label(CAPTURE), label(RECORD)) {
        (c, r) if !c.is_empty() && !r.is_empty() => format!("Jie · 截图 {c} · 录屏 {r}"),
        (c, _) if !c.is_empty() => format!("Jie · 截图 {c}"),
        _ => "Jie · 快捷键没有生效，单击截图".into(),
    }
}

/// 快捷键换了以后更新托盘提示
pub fn refresh_tray() {
    plat::set_tray_tip(&tray_tip());
}
