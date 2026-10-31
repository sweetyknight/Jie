// release 构建不弹控制台；debug 构建保留控制台看日志
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod annotate;
mod app;
mod card;
mod image_io;
mod ocr;
mod overlay;
mod pin;
mod plat;
mod settings;
mod settings_ui;
mod toast;
mod translate;
mod ui;

fn main() {
    plat::run(app::APP);
}
