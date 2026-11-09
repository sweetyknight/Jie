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

/// 在工作线程里把文件 `src` 拷成 `dst`，写完回到界面线程出胶囊；成功才调 `then`，窗口由调用方处理。
pub fn copy_async(src: PathBuf, dst: PathBuf, mon: Monitor, then: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        let result = if src == dst { Ok(0) } else { std::fs::copy(&src, &dst) };
        let result = result.map(|_| dst).map_err(|e| e.to_string());
        plat::post(move || saved(result, mon, then));
    });
}

/// 在工作线程里编码并写文件，成功才在界面线程调 `then`；扩展名是 .jpg/.jpeg 就存 JPEG，否则 PNG。
pub fn save_async(img: Image, path: PathBuf, mon: Monitor, then: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        let jpeg = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
        let result = plat::encode(&img, jpeg).and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string()));
        drop(img);
        plat::post(move || saved(result.map(|_| path), mon, then));
    });
}

fn saved(result: Result<PathBuf, String>, mon: Monitor, then: impl FnOnce()) {
    match result {
        Ok(path) => {
            // 只在落盘成功后通知调用方收起结果，失败时原图仍可重试。
            then();
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// 会显示结果窗口和错误提示，单独运行：cargo test save_failure_keeps_result -- --ignored
    #[test]
    #[ignore]
    fn save_failure_keeps_result() {
        let mon = plat::monitor_at(plat::Point::default());
        let mut img = plat::Bitmap::new(120, 80).unwrap();
        img.pixels_mut().fill(255);
        // 结果入口保留原选区尺寸，兼容长图的窗口内滚动模式。
        let region = plat::Rect { left: mon.work.left + 100, top: mon.work.top + 100, right: mon.work.left + 220, bottom: mon.work.top + 180 };
        crate::pin::create_result(img, region, "测试".into());
        let w = *Win::all(plat::Kind::Pin).last().unwrap();
        // 把目录当成文件必然写入失败，不碰用户文件；失败回调不能关闭唯一的结果。
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let result = std::fs::write(&path, []).map(|()| path).map_err(|e| e.to_string());
        assert!(result.is_err());
        saved(result, mon, || w.close());
        assert!(w.alive());
        w.close();
        Win::all(plat::Kind::Toast).into_iter().for_each(Win::close);
    }
}
