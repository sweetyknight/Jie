// release 构建不弹控制台；debug 构建保留控制台看日志
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anim;
mod annotate;
mod app;
mod card;
mod hotkeys;
mod image_io;
mod ocr;
mod overlay;
mod pin;
mod plat;
mod record;
mod settings;
mod settings_ui;
mod snap;
mod toast;
mod toolbar;
mod translate;
mod tray;
mod ui;

fn main() {
    plat::run(app::APP);
}
