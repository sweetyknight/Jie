//! macOS implementation: AppKit windows, CoreGraphics drawing and native services.

mod app;
mod gfx;
mod io;
mod record;
mod screen;
mod store;
mod window;

pub use app::{add_tray, hotkey_ok, notify, post, quit, run, set_hotkey, set_tray_tip, tray_menu};
pub(crate) use gfx::context_canvas;
pub use gfx::{Affine, Bitmap, Canvas, Color, Face, Path, PathBuilder, RectF, Stroke, Text, warm_up};
pub use io::{ask_folder, ask_save_path, copy_file, copy_image, copy_text, encode, paste_text, scale_down};
pub use record::{Recording, record};
#[cfg(test)]
pub use screen::capture_cursor_monitor;
pub(crate) use screen::{cocoa_frame, physical_rect_for_cocoa};
pub use screen::{cursor_pos, monitor_at, request_capture, window_rects};
pub use store::{KEY_STORE, api_key, autostart, config_dir, key_name, now_stamp, pictures_dir, set_api_key, set_autostart};
pub use window::Win;
#[cfg(test)]
pub use window::pump;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(C)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(C)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}
