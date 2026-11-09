//! AppKit points are bottom-up; handlers use top-down physical pixels within each display.

use super::{Bitmap, Point, Rect};
use crate::plat::{Image, Monitor};
use block2::RcBlock;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSEvent, NSScreen};
use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGContext, CGImage, CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption, kCGColorSpaceSRGB, kCGWindowAlpha,
    kCGWindowBounds, kCGWindowLayer,
};
use objc2_foundation::{NSArray, NSError, NSNumber, ns_string};
use objc2_screen_capture_kit::{SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration};
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
struct ScreenInfo {
    frame: CGRect,
    scale: f64,
    rect: Rect,
    work: Rect,
    id: u32,
}

impl ScreenInfo {
    fn physical(self, r: CGRect) -> Rect {
        let left = self.rect.left + ((r.origin.x - self.frame.origin.x) * self.scale).round() as i32;
        let top = self.rect.top + ((self.frame.origin.y + self.frame.size.height - r.origin.y - r.size.height) * self.scale).round() as i32;
        Rect { left, top, right: left + (r.size.width * self.scale).round() as i32, bottom: top + (r.size.height * self.scale).round() as i32 }
    }
    fn cocoa(self, r: Rect) -> CGRect {
        CGRect::new(
            CGPoint::new(
                self.frame.origin.x + (r.left - self.rect.left) as f64 / self.scale,
                self.frame.origin.y + self.frame.size.height - (r.bottom - self.rect.top) as f64 / self.scale,
            ),
            CGSize::new((r.right - r.left) as f64 / self.scale, (r.bottom - r.top) as f64 / self.scale),
        )
    }
    fn monitor(self) -> Monitor {
        Monitor { rect: self.rect, work: self.work, dpi: (self.scale * 96.0).round() as u32 }
    }
}
fn screens() -> Vec<ScreenInfo> {
    let Some(mtm) = MainThreadMarker::new() else { return Vec::new() };
    let all = NSScreen::screens(mtm);
    let top = all.firstObject().map(|s| s.frame().size.height).unwrap_or(0.0);
    // Separate display origins at the largest scale so mixed-DPI pixel rectangles never overlap.
    let desktop_scale = all.iter().map(|s| s.backingScaleFactor()).fold(1.0, f64::max);
    all.iter()
        .filter_map(|s| {
            let frame = s.frame();
            let scale = s.backingScaleFactor();
            let id = s.deviceDescription().objectForKey(ns_string!("NSScreenNumber"))?.downcast::<NSNumber>().ok()?.unsignedIntValue();
            let left = (frame.origin.x * desktop_scale).round() as i32;
            let y = ((top - frame.origin.y - frame.size.height) * desktop_scale).round() as i32;
            let mut info = ScreenInfo {
                frame,
                scale,
                id,
                rect: Rect { left, top: y, right: left + (frame.size.width * scale).round() as i32, bottom: y + (frame.size.height * scale).round() as i32 },
                work: Rect::default(),
            };
            info.work = info.physical(s.visibleFrame());
            Some(info)
        })
        .collect()
}
fn distance(p: CGPoint, r: CGRect) -> f64 {
    (r.origin.x - p.x).max(0.0).max(p.x - r.origin.x - r.size.width).powi(2) + (r.origin.y - p.y).max(0.0).max(p.y - r.origin.y - r.size.height).powi(2)
}
fn cocoa_info(p: CGPoint) -> Option<ScreenInfo> {
    screens().into_iter().min_by(|a, b| distance(p, a.frame).total_cmp(&distance(p, b.frame)))
}
fn info_at(p: Point) -> Option<ScreenInfo> {
    screens().into_iter().min_by_key(|s| {
        let dx = (s.rect.left as i64 - p.x as i64).max(0).max(p.x as i64 - s.rect.right as i64);
        let dy = (s.rect.top as i64 - p.y as i64).max(0).max(p.y as i64 - s.rect.bottom as i64);
        dx * dx + dy * dy
    })
}
pub(crate) fn cocoa_frame(r: Rect) -> CGRect {
    info_at(Point { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 }).map(|s| s.cocoa(r)).unwrap_or_default()
}
/// 区域所在的显示器编号，和区域在这块显示器上的位置（点，左上角为原点；SCStream 的 sourceRect 用）
pub(crate) fn display_rect(r: Rect) -> Option<(u32, CGRect)> {
    let s = info_at(Point { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 })?;
    let k = s.scale;
    let origin = CGPoint::new((r.left - s.rect.left) as f64 / k, (r.top - s.rect.top) as f64 / k);
    Some((s.id, CGRect::new(origin, CGSize::new((r.right - r.left) as f64 / k, (r.bottom - r.top) as f64 / k))))
}
pub(crate) fn cocoa_point(p: Point) -> CGPoint {
    info_at(p).map(|s| s.cocoa(Rect { left: p.x, top: p.y, right: p.x, bottom: p.y }).origin).unwrap_or_default()
}
pub(crate) fn physical_rect_for_cocoa(r: CGRect) -> Rect {
    cocoa_info(CGPoint::new(r.origin.x + r.size.width / 2.0, r.origin.y + r.size.height / 2.0)).map(|s| s.physical(r)).unwrap_or_default()
}
/// 往屏幕上 `at` 那里发一次滚轮（`delta`：一格 120，往上为正），一格换成 3 行。
/// 合成事件要「辅助功能」权限：没有就弹系统的请求、返回错误（用户自己滚滚轮、触控板不受影响）
pub fn scroll(at: Point, delta: i32) -> Result<(), String> {
    use objc2_core_graphics::{CGEvent, CGEventTapLocation, CGPreflightPostEventAccess, CGRequestPostEventAccess, CGScrollEventUnit};
    if !CGPreflightPostEventAccess() {
        CGRequestPostEventAccess();
        return Err("按方向键滚动要在「系统设置 → 隐私与安全性 → 辅助功能」里允许 Jie".into());
    }
    let mtm = MainThreadMarker::new().ok_or("不在主线程")?;
    let lines = if delta.abs() >= 40 { delta * 3 / 120 } else { delta.signum() };
    let ev = CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Line, 1, lines, 0, 0).ok_or("建不了滚轮事件")?;
    // CGEvent 的坐标是点，原点在主显示器左上角（cocoa 的原点在左下角）
    let top = NSScreen::screens(mtm).firstObject().map(|s| s.frame().size.height).unwrap_or(0.0);
    let p = cocoa_point(at);
    CGEvent::set_location(Some(&ev), CGPoint::new(p.x, top - p.y));
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
    Ok(())
}
pub fn monitor_at(p: Point) -> Monitor {
    info_at(p).map(|s| s.monitor()).unwrap_or(Monitor { rect: Rect::default(), work: Rect::default(), dpi: 96 })
}
pub fn cursor_pos() -> Point {
    if MainThreadMarker::new().is_none() {
        return Point::default();
    }
    let p = NSEvent::mouseLocation();
    let r = cocoa_info(p).map(|s| s.physical(CGRect::new(p, CGSize::new(0.0, 0.0)))).unwrap_or_default();
    Point { x: r.left, y: r.top }
}

type Completion = Arc<Mutex<Option<Box<dyn FnOnce(Result<Image, String>) + Send>>>>;
fn finish(done: &Completion, result: Result<Image, String>) {
    let f = done.lock().unwrap().take();
    if let Some(f) = f {
        f(result);
    }
}
fn capture(s: ScreenInfo, ready: impl FnOnce(Result<Image, String>) + Send + 'static) {
    let done: Completion = Arc::new(Mutex::new(Some(Box::new(ready))));
    let timeout = done.clone();
    let _ = dispatch2::DispatchQueue::main().after(dispatch2::DispatchTime::NOW.time(10_000_000_000), move || finish(&timeout, Err("截屏超时，请重试".into())));
    let content = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        let Some(content) = (unsafe { content.as_ref() }) else {
            finish(&done, Err(error(err)));
            return;
        };
        let displays = unsafe { content.displays() };
        let Some(display) = displays.iter().find(|d| unsafe { d.displayID() } == s.id) else {
            finish(&done, Err("显示器已断开".into()));
            return;
        };
        let filter = unsafe { SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &NSArray::new()) };
        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            config.setWidth((s.rect.right - s.rect.left) as usize);
            config.setHeight((s.rect.bottom - s.rect.top) as usize);
            config.setShowsCursor(false);
            config.setColorSpaceName(kCGColorSpaceSRGB);
        }
        let done = done.clone();
        let shot = RcBlock::new(move |image: *mut CGImage, err: *mut NSError| {
            let result = unsafe { image.as_ref() }
                .and_then(|img| {
                    let w = i32::try_from(CGImage::width(Some(img))).ok()?;
                    let h = i32::try_from(CGImage::height(Some(img))).ok()?;
                    let bmp = Bitmap::new(w, h)?;
                    CGContext::draw_image(Some(&bmp.ctx), CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64)), Some(img));
                    Some(bmp.crop(&bmp.full()))
                })
                .ok_or_else(|| error(err));
            finish(&done, result);
        });
        unsafe {
            SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(&filter, &config, Some(&shot));
        }
    });
    unsafe {
        SCShareableContent::getShareableContentWithCompletionHandler(&content);
    }
}
fn error(err: *mut NSError) -> String {
    unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string()).unwrap_or_else(|| "无法取得屏幕图像".into())
}
fn permission() -> bool {
    if objc2_core_graphics::CGPreflightScreenCaptureAccess() {
        return true;
    }
    objc2_core_graphics::CGRequestScreenCaptureAccess();
    super::notify("需要屏幕录制权限", "请在系统设置的「隐私与安全性 → 屏幕录制」允许 Jie，再重新打开", None);
    false
}
pub fn request_capture(ready: impl FnOnce(Option<(Monitor, Bitmap)>) + Send + 'static) {
    if !permission() {
        ready(None);
        return;
    }
    let Some(s) = info_at(cursor_pos()) else {
        ready(None);
        return;
    };
    capture(s, move |result| {
        super::post(move || match result {
            Ok(image) => {
                let Some(mut bmp) = Bitmap::new(image.w, image.h) else {
                    ready(None);
                    return;
                };
                bmp.pixels_mut().copy_from_slice(&image.px);
                let mut mon = s.monitor();
                mon.rect.right = mon.rect.left + image.w;
                mon.rect.bottom = mon.rect.top + image.h;
                ready(Some((mon, bmp)));
            }
            Err(e) => {
                super::notify("截屏失败", &e, None);
                ready(None);
            }
        })
    });
}

#[cfg(test)]
pub fn capture_cursor_monitor() -> Option<(Monitor, Bitmap)> {
    if !permission() {
        return None;
    }
    let s = info_at(cursor_pos())?;
    let (tx, rx) = std::sync::mpsc::channel();
    capture(s, move |r| {
        let _ = tx.send(r);
    });
    let image = rx.recv_timeout(std::time::Duration::from_secs(10)).ok()?.ok()?;
    let mut bmp = Bitmap::new(image.w, image.h)?;
    bmp.pixels_mut().copy_from_slice(&image.px);
    Some((s.monitor(), bmp))
}
pub fn window_rects(mon: &Monitor) -> Vec<Rect> {
    let Some(s) = info_at(Point { x: mon.rect.left, y: mon.rect.top }) else { return Vec::new() };
    let Some(mtm) = MainThreadMarker::new() else { return Vec::new() };
    let top = NSScreen::screens(mtm).firstObject().map(|s| s.frame().size.height).unwrap_or(0.0);
    let Some(list) = CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements, 0) else {
        return Vec::new();
    };
    let array: &CFArray = &list;
    let mut out = Vec::new();
    for i in 0..array.count() {
        let ptr = unsafe { array.value_at_index(i) };
        if ptr.is_null() {
            continue;
        }
        let dict = unsafe { &*(ptr as *const CFDictionary) };
        let layer = unsafe { dict.value(kCGWindowLayer as *const _ as *const c_void) };
        if layer.is_null() || unsafe { (&*(layer as *const CFNumber)).as_i32() } != Some(0) {
            continue;
        }
        // 全透明的窗口看不见，却会挡住下面真正的窗口
        let alpha = unsafe { dict.value(kCGWindowAlpha as *const _ as *const c_void) };
        if !alpha.is_null() && unsafe { (&*(alpha as *const CFNumber)).as_f64() }.is_some_and(|a| a <= 0.0) {
            continue;
        }
        let bounds = unsafe { dict.value(kCGWindowBounds as *const _ as *const c_void) };
        if bounds.is_null() {
            continue;
        }
        let mut r = CGRect::default();
        if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(&*(bounds as *const CFDictionary)), &mut r) } {
            continue;
        }
        r.origin.y = top - r.origin.y - r.size.height; // CGWindow uses Quartz's top-left point coordinates.
        let r = s.physical(r);
        let local = Rect {
            left: r.left.max(mon.rect.left) - mon.rect.left,
            top: r.top.max(mon.rect.top) - mon.rect.top,
            right: r.right.min(mon.rect.right) - mon.rect.left,
            bottom: r.bottom.min(mon.rect.bottom) - mon.rect.top,
        };
        if local.right - local.left > 8 && local.bottom - local.top > 8 {
            out.push(local);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_scale_and_negative_origins_round_trip() {
        for (x, y, k, left, top) in [(0.0, 0.0, 2.0, 0, 0), (-1920.0, 0.0, 1.0, -3840, 100), (100.0, 1200.0, 2.0, 200, -2160)] {
            let s = ScreenInfo {
                frame: CGRect::new(CGPoint::new(x, y), CGSize::new(1920.0, 1080.0)),
                scale: k,
                rect: Rect { left, top, right: left + (1920.0 * k) as i32, bottom: top + (1080.0 * k) as i32 },
                work: Rect::default(),
                id: 1,
            };
            let r = Rect { left: left + 30, top: top + 50, right: left + 300, bottom: top + 450 };
            assert_eq!(s.physical(s.cocoa(r)), r);
            assert_eq!(s.physical(s.frame), s.rect);
        }
    }
}
