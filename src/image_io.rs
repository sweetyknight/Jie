//! 保存截图和录屏动图：对话框的起始位置和默认文件名，在工作线程里编码（或拷贝）写文件，写完出胶囊。

use crate::plat::{self, Image, Monitor, SaveAs, Win};
use crate::settings;
use crate::toast::{self, Kind};
use std::path::PathBuf;

/// 系统保存对话框（`w` 是它的主人）：格式按设置，默认文件名 Jie_yyyyMMdd_HHmmss，从设置里的保存位置开始。
/// 选好了把路径交给 `then`，取消就什么都不做
pub fn ask_save(w: Win, then: impl FnOnce(PathBuf) + 'static) {
    let s = settings::get();
    let kind = if s.save_format == settings::Format::Jpeg { SaveAs::Jpeg } else { SaveAs::Png };
    w.ask_save(s.save_dir(), format!("Jie_{}", plat::now_stamp()), kind, move |p| {
        if let Some(p) = p {
            then(p);
        }
    });
}

/// 保存动图的对话框：只能存 GIF，起始位置和默认文件名同截图
pub fn ask_save_gif(w: Win, then: impl FnOnce(PathBuf) + 'static) {
    w.ask_save(settings::get().save_dir(), format!("Jie_{}", plat::now_stamp()), SaveAs::Gif, move |p| {
        if let Some(p) = p {
            then(p);
        }
    });
}

/// 在工作线程里把文件 `src` 拷成 `dst`（录屏的动图已经写好在临时文件里），写完回到界面线程出胶囊
pub fn copy_async(src: PathBuf, dst: PathBuf, mon: Monitor) {
    std::thread::spawn(move || {
        let result = if src == dst { Ok(0) } else { std::fs::copy(&src, &dst) };
        let result = result.map(|_| dst).map_err(|e| e.to_string());
        plat::post(move || saved(result, mon));
    });
}

/// 在工作线程里编码并写文件，写完回到界面线程出胶囊；扩展名是 .jpg/.jpeg 就存 JPEG，否则 PNG
pub fn save_async(img: Image, path: PathBuf, mon: Monitor) {
    std::thread::spawn(move || {
        let jpeg = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
        let result = plat::encode(&img, jpeg).and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string()));
        drop(img);
        plat::post(move || saved(result.map(|_| path), mon));
    });
}

fn saved(result: Result<PathBuf, String>, mon: Monitor) {
    match result {
        Ok(path) => {
            // 保存成功的文件夹记成下次的起始位置
            if let Some(dir) = path.parent() {
                settings::remember_save_dir(dir);
            }
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            toast::show(&mon, Kind::Ok, &format!("已保存 {name}"));
        }
        Err(e) => toast::show(&mon, Kind::Err, &format!("保存失败：{e}")),
    }
}
