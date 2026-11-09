//! Windows 实现：Win32、GDI、Direct2D/DirectWrite、WIC、凭据管理器、注册表。

mod app;
mod bitmap;
mod field;
mod gfx;
mod io;
mod menu;
mod record;
mod screen;
mod store;
mod window;

pub use app::{add_tray, hotkey_ok, notify, post, quit, run, set_hotkey, set_tray_tip};
pub(crate) use app::{app_icon, main_window};
pub use bitmap::Bitmap;
pub use gfx::{Affine, Canvas, Color, Face, Path, PathBuilder, RectF, Stroke, Text, warm_up};
pub(crate) use menu::{menu_draw, menu_measure, popup_menu};
#[cfg(test)]
pub use menu::answer_menu;
pub use io::{ask_folder, ask_save_path, copy_file, copy_image, copy_text, encode, paste_text, scale_down};
pub use record::{Recording, record};
pub use screen::{capture_cursor_monitor, cursor_pos, monitor_at, window_rects};
pub use window::Win;
#[cfg(test)]
pub use window::pump;
pub use store::{KEY_STORE, api_key, autostart, config_dir, extended, key_name, now_stamp, pictures_dir, set_api_key, set_autostart};

/// 整数矩形、点：直接用 Win32 的，字段名和 mac 上的同名结构一样
pub type Rect = windows::Win32::Foundation::RECT;
pub type Point = windows::Win32::Foundation::POINT;
