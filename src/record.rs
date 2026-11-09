//! 录屏：选好的区域外画一圈边框（选区颜色），旁边放控制条，马上开始截（选了延时就先在区域中间倒数）；平台层按帧率截这块活的屏幕，
//! 截屏线程上边截边编成 GIF 写进临时文件。停止后把动图交给钉图（原位置循环播放，下面是结果条），底部胶囊报时长和大小。
//! 边框和控制条靠放在区域外面录不进去，远程桌面、会议共享也看得到。贴着屏幕边、外面画不下的那条边框不画；
//! 控制条外面放不下（框满整块屏幕）就放在区域里的任务栏上，设成不被截屏：照样看得到，录的时候平台层把它那块换成它底下的实时画面。
//! 只依赖平台层、动图编码（anim）、钉图、胶囊、快捷键和设置；不知道遮罩和外壳，选区由调用方给。

use crate::anim::Writer;
use crate::hotkeys;
use crate::pin;
use crate::plat::{self, Button, Canvas, Color, Cursor, Event, Face, Handler, Monitor, Point, Rect, Spec, Text, Win};
use crate::settings;
use crate::toast::{self, Kind};
use crate::ui::{self, Hud, rectf, rgb};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 控制条、倒计时的刷新定时器（250ms 一次：倒数、计时、圆点闪烁）
const TICK: usize = 1;
const TICK_MS: u32 = 250;
/// 控制条上的工具提示：鼠标停 400ms 后出现
const TIP: usize = 2;

/// 截屏线程和界面线程共用：编码器（停的时候拿回来收尾），写出错时的原因
struct Shared {
    writer: Option<Writer>,
    error: Option<String>,
}

struct Session {
    mon: Monitor,
    region: Rect,
    path: PathBuf,
    shared: Arc<Mutex<Shared>>,
    /// 开始截以后才有；开始的时刻
    rec: Option<(plat::Recording, Instant)>,
    /// 边框的四条和控制条
    windows: Vec<Win>,
    /// 区域中间的倒计时，开始截时关掉
    count: Option<Win>,
}

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

/// 录屏的临时文件放在这里；每次启动清空（上次录的已经保存或丢弃了）
fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("Jie-recordings")
}

/// 启动时清掉上次留下的临时动图（文件夹里只有录屏写的 GIF，逐个删就行，用不着 remove_dir_all 那一大套）
pub fn cleanup() {
    for e in std::fs::read_dir(temp_dir()).into_iter().flatten().flatten() {
        let _ = std::fs::remove_file(e.path());
    }
}

/// 正在录（包括开始前的倒计时）
pub fn active() -> bool {
    SESSION.with_borrow(|s| s.is_some())
}

/// 开始截的时刻；还在倒计时、没在录是 None
pub fn started_at() -> Option<Instant> {
    SESSION.with_borrow(|s| s.as_ref()?.rec.as_ref().map(|r| r.1))
}

fn lock(m: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 录 `region`（全局桌面物理像素，在显示器 `mon` 上）：框和控制条出来，过 `delay`（0 就是马上）开始截，
/// 延时的话区域中间倒数；已经在录就不管
pub fn start(mon: Monitor, region: Rect, delay: Duration) {
    if active() {
        return;
    }
    let fail = |e: String| toast::show(&mon, Kind::Err, &format!("录屏没能开始：{e}"));
    let (w, h) = (region.right - region.left, region.bottom - region.top);
    // 系统临时目录本来就在，只建这一层
    if let Err(e) = std::fs::create_dir(temp_dir())
        && e.kind() != std::io::ErrorKind::AlreadyExists
    {
        return fail(e.to_string());
    }
    let path = temp_dir().join(format!("Jie_{}.gif", plat::now_stamp()));
    // 边框和圆点用设置里的选区颜色，和截图选区一样；动图边框编进 GIF，和录制时那圈一样粗、在同一个位置
    let st = settings::get();
    let color = st.sel_color;
    let writer = match Writer::framed(&path, w, h, st.frame_px(mon.scale(), st.gif_frame), color) {
        Ok(wr) => wr,
        Err(e) => return fail(e.to_string()),
    };
    let bytes = writer.bytes();
    let shared = Arc::new(Mutex::new(Shared { writer: Some(writer), error: None }));
    let at = Instant::now() + delay;
    let mut windows = frame(&mon, region, color);
    let bar = ControlBar::open(mon, region, bytes, at, color);
    // 倒计时由控制条的定时器推着走；不延时、控制条没开出来就直接开始
    let now = delay.is_zero() || bar.is_none();
    windows.extend(bar);
    let count = if now { None } else { Countdown::open(mon, region, at) };
    SESSION.set(Some(Session { mon, region, path, shared, rec: None, windows, count }));
    if now {
        Win::later(begin);
    }
}

/// 不延时、倒数完或者点了「立即开始」：关掉倒计时，开始截
fn begin() {
    let Some((region, shared, count)) = SESSION.with_borrow_mut(|s| {
        let s = s.as_mut().filter(|s| s.rec.is_none())?;
        Some((s.region, s.shared.clone(), s.count.take()))
    }) else {
        return;
    };
    // 倒计时窗口先关：mac 开始截时按当时开着的录屏窗口排除，它已经不在了
    count.into_iter().for_each(Win::close);
    let on_frame: plat::OnFrame = Box::new(move |px, stride, t| {
        let g = &mut *lock(&shared);
        let failed = match &mut g.writer {
            Some(wr) => wr.push(px, stride, t).err(),
            None => None,
        };
        // 写不下去了（比如磁盘满）：后面的帧都不要了，停的时候报原因
        if let Some(e) = failed {
            (g.error, g.writer) = (Some(e.to_string()), None);
        }
    });
    match plat::record(region, settings::get().record_fps, true, on_frame, |e| plat::post(move || broken(e))) {
        Ok(rec) => {
            SESSION.with_borrow_mut(|s| {
                if let Some(s) = s {
                    s.rec = Some((rec, Instant::now()));
                }
            });
            let key = hotkeys::label(hotkeys::RECORD);
            plat::set_tray_tip(&if key.is_empty() { "Jie · 正在录屏".into() } else { format!("Jie · 正在录屏 · {key} 停止") });
        }
        Err(e) => {
            let mon = SESSION.with_borrow(|s| s.as_ref().map(|s| s.mon));
            discard();
            if let Some(mon) = mon {
                toast::show(&mon, Kind::Err, &format!("录屏没能开始：{e}"));
            }
        }
    }
}

/// 录完的这次：在哪、临时文件、录了多久（倒计时里就停了是 None）、编码器（写出错时是原因）
struct Ended {
    mon: Monitor,
    region: Rect,
    path: PathBuf,
    secs: Option<u64>,
    writer: Result<Writer, String>,
}

/// 停住、关掉边框和控制条，拿回编码器
fn end() -> Option<Ended> {
    let Session { mon, region, path, shared, rec, windows, count } = SESSION.take()?;
    // 等截屏线程退出（Windows）、帧回调不会再进来（mac），之后编码器只归这边
    let secs = rec.map(|(r, t0)| {
        r.stop();
        t0.elapsed().as_secs()
    });
    windows.into_iter().for_each(Win::close);
    if let Some(c) = count {
        c.close();
    }
    hotkeys::refresh_tray();
    let mut g = lock(&shared);
    let writer = match (g.writer.take(), g.error.take()) {
        (_, Some(e)) => Err(e),
        (Some(w), None) => Ok(w),
        (None, None) => Err("没有录到画面".into()),
    };
    Some(Ended { mon, region, path, secs, writer })
}

/// 停止：写完动图，原位置钉出来，下面是结果条（复制、保存、钉住、丢弃）；倒计时里停等于取消
pub fn stop() {
    let Some(e) = end() else { return };
    let Some(secs) = e.secs else {
        drop(e.writer);
        let _ = std::fs::remove_file(&e.path);
        return;
    };
    // 时长和大小写在结果条最左边，录完不另外弹胶囊；出错照旧弹。GIF 带边框时左上角往外挪，画面还在原位置
    let result = e.writer.and_then(|w| {
        let pad = w.pad();
        w.finish().map(|_| pad).map_err(|x| x.to_string())
    });
    let result = result.and_then(|pad| {
        let at = Point { x: e.region.left - pad, y: e.region.top - pad };
        let bytes = std::fs::metadata(&e.path).map_or(0, |m| m.len());
        pin::create_gif(e.path.clone(), at, Some(format!("{} · {}", clock(secs), size(bytes))))
    });
    if let Err(x) = result {
        let _ = std::fs::remove_file(&e.path);
        toast::show(&e.mon, Kind::Err, &format!("录屏失败：{x}"));
    }
}

/// 放弃：停掉，不留文件
pub fn discard() {
    if let Some(e) = end() {
        drop(e.writer);
        let _ = std::fs::remove_file(&e.path);
    }
}

/// 平台层报录不下去了（权限被收回、显示器断开）：放弃这次，告诉用户
fn broken(e: String) {
    let Some(mon) = SESSION.with_borrow(|s| s.as_ref().map(|s| s.mon)) else { return };
    discard();
    toast::show(&mon, Kind::Err, &format!("录屏中断：{e}"));
}

// ---- 边框：区域外面一圈选区颜色，四条细窗口拼成（一整个透明窗口要和区域一样大的位图），鼠标穿透 ----

/// 一条边框，选区颜色
struct Strip(u32);

impl Handler for Strip {
    fn event(&mut self, _w: Win, _e: Event) {}

    fn paint(&mut self, _w: Win, c: &Canvas) {
        // 透明窗口要带 alpha 画：GDI 的 fill_px 在 Windows 上 alpha 是 0，会看不见
        let a = c.area();
        c.fill_rect(rectf(a.left as f32, a.top as f32, a.right as f32, a.bottom as f32), rgb(self.0));
    }
}

/// 区域 `r` 外面一圈的四条；贴着显示器边、外面放不下的那条挪到区域里面（`frame` 不画它）
fn frame_rects(mon: &Rect, r: Rect, b: i32) -> [Rect; 4] {
    let l = if r.left - b >= mon.left { r.left - b } else { r.left };
    let t = if r.top - b >= mon.top { r.top - b } else { r.top };
    let rr = if r.right + b <= mon.right { r.right + b } else { r.right };
    let bb = if r.bottom + b <= mon.bottom { r.bottom + b } else { r.bottom };
    [
        Rect { left: l, top: t, right: rr, bottom: t + b },
        Rect { left: l, top: bb - b, right: rr, bottom: bb },
        Rect { left: l, top: t + b, right: l + b, bottom: bb - b },
        Rect { left: rr - b, top: t + b, right: rr, bottom: bb - b },
    ]
}

pub(crate) fn border(scale: f32) -> i32 {
    settings::get().frame_px(scale, true)
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

pub(crate) fn frame(mon: &Monitor, r: Rect, color: u32) -> Vec<Win> {
    let mut v = Vec::new();
    for rect in frame_rects(&mon.rect, r, border(mon.scale())) {
        // 挪进区域里的那条会录进去：屏幕边本身就是边界，不画
        if overlaps(&rect, &r) {
            continue;
        }
        if let Some(w) = Win::open(Spec { kind: plat::Kind::RecFrame, rect, title: "Jie 录屏边框", owner: None }, Strip(color)) {
            w.show();
            v.push(w);
        }
    }
    v
}

// ---- 倒计时：区域中间一块柠檬黄方块写大数字，下面一行「按 … 停止录制」；鼠标穿透。开始截之前就关掉，不用设不被截屏 ----

/// 倒计时方块的边长、实心阴影（DIP）
const CD_BOX: f32 = 104.0;
const CD_SHADOW: f32 = 6.0;

struct Countdown {
    scale: f32,
    at: Instant,
    hint: Option<Hud>,
    /// 上次画的数字，变了才重画
    shown: u64,
}

/// 离 `at` 还有几秒（向上取整，最少 1）
fn left(at: Instant) -> u64 {
    at.saturating_duration_since(Instant::now()).as_millis().div_ceil(1000).max(1) as u64
}

impl Countdown {
    /// 在区域中间（挪进显示器里）
    fn open(mon: Monitor, region: Rect, at: Instant) -> Option<Win> {
        let s = mon.scale();
        let px = |d: f32| (d * s).round() as i32;
        let key = hotkeys::label(hotkeys::RECORD);
        let hint = if key.is_empty() {
            Hud::new(&[("点控制条上的停止按钮结束录制", 0xFFFFFF)], s)
        } else {
            Hud::new(&[("按", 0xFFFFFF), (key.as_str(), ui::HUD_TEXT2), ("停止录制", 0xFFFFFF)], s)
        };
        let boxed = px(CD_BOX) + px(CD_SHADOW);
        let w = hint.as_ref().map_or(0, |h| h.w).max(boxed);
        let h = boxed + hint.as_ref().map_or(0, |h| px(12.0) + h.h);
        let m = mon.rect;
        let x = ((region.left + region.right - w) / 2).clamp(m.left, m.right - w);
        let y = ((region.top + region.bottom - h) / 2).clamp(m.top, m.bottom - h);
        let cd = Countdown { scale: s, at, hint, shown: left(at) };
        let win = Win::open(Spec { kind: plat::Kind::RecFrame, rect: Rect { left: x, top: y, right: x + w, bottom: y + h }, title: "Jie 录屏倒计时", owner: None }, cd)?;
        win.show();
        win.timer(TICK, TICK_MS);
        Some(win)
    }
}

impl Handler for Countdown {
    fn event(&mut self, w: Win, e: Event) {
        if let Event::Timer(TICK) = e {
            let n = left(self.at);
            if n != self.shown {
                self.shown = n;
                w.invalidate_all();
            }
        }
    }

    fn paint(&mut self, _w: Win, c: &Canvas) {
        let s = self.scale;
        let a = c.area();
        let w = (a.right - a.left) as f32;
        c.clear(Color::default());
        let (b, d) = ((CD_BOX * s).round(), (CD_SHADOW * s).round());
        let x = ((w - b - d) / 2.0).round();
        let r = rectf(x, 0.0, x + b, b);
        ui::block(c, r, 14.0, CD_SHADOW, ui::PICK, s);
        if let Some(n) = Text::tabular(Face::Ui, 56.0 * s, true, &self.shown.to_string()) {
            c.text(&n, x + (b - n.width()) / 2.0, (b - n.height()) / 2.0, rgb(ui::INK));
        }
        if let Some(h) = &self.hint {
            h.draw(c, ((w - h.w as f32) / 2.0).round() as i32, (b + d + 12.0 * s).round() as i32);
        }
    }
}

// ---- 控制条：倒计时里「● 00:00  3 秒后开始 | [▶] [✕]」，录制中「● 00:12  录制中 · 1.3 MB | [■] [🗑]」 ----
// 样式和操作栏一样（白底、墨色描边、4,4 实心阴影）；按钮只放图标，鼠标停 400ms 出提示（名字和快捷键）。
// 窗口比控制条多出放提示的一截，放在不压着录制区域的那一边，提示不会录进去

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    /// 立即开始 / 停止
    Main,
    /// 取消 / 放弃
    Second,
}

struct ControlBar {
    scale: f32,
    /// 倒计时里是开始录的时刻；开始以后是真正开始的时刻
    at: Instant,
    started: bool,
    bytes: Arc<AtomicU64>,
    /// 圆点的颜色（选区颜色）
    color: u32,
    /// 录屏快捷键的名字，提示里写
    key: String,
    /// 控制条本体（连阴影）在窗口里的位置；窗口上面或下面还留着放提示的那一截
    body: Rect,
    /// 两个按钮在窗口里的位置
    main: Rect,
    second: Rect,
    /// 状态文字、分组线离本体左边多远（DIP）
    status_x: f32,
    sep_x: f32,
    hot: Option<Btn>,
    down: Option<Btn>,
    /// 显示提示的按钮；提示在本体上方（否则下方）
    tip: Option<Btn>,
    tip_up: bool,
}

/// 时间「01:05」，超过一小时「1:01:05」
fn clock(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m:02}:{s:02}") }
}

/// 文件大小「820 KB」「12.4 MB」
fn size(n: u64) -> String {
    if n >= 1 << 20 { format!("{:.1} MB", n as f64 / (1u64 << 20) as f64) } else { format!("{} KB", n.div_ceil(1024)) }
}

fn contains(r: &Rect, p: Point) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

/// 控制条（宽高 `size`）放哪：区域外面依次试下方（和边框外沿右对齐）、上方、右侧、左侧，先找离得开 `g` 的，
/// 再找贴着也放得下的（在那条空隙里居中，比如压在任务栏上）；都不行放进区域里，返回 true：
/// 区域里有任务栏就压在任务栏右边（那块画面几乎不动，录的时候盖成开始前的样子不显眼），没有就放右下角。
/// `b` 是边框粗细，`m` 是离屏幕边至少留多少
pub(crate) fn bar_rect(mon: &Rect, work: &Rect, region: Rect, (bw, bh): (i32, i32), b: i32, g: i32, m: i32) -> (Rect, bool) {
    let at = |x: i32, y: i32| Rect { left: x, top: y, right: x + bw, bottom: y + bh };
    let x_under = (region.right + b - bw).min(mon.right - m - bw).max(mon.left + m);
    let y_side = (region.bottom - bh).min(mon.bottom - m - bh).max(mon.top + m);
    let (below, above, right, left) = (mon.bottom - region.bottom, region.top - mon.top, mon.right - region.right, region.left - mon.left);
    for tight in [false, true] {
        // 离区域多远：空隙够就隔 g，不够就在空隙里居中
        let off = |space: i32, need: i32| if tight { (space >= need).then(|| (space - need) / 2) } else { (space >= g + need + m).then_some(g) };
        if let Some(d) = off(below, bh) {
            return (at(x_under, region.bottom + d), false);
        }
        if let Some(d) = off(above, bh) {
            return (at(x_under, region.top - d - bh), false);
        }
        if let Some(d) = off(right, bw) {
            return (at(region.right + d, y_side), false);
        }
        if let Some(d) = off(left, bw) {
            return (at(region.left - d - bw, y_side), false);
        }
    }
    let (bar_below, bar_above) = (mon.bottom - work.bottom, work.top - mon.top);
    let on_bar = (region.right.min(mon.right) - m - bw).max(region.left + m);
    // 区域里含着的那截任务栏放得下就行：手动拖到屏幕最后一个像素会差 1 像素
    let spot = if bar_below >= bh && region.bottom - work.bottom >= bh {
        at(on_bar, work.bottom + (bar_below - bh) / 2)
    } else if bar_above >= bh && work.top - region.top >= bh {
        at(on_bar, mon.top + (bar_above - bh) / 2)
    } else {
        at((region.right - g - bw).min(work.right - m - bw).max(work.left + m), (region.bottom - g - bh).min(work.bottom - m - bh).max(work.top + m))
    };
    (spot, true)
}

// 尺寸（DIP）：本体高 48（描边 2 + 内边距 5 + 按钮 34 + 5 + 2），右下阴影 4；圆点中心离左边 19；提示高 26，离本体 6
const BODY_H: f32 = 48.0;
const SHADOW: f32 = 4.0;
const TIP_ROOM: f32 = 6.0 + 26.0;

impl ControlBar {
    /// 状态文字：倒计时里是还剩几秒，录制中是「录制中 · 文件大小」
    fn status(&self) -> String {
        if self.started { format!("录制中 · {}", size(self.bytes.load(Ordering::Relaxed))) } else { format!("{} 秒后开始", left(self.at)) }
    }

    /// 按钮的提示：名字和快捷键
    fn tip_text(&self, b: Btn) -> (&'static str, &str) {
        match (b, self.started) {
            (Btn::Main, true) => ("停止", &self.key),
            (Btn::Second, true) => ("放弃，不留文件", ""),
            (Btn::Main, false) => ("立即开始", ""),
            (Btn::Second, false) => ("取消", &self.key),
        }
    }

    /// 按缩放 `s` 排好（状态文字按两种状态里宽的那个留位置，换状态不变宽）；返回控制条和本体连阴影的宽高（像素），还没留提示的地方。
    /// `at` 到了才开始；已经过了（不延时）就是录制中
    fn new(s: f32, bytes: Arc<AtomicU64>, at: Instant, color: u32) -> (ControlBar, i32, i32) {
        let px = |d: f32| (d * s).round() as i32;
        let dip = |t: Option<Text>| t.map_or(40.0, |l| l.width() / s);
        let widest = |ts: &[&str], size: f32| ts.iter().map(|t| dip(Text::tabular(Face::Ui, size * s, true, t))).fold(0.0, f32::max);
        // 圆点、时间（左 32）、10、状态、10、分组线 2、5、主按钮 44、3、次按钮 34、内边 7
        let status_x = 32.0 + widest(&["00:00"], 14.0) + 10.0;
        let sep_x = (status_x + widest(&["录制中 · 999.9 MB", "10 秒后开始"], 12.5) + 10.0).ceil();
        let x_main = sep_x + 2.0 + 5.0;
        let width = x_main + 44.0 + 3.0 + 34.0 + 7.0;
        let (t, b) = (px(7.0), px(7.0 + 34.0));
        let (w, h) = (px(width) + px(SHADOW), px(BODY_H) + px(SHADOW));
        let bar = ControlBar {
            scale: s,
            at,
            started: Instant::now() >= at,
            bytes,
            color,
            key: hotkeys::label(hotkeys::RECORD),
            body: Rect { left: 0, top: 0, right: w, bottom: h },
            main: Rect { left: px(x_main), top: t, right: px(x_main + 44.0), bottom: b },
            second: Rect { left: px(x_main + 47.0), top: t, right: px(x_main + 81.0), bottom: b },
            status_x,
            sep_x,
            hot: None,
            down: None,
            tip: None,
            tip_up: false,
        };
        (bar, w, h)
    }

    /// 窗口在本体上方（`up`）或下方多留放提示的一截；返回多出来的高度。放上方时本体和按钮往下挪
    fn tip_room(&mut self, up: bool) -> i32 {
        let extra = (TIP_ROOM * self.scale).round() as i32;
        self.tip_up = up;
        if up {
            for r in [&mut self.body, &mut self.main, &mut self.second] {
                (r.top, r.bottom) = (r.top + extra, r.bottom + extra);
            }
        }
        extra
    }

    /// 摆在区域外面（见 `bar_rect`）；外面放不下才放进区域里，这时设成不被截屏（显示之前）。
    /// 提示那一截先找不压着区域、不出屏的一边（下面优先），都不行就找不出屏的
    fn open(mon: Monitor, region: Rect, bytes: Arc<AtomicU64>, at: Instant, color: u32) -> Option<Win> {
        let s = mon.scale();
        let (mut bar, bw, bh) = ControlBar::new(s, bytes, at, color);
        let px = |d: f32| (d * s).round() as i32;
        let b = border(s);
        let (rect, inside) = bar_rect(&mon.rect, &mon.work, region, (bw, bh), b, px(8.0) + b, px(4.0));
        let extra = px(TIP_ROOM);
        let below = Rect { top: rect.bottom, bottom: rect.bottom + extra, ..rect };
        let above = Rect { top: rect.top - extra, bottom: rect.top, ..rect };
        let fits = |r: &Rect| r.top >= mon.rect.top && r.bottom <= mon.rect.bottom;
        let up = if fits(&below) && !overlaps(&below, &region) {
            false
        } else if fits(&above) && !overlaps(&above, &region) {
            true
        } else {
            !fits(&below)
        };
        bar.tip_room(up);
        let rect = if up { Rect { top: rect.top - extra, ..rect } } else { Rect { bottom: rect.bottom + extra, ..rect } };
        let w = Win::open(Spec { kind: plat::Kind::RecBar, rect, title: "Jie 录屏", owner: None }, bar)?;
        // 截不下底下的画面时照样显示：没有按钮比录进去更糟
        if inside {
            w.hide_from_capture();
        }
        w.show();
        w.timer(TICK, TICK_MS);
        Some(w)
    }

    fn button_at(&self, p: Point) -> Option<Btn> {
        if contains(&self.main, p) {
            Some(Btn::Main)
        } else if contains(&self.second, p) {
            Some(Btn::Second)
        } else {
            None
        }
    }

    /// 倒数完或者点了「立即开始」
    fn go(&mut self) {
        (self.started, self.at) = (true, Instant::now());
        Win::later(begin);
    }

    /// 鼠标换了按钮：提示先收起，停 400ms 再出来
    fn set_hot(&mut self, w: Win, hot: Option<Btn>) {
        if hot != self.hot {
            (self.hot, self.tip) = (hot, None);
            w.kill_timer(TIP);
            if hot.is_some() && self.down.is_none() {
                w.timer(TIP, 400);
            }
            w.invalidate_all();
        }
    }
}

impl Handler for ControlBar {
    fn event(&mut self, w: Win, e: Event) {
        match e {
            Event::Timer(TICK) => {
                if !self.started && Instant::now() >= self.at {
                    self.go();
                }
                w.invalidate_all();
            }
            Event::Timer(TIP) => {
                w.kill_timer(TIP);
                self.tip = self.hot;
                w.invalidate_all();
            }
            Event::Move { p, .. } => self.set_hot(w, self.button_at(p)),
            Event::Leave => self.set_hot(w, None),
            Event::Down { p, button: Button::Left, .. } => {
                (self.down, self.tip) = (self.button_at(p), None);
                w.kill_timer(TIP);
                w.invalidate_all();
            }
            // 松开时还在按下的那个按钮上才算点击；停止会关掉这个窗口，等这次处理完再做
            Event::Up { button: Button::Left } => {
                let clicked = self.down.take().filter(|b| self.hot == Some(*b));
                w.invalidate_all();
                match clicked {
                    Some(Btn::Main) if !self.started => self.go(),
                    Some(Btn::Main) => Win::later(stop),
                    Some(Btn::Second) => Win::later(discard),
                    None => {}
                }
            }
            Event::CaptureLost => self.down = None,
            _ => {}
        }
    }

    fn paint(&mut self, _w: Win, c: &Canvas) {
        let s = self.scale;
        let f = |r: &Rect| rectf(r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
        c.clear(Color::default());
        let d = (SHADOW * s).round();
        let body = f(&self.body);
        let body = rectf(body.left, body.top, body.right - d, body.bottom - d);
        ui::block(c, body, 10.0, SHADOW, ui::SURF, s);
        let (x0, mid, lw) = (body.left, (body.top + body.bottom) / 2.0, ui::line_w(s));
        // 点：录制中每半秒闪一下（选区颜色，外面一圈墨色，选了白色也看得见），倒计时里是灰的
        let elapsed = self.at.elapsed().as_millis();
        let dot = if !self.started { Some(ui::FAINT) } else { (elapsed / 500).is_multiple_of(2).then_some(self.color) };
        if let Some(col) = dot {
            c.fill_ellipse(x0 + 19.0 * s, mid, 5.0 * s + lw, 5.0 * s + lw, rgb(ui::INK));
            c.fill_ellipse(x0 + 19.0 * s, mid, 5.0 * s, 5.0 * s, rgb(col));
        }
        let secs = if self.started { elapsed as u64 / 1000 } else { 0 };
        if let Some(t) = Text::tabular(Face::Ui, 14.0 * s, true, &clock(secs)) {
            c.text(&t, x0 + 32.0 * s, mid - t.height() / 2.0, rgb(if self.started { ui::INK } else { ui::MUTED }));
        }
        if let Some(t) = Text::tabular(Face::Ui, 12.5 * s, true, &self.status()) {
            c.text(&t, x0 + self.status_x * s, mid - t.height() / 2.0, rgb(ui::MUTED));
        }
        let sx = (x0 + self.sep_x * s).round();
        c.fill_rect(rectf(sx, mid - 11.0 * s, sx + lw, mid + 11.0 * s), rgb(ui::INK));
        let st = |b: Btn| ui::state(true, false, self.hot == Some(b), self.down == Some(b));
        // 主按钮：紫底白图标，和截图操作栏一样
        let face = ui::primary(c, f(&self.main), st(Btn::Main), s);
        let icon = if self.started { "stop" } else { "play" };
        ui::icon(c, icon, face.left + (face.right - face.left - 20.0 * s) / 2.0, face.top + (face.bottom - face.top - 20.0 * s) / 2.0, 20.0 * s, rgb(0xFFFFFF));
        let r = f(&self.second);
        let (dy, fg) = ui::button_bg(c, r, st(Btn::Second), s);
        let icon = if self.started { "trash" } else { "cancel" };
        ui::icon(c, icon, r.left + (r.right - r.left - 20.0 * s) / 2.0, r.top + (r.bottom - r.top - 20.0 * s) / 2.0 + dy, 20.0 * s, fg);
        // 提示：按钮正上方或正下方，夹在窗口里
        if let Some(b) = self.tip {
            let (name, key) = self.tip_text(b);
            let parts: &[(&str, u32)] = if key.is_empty() { &[(name, 0xFFFFFF)] } else { &[(name, 0xFFFFFF), (key, ui::HUD_TEXT2)] };
            if let Some(h) = Hud::new(parts, s) {
                let r = if b == Btn::Main { self.main } else { self.second };
                let x = ((r.left + r.right - h.w) / 2).clamp(0, (self.body.right - h.w).max(0));
                let g = (6.0 * s).round() as i32;
                let y = if self.tip_up { self.body.top - g - h.h } else { self.body.bottom + g };
                h.draw(c, x, y);
            }
        }
    }

    fn cursor(&mut self, _w: Win, _p: Point) -> Cursor {
        Cursor::Arrow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 控制条画在灰底上（倒计时里鼠标停在「取消」上、录制中鼠标停在「停止」上并出了提示），存成 target/test-tmp/record/bar-*.png。
    /// 平时不跑：cargo test control_bar_sample -- --ignored
    #[test]
    #[ignore]
    fn control_bar_sample() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("record");
        std::fs::create_dir_all(&dir).unwrap();
        for started in [false, true] {
            let (mut bar, w, mut h) = ControlBar::new(1.5, Arc::new(AtomicU64::new(1_363_149)), Instant::now() + Duration::from_millis(2500), settings::sel_color());
            if started {
                bar.started = true;
                (bar.hot, bar.tip) = (Some(Btn::Main), Some(Btn::Main));
                h += bar.tip_room(false);
            } else {
                bar.hot = Some(Btn::Second);
                h += bar.tip_room(true);
            }
            let b = plat::Bitmap::new(w, h).unwrap();
            bar.paint(Win::default(), &Canvas::new(&b, b.full(), true));
            let mut out = plat::Bitmap::new(w + 40, h + 40).unwrap();
            out.pixels_mut().fill(0x70);
            // 预乘 alpha 叠到灰底上
            let (src, ow) = (b.pixels().to_vec(), out.w);
            for y in 0..h {
                for x in 0..w {
                    let si = ((y * w + x) * 4) as usize;
                    let di = (((y + 20) * ow + x + 20) * 4) as usize;
                    let a = src[si + 3] as u32;
                    for k in 0..3 {
                        out.pixels_mut()[di + k] = (src[si + k] as u32 + 0x70 * (255 - a) / 255) as u8;
                    }
                }
            }
            let name = if started { "bar-recording.png" } else { "bar-countdown.png" };
            std::fs::write(dir.join(name), plat::encode(&out.crop(&out.full()), false).unwrap()).unwrap();
        }
    }

    #[test]
    fn clock_and_size_text() {
        assert_eq!(clock(0), "00:00");
        assert_eq!(clock(65), "01:05");
        assert_eq!(clock(3661), "1:01:01");
        assert_eq!(size(1), "1 KB");
        assert_eq!(size(820 * 1024), "820 KB");
        assert_eq!(size(13_002_342), "12.4 MB");
    }

    /// 边框在区域外面一圈；贴着显示器边的那条挪进区域里面，不出屏
    #[test]
    fn frame_stays_on_screen() {
        let mon = Rect { left: 0, top: 0, right: 1000, bottom: 800 };
        let r = Rect { left: 100, top: 100, right: 500, bottom: 400 };
        let [t, b, l, rr] = frame_rects(&mon, r, 3);
        assert_eq!(t, Rect { left: 97, top: 97, right: 503, bottom: 100 });
        assert_eq!(b, Rect { left: 97, top: 400, right: 503, bottom: 403 });
        assert_eq!(l, Rect { left: 97, top: 100, right: 100, bottom: 400 });
        assert_eq!(rr, Rect { left: 500, top: 100, right: 503, bottom: 400 });
        // 全屏：四条都在里面
        for s in frame_rects(&mon, mon, 3) {
            assert!(s.left >= 0 && s.top >= 0 && s.right <= 1000 && s.bottom <= 800, "{s:?}");
        }
    }

    /// 控制条放在区域外面：一般在下方；区域到底了放上方；录「屏幕去掉任务栏」时压在任务栏上；框满整块屏幕才放进里面
    #[test]
    fn bar_goes_outside() {
        let mon = Rect { left: 0, top: 0, right: 1920, bottom: 1080 };
        let work = Rect { bottom: 1032, ..mon };
        let place = |r: Rect| bar_rect(&mon, &work, r, (330, 40), 3, 11, 4);
        let (r, inside) = place(Rect { left: 400, top: 300, right: 1000, bottom: 700 });
        assert_eq!((r, inside), (Rect { left: 673, top: 711, right: 1003, bottom: 751 }, false));
        // 下面只剩 50，放上方
        let (r, inside) = place(Rect { left: 400, top: 300, right: 1000, bottom: 1030 });
        assert_eq!((r.top, r.bottom, inside), (249, 289, false));
        // 去掉任务栏的整块屏幕：在任务栏那 48 里居中，在区域外面
        let (r, inside) = place(work);
        assert_eq!((r, inside), (Rect { left: 1586, top: 1036, right: 1916, bottom: 1076 }, false));
        // 左半边整列：放右侧
        let (r, inside) = place(Rect { left: 0, top: 0, right: 900, bottom: 1080 });
        assert_eq!((r.left, r.bottom, inside), (911, 1076, false));
        // 整块屏幕：只能放进里面，压在任务栏右边
        let (r, inside) = place(mon);
        assert_eq!((r, inside), (Rect { left: 1586, top: 1036, right: 1916, bottom: 1076 }, true));
        // 手动拖满差 1 像素：一样
        let (r, inside) = place(Rect { right: 1919, bottom: 1079, ..mon });
        assert_eq!((r.top, inside), (1036, true));
        // 任务栏自动隐藏：放右下角
        let (r, inside) = bar_rect(&mon, &mon, mon, (330, 40), 3, 11, 4);
        assert_eq!((r, inside), (Rect { left: 1579, top: 1029, right: 1909, bottom: 1069 }, true));
        // 外面的那几种都不碰区域
        for reg in [Rect { left: 400, top: 300, right: 1000, bottom: 700 }, work, Rect { left: 0, top: 0, right: 900, bottom: 1080 }] {
            assert!(!overlaps(&place(reg).0, &reg));
        }
    }

    #[cfg(windows)]
    fn click(w: Win, r: Rect) {
        let (p, mods) = (Point { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 }, plat::Mods::default());
        w.send(Event::Move { p, mods });
        w.send(Event::Down { p, button: Button::Left, double: false, mods });
        w.send(Event::Up { button: Button::Left });
    }

    #[cfg(windows)]
    fn pump_until(what: &str, done: impl Fn() -> bool) {
        let t0 = Instant::now();
        while !done() {
            assert!(t0.elapsed() < Duration::from_secs(5), "等不到：{what}");
            if !plat::pump() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// 截下 `area` 这块真实屏幕存成 png：边框、控制条、倒计时没设不被截屏，截出来的就是远程桌面看到的样子
    #[cfg(windows)]
    fn scene(name: &str, area: Rect) {
        let (mon, screen) = plat::capture_cursor_monitor().unwrap();
        let r = Rect { left: area.left - mon.rect.left, top: area.top - mon.rect.top, right: area.right - mon.rect.left, bottom: area.bottom - mon.rect.top };
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("record");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), plat::encode(&screen.crop(&r), false).unwrap()).unwrap();
    }

    /// 整个流程：框和倒计时出来 → 点「立即开始」→ 录 1 秒 → 停止：边框控制条都关掉，原位置出动图钉图和结果条。
    /// 叠出来的效果图存在 target/test-tmp/record/scene-*.png。会在屏幕上开窗口、截屏，平时不跑：
    /// cargo test record_flow -- --ignored
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn record_flow() {
        // 项目目录里跑的测试是低完整性，写不了系统临时目录
        let tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("record");
        std::fs::create_dir_all(&tmp).unwrap();
        unsafe { std::env::set_var("TMP", &tmp) };
        let mon = plat::monitor_at(plat::cursor_pos());
        let wk = mon.work;
        let region = Rect { left: wk.left + 400, top: wk.top + 300, right: wk.left + 1000, bottom: wk.top + 700 };
        start(mon, region, Duration::from_secs(3));
        assert!(active());
        // 四条边框和倒计时
        assert_eq!(Win::all(plat::Kind::RecFrame).len(), 5);
        let bar = *Win::all(plat::Kind::RecBar).last().expect("控制条没出来");
        let br = bar.rect();
        assert!(br.top >= region.bottom && br.right <= region.right + border(mon.scale()), "{br:?}");
        let area = Rect { left: region.left - 40, top: region.top - 40, right: region.right + 40, bottom: br.bottom + 40 };
        let wait = |ms: u64| {
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_millis(ms) {
                if !plat::pump() {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        };
        wait(150);
        scene("scene-countdown.png", area);
        click(bar, bar.peek::<ControlBar>().main);
        pump_until("开始截", || SESSION.with_borrow(|s| s.as_ref().is_some_and(|s| s.rec.is_some())));
        assert_eq!(Win::all(plat::Kind::RecFrame).len(), 4, "倒计时该关了");
        wait(1200);
        scene("scene-recording.png", area);
        let path = SESSION.with_borrow(|s| s.as_ref().unwrap().path.clone());
        stop();
        assert!(!active() && Win::all(plat::Kind::RecFrame).is_empty() && Win::all(plat::Kind::RecBar).is_empty());
        let pin = *Win::all(plat::Kind::Pin).last().expect("动图钉图没出来");
        // 动图钉图的画面在原位置，GIF 里外面那圈框和录制时的边框一样粗
        let st = settings::get();
        let f = st.frame_px(mon.scale(), st.gif_frame);
        assert_eq!((pin.rect().left, pin.rect().top), (region.left - f, region.top - f));
        // 时长和大小写在结果条上，录完不弹胶囊
        assert!(!Win::all(plat::Kind::PinBar).is_empty() && Win::all(plat::Kind::Toast).is_empty(), "该有结果条、没有胶囊");
        // 录制时的边框在区域外面，GIF 画面最上面一行不该是边框的颜色（RGBA、BGRA 都认）；编进去的那圈框是
        let mut p = crate::anim::Player::open(&path).unwrap();
        assert_eq!((p.w, p.h), (region.right - region.left + 2 * f, region.bottom - region.top + 2 * f));
        let mut px = vec![0u8; (p.w * p.h * 4) as usize];
        p.next(&mut px).unwrap();
        let col = settings::sel_color();
        let (r, g, b) = ((col >> 16) as u8, (col >> 8) as u8, col as u8);
        let near = |x: u8, y: u8| x.abs_diff(y) < 24;
        let edge = |c: &[u8]| near(c[1], g) && ((near(c[0], r) && near(c[2], b)) || (near(c[0], b) && near(c[2], r)));
        let row = |y: i32| px[(y * p.w * 4) as usize..((y + 1) * p.w * 4) as usize].chunks_exact(4).filter(|c| edge(c)).count();
        let top = row(f);
        assert!(top * 2 < p.w as usize, "GIF 画面第一行 {top}/{} 是边框颜色：录制时的边框录进去了", p.w);
        assert!(f == 0 || row(0) == p.w as usize, "动图边框没编进 GIF");
        pin.close();
        // 屏幕去掉任务栏：控制条压在任务栏上，不放进区域里
        if mon.rect.bottom - wk.bottom >= br.bottom - br.top {
            start(mon, wk, Duration::from_secs(3));
            let bar = *Win::all(plat::Kind::RecBar).last().unwrap();
            assert!(bar.rect().top >= wk.bottom, "{:?}", bar.rect());
            wait(150);
            scene("scene-work.png", Rect { left: wk.right - 900, top: wk.bottom - 160, right: wk.right, bottom: mon.rect.bottom });
            discard();
        }
        cleanup();
    }

    /// `a`、`b` 两张 BGRA 图（宽 `aw`、`bw`）里 `r` 这块（各自的坐标）平均每个通道差多少
    #[cfg(windows)]
    fn diff(a: &[u8], aw: i32, b: &[u8], bw: i32, r: Rect) -> f64 {
        let mut sum = 0u64;
        for y in r.top..r.bottom {
            for x in r.left..r.right {
                let (i, j) = (((y * aw + x) * 4) as usize, ((y * bw + x) * 4) as usize);
                sum += (0..3).map(|k| a[i + k].abs_diff(b[j + k]) as u64).sum::<u64>();
            }
        }
        sum as f64 / ((r.right - r.left) * (r.bottom - r.top) * 3) as f64
    }

    /// 一块纯色的普通窗口，颜色录的时候能换
    #[cfg(windows)]
    struct Swatch(u32);

    #[cfg(windows)]
    impl Handler for Swatch {
        fn event(&mut self, _w: Win, _e: Event) {}
        fn paint(&mut self, _w: Win, c: &Canvas) {
            let a = c.area();
            c.fill_rect(rectf(a.left as f32, a.top as f32, a.right as f32, a.bottom as f32), rgb(self.0));
        }
    }

    /// `r` 这块（BGRA 图里的坐标，宽 `w`）的平均颜色 0xRRGGBB
    #[cfg(windows)]
    fn mean(px: &[u8], w: i32, r: Rect) -> u32 {
        let mut sum = [0u64; 3];
        for y in r.top..r.bottom {
            for x in r.left..r.right {
                let i = ((y * w + x) * 4) as usize;
                (0..3).for_each(|k| sum[k] += px[i + k] as u64);
            }
        }
        let n = ((r.right - r.left) * (r.bottom - r.top)) as u64;
        ((sum[2] / n) << 16 | (sum[1] / n) << 8 | (sum[0] / n)) as u32
    }

    #[cfg(windows)]
    fn near(a: u32, b: u32) -> bool {
        [16, 8, 0].iter().all(|s| ((a >> s) & 0xFF).abs_diff((b >> s) & 0xFF) < 40)
    }

    /// 框满整块屏幕（直接开始，或者从录屏遮罩拖满后按 Enter）：四条边框都贴着屏幕边，不画；控制条压在任务栏上，
    /// 截屏截得到（远程看得到）。控制条底下先垫一块绿色的窗口，录到一半换成橙色：GIF 第一帧那块是绿的、
    /// 最后一帧是橙的——那块是活的画面，不是控制条，也不是开始前冻住的样子（从遮罩来时也不是变暗的遮罩）。
    /// 会录整块屏幕，平时不跑；要用发布版（调试版在 4K 屏上一秒截不到几帧）：
    /// cargo test --release full_screen -- --ignored --nocapture --test-threads 1
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn full_screen_flow() {
        full_screen(false);
    }

    #[test]
    #[ignore]
    #[cfg(windows)]
    fn full_screen_from_overlay() {
        full_screen(true);
    }

    #[cfg(windows)]
    fn full_screen(via_overlay: bool) {
        let tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("record");
        std::fs::create_dir_all(&tmp).unwrap();
        unsafe { std::env::set_var("TMP", &tmp) };
        let wait = |ms: u64| {
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_millis(ms) {
                if !plat::pump() {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        };
        let mon = plat::monitor_at(plat::cursor_pos());
        // 控制条会放在哪：照 start 的算法先算出来，在那里垫一块绿色
        let s = mon.scale();
        let (_, bw, bh) = ControlBar::new(s, Arc::new(AtomicU64::new(0)), Instant::now(), 0);
        let (b, px) = (border(s), |d: f32| (d * s).round() as i32);
        let (spot, inside) = bar_rect(&mon.rect, &mon.work, mon.rect, (bw, bh), b, px(8.0) + b, px(4.0));
        assert!(inside);
        let (green, orange) = (0x23A55A, 0xF27A1A);
        let pad = Rect { left: spot.left - 30, top: spot.top - 6, right: spot.right + 30, bottom: spot.bottom + 6 };
        // 用钉图这种窗口：不透明、走 WM_PAINT（卡片现在是透明窗口）
        let sw = Win::open(Spec { kind: plat::Kind::Pin, rect: pad, title: "Jie 测试色块", owner: None }, Swatch(green)).unwrap();
        let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::ShowWindow(sw.hwnd(), windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE) };
        wait(200);
        let (mon, before) = plat::capture_cursor_monitor().unwrap();
        if via_overlay {
            crate::overlay::show(crate::overlay::Mode::Record);
            let ov = *Win::all(plat::Kind::Overlay).last().expect("遮罩没出来");
            let (w, h, mods) = (mon.rect.right - mon.rect.left, mon.rect.bottom - mon.rect.top, plat::Mods::default());
            ov.send(Event::Down { p: Point { x: 0, y: 0 }, button: Button::Left, double: false, mods });
            ov.send(Event::Move { p: Point { x: w / 2, y: h / 2 }, mods });
            ov.send(Event::Move { p: Point { x: w - 1, y: h - 1 }, mods });
            ov.send(Event::Up { button: Button::Left });
            ov.send(Event::Key { key: plat::Key::Enter, mods });
            pump_until("从遮罩开始录屏", active);
        } else {
            start(mon, mon.rect, Duration::from_secs(3));
        }
        // 遮罩上默认不延时：马上开始、没有倒计时；直接开始的这条倒数 3 秒
        assert_eq!(Win::all(plat::Kind::RecFrame).len(), usize::from(!via_overlay), "四条边框都不该画");
        let bar = *Win::all(plat::Kind::RecBar).last().unwrap();
        // 控制条本体（窗口上下还留着放提示的空白）
        let (wr, body) = (bar.rect(), bar.peek::<ControlBar>().body);
        let br = Rect { left: wr.left + body.left, top: wr.top + body.top, right: wr.left + body.right, bottom: wr.top + body.bottom };
        if mon.rect.bottom - mon.work.bottom >= br.bottom - br.top {
            assert!(br.top >= mon.work.bottom, "控制条该压在任务栏上：{br:?}");
        }
        let local = Rect { left: br.left - mon.rect.left, top: br.top - mon.rect.top, right: br.right - mon.rect.left, bottom: br.bottom - mon.rect.top };
        wait(150);
        let (_, shown) = plat::capture_cursor_monitor().unwrap();
        let seen = diff(before.pixels(), before.w, shown.pixels(), shown.w, local);
        println!("截屏里控制条那块和开始前差 {seen:.1}");
        assert!(seen > 20.0, "截屏里看不到控制条");
        scene(if via_overlay { "scene-full-overlay.png" } else { "scene-full.png" }, Rect { left: br.left - 300, top: br.top - 120, right: mon.rect.right, bottom: mon.rect.bottom });
        if !bar.peek::<ControlBar>().started {
            click(bar, bar.peek::<ControlBar>().main);
        }
        pump_until("开始截", || SESSION.with_borrow(|s| s.as_ref().is_some_and(|s| s.rec.is_some())));
        wait(800);
        // 录到一半，控制条底下那块变色
        sw.with(|w: &mut Swatch| w.0 = orange);
        sw.invalidate_all();
        wait(1200);
        let path = SESSION.with_borrow(|s| s.as_ref().unwrap().path.clone());
        stop();
        // 第一帧：和开始前一样（绿的），没有控制条
        let mut p = crate::anim::Player::open(&path).unwrap();
        let mut whole = vec![0u8; (p.w * p.h * 4) as usize];
        p.next(&mut whole).unwrap();
        // GIF 里编进去的那圈边框去掉，坐标才和屏幕对得上
        let st = settings::get();
        let (f, pw) = (st.frame_px(s, st.gif_frame), p.w);
        let (gw, gh) = (pw - 2 * f, p.h - 2 * f);
        let inner = |px: &[u8]| -> Vec<u8> { (f..f + gh).flat_map(|y| px[((y * pw + f) * 4) as usize..((y * pw + f + gw) * 4) as usize].to_vec()).collect() };
        let frame = inner(&whole);
        let left = diff(before.pixels(), before.w, &frame, gw, local);
        let first = mean(&frame, gw, local);
        let not_bar = diff(shown.pixels(), shown.w, &frame, gw, local);
        println!("GIF 第一帧控制条那块：和开始前差 {left:.1}，和控制条差 {not_bar:.1}，平均色 {first:06X}");
        assert!(not_bar > 20.0, "控制条录进去了");
        let around = Rect { left: (local.left - 200).max(0), top: (local.top - 100).max(0), right: local.right.min(gw), bottom: local.bottom.min(gh) };
        let crop = |px: &[u8], w: i32| plat::Image {
            w: around.right - around.left,
            h: around.bottom - around.top,
            px: (around.top..around.bottom).flat_map(|y| px[((y * w + around.left) * 4) as usize..((y * w + around.right) * 4) as usize].to_vec()).collect(),
        };
        let name = if via_overlay { "gif-first-overlay.png" } else { "gif-first.png" };
        std::fs::write(tmp.join(name), plat::encode(&crop(&frame, gw), false).unwrap()).unwrap();
        // 色块是钉图这种窗口，从遮罩来时也一直在任务栏上面：第一帧是绿的（不是控制条、也不是变暗的遮罩）
        assert!(left < 12.0 && near(first, green), "那块不是开始前的样子");
        // 最后一帧：那块跟着变成橙的
        let frames = {
            let mut o = gif::DecodeOptions::new();
            o.set_color_output(gif::ColorOutput::RGBA);
            let mut d = o.read_info(std::fs::File::open(&path).unwrap()).unwrap();
            std::iter::from_fn(|| d.read_next_frame().unwrap().map(|_| ())).count()
        };
        for _ in 1..frames {
            p.next(&mut whole).unwrap();
        }
        let last = mean(&inner(&whole), gw, local);
        println!("GIF 共 {frames} 帧，最后一帧控制条那块平均色 {last:06X}");
        assert!(near(last, orange), "控制条底下那块没跟着变（是冻住的旧画面）：{last:06X}");
        // 色块和录完弹出的结果都是钉图这种窗口，一起关
        for w in Win::all(plat::Kind::Pin) {
            w.close();
        }
        cleanup();
    }

    /// 真的录一小块屏幕（鼠标所在显示器左上角往里 100 的 200 × 120）0.7 秒，编成 GIF 再解出来；打印帧数和大小。
    /// 会截到屏幕内容，文件放 target/test-tmp/record，平时不跑：cargo test record_region -- --ignored --nocapture
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn record_region() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("record");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("region.gif");
        let m = plat::monitor_at(plat::cursor_pos()).rect;
        let r = Rect { left: m.left + 100, top: m.top + 100, right: m.left + 300, bottom: m.top + 220 };
        let shared = Arc::new(Mutex::new(Shared { writer: Some(Writer::create(&path, 200, 120).unwrap()), error: None }));
        let (sink, count) = (shared.clone(), Arc::new(AtomicU64::new(0)));
        let n = count.clone();
        let rec = plat::record(
            r,
            15,
            true,
            Box::new(move |px, stride, t| {
                n.fetch_add(1, Ordering::Relaxed);
                lock(&sink).writer.as_mut().unwrap().push(px, stride, t).unwrap();
            }),
            |_| {},
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(700));
        rec.stop();
        let calls = count.load(Ordering::Relaxed);
        let frames = lock(&shared).writer.take().unwrap().finish().unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        println!("0.7 秒：截了 {calls} 帧，GIF 里 {frames} 帧，{size} 字节");
        // 第一帧等一个间隔，0.7 秒 15 fps 应该截到 9 帧左右
        assert!((7..=11).contains(&calls), "{calls}");
        let mut p = crate::anim::Player::open(&path).unwrap();
        assert_eq!((p.w, p.h), (200, 120));
        let mut canvas = vec![0u8; 200 * 120 * 4];
        p.next(&mut canvas).unwrap();
        assert!(canvas.chunks_exact(4).all(|c| c[3] == 255));
    }

    // ---- 9.6 实测：三种画面各录 30 秒（1280 × 720），记帧率、CPU、内存、文件大小；再同时开 5 个动图钉图量内存 ----
    // 会在屏幕中间开窗口、录屏，要用发布版跑，平时不跑：cargo test --release measure_ -- --ignored --nocapture --test-threads 1

    /// 进程的私有内存（字节）
    #[cfg(windows)]
    fn private_bytes() -> usize {
        use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
        use windows::Win32::System::Threading::GetCurrentProcess;
        let mut m = PROCESS_MEMORY_COUNTERS_EX { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
        let _ = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut m as *mut _ as *mut PROCESS_MEMORY_COUNTERS, m.cb) };
        m.PrivateUsage
    }

    /// 进程到现在用掉的 CPU 时间（内核 + 用户）
    #[cfg(windows)]
    fn cpu_time() -> Duration {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let mut f = [FILETIME::default(); 4];
        let [a, b, k, u] = &mut f;
        let _ = unsafe { GetProcessTimes(GetCurrentProcess(), a, b, k, u) };
        let n = |t: &FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
        Duration::from_nanos((n(&f[2]) + n(&f[3])) * 100)
    }

    #[cfg(windows)]
    fn mb(n: usize) -> f64 {
        n as f64 / (1 << 20) as f64
    }

    #[cfg(windows)]
    #[derive(Clone, Copy, PartialEq)]
    enum Scene {
        /// 屏幕上原来是什么就录什么（多半是静止的界面）
        Still,
        /// 一张真实的屏幕截图每秒往上滚 400 像素
        Scroll,
        /// 每秒 30 次重画的流动彩色图案：整块都在变、颜色很多，当成播放视频
        Video,
    }

    /// 测量用的画面窗口
    #[cfg(windows)]
    struct Show {
        scene: Scene,
        t0: Instant,
        shot: Option<plat::Bitmap>,
        sin: Vec<usize>,
        buf: Vec<u8>,
        w: i32,
        h: i32,
    }

    #[cfg(windows)]
    impl Handler for Show {
        fn event(&mut self, w: Win, e: Event) {
            if let Event::Timer(1) = e {
                w.invalidate_all();
            }
        }

        fn paint(&mut self, _w: Win, c: &Canvas) {
            let t = self.t0.elapsed().as_secs_f32();
            let (w, h) = (self.w as usize, self.h as usize);
            if let (Scene::Scroll, Some(s)) = (self.scene, &self.shot) {
                let x0 = (s.w - self.w) / 2;
                let off = (t * 400.0) as i32 % s.h;
                c.blit(s, Rect { left: x0, top: off, right: x0 + self.w, bottom: s.h }, 0, 0);
                c.blit(s, Rect { left: x0, top: 0, right: x0 + self.w, bottom: off }, 0, s.h - off);
                return;
            }
            let k = (t * 60.0) as usize;
            for y in 0..h {
                for x in 0..w {
                    let v = self.sin[(x / 5 + k) & 255] + self.sin[(y / 4 + 2 * k) & 255] + self.sin[((x + y) / 7 + 3 * k) & 255];
                    let i = (y * w + x) * 4;
                    self.buf[i..i + 4].copy_from_slice(&[(v / 3) as u8, self.sin[(v / 2 + k) & 255] as u8, (255 - v / 3) as u8, 255]);
                }
            }
            c.pixels(&self.buf, w as u32, h as u32, rectf(0.0, 0.0, w as f32, h as f32));
        }
    }

    /// 录一种画面 30 秒：和 record::start 开始以后一样，截屏线程上边截边编成 GIF。结果打印出来、存进 target/test-tmp/measure
    #[cfg(windows)]
    fn measure(scene: Scene, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("measure");
        std::fs::create_dir_all(&dir).unwrap();
        let pump = |ms: u64| {
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_millis(ms) {
                if !plat::pump() {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        };
        let mon = plat::monitor_at(plat::cursor_pos());
        let (w, h) = (1280, 720);
        let (cx, cy) = ((mon.work.left + mon.work.right) / 2, (mon.work.top + mon.work.bottom) / 2);
        let region = Rect { left: cx - w / 2, top: cy - h / 2, right: cx + w / 2, bottom: cy + h / 2 };
        let idle = private_bytes();
        let show = (scene != Scene::Still).then(|| {
            let shot = (scene == Scene::Scroll).then(|| plat::capture_cursor_monitor().unwrap().1);
            let sin = (0..256).map(|i| ((i as f32 / 256.0 * std::f32::consts::TAU).sin() * 127.5 + 127.5) as usize).collect();
            let s = Show { scene, t0: Instant::now(), shot, sin, buf: vec![0; (w * h * 4) as usize], w, h };
            let win = Win::open(Spec { kind: plat::Kind::Card, rect: region, title: "Jie 测量画面", owner: None }, s).unwrap();
            let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::ShowWindow(win.hwnd(), windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE) };
            win.timer(1, 33);
            win
        });
        pump(2000);
        // 基线：画面照常在动，还没录
        let (c0, t0) = (cpu_time(), Instant::now());
        pump(3000);
        let base = (cpu_time() - c0).as_secs_f64() / t0.elapsed().as_secs_f64();
        let before = private_bytes();
        let path = dir.join(format!("{name}.gif"));
        let shared = Arc::new(Mutex::new(Shared { writer: Some(Writer::create(&path, w, h).unwrap()), error: None }));
        let (sink, calls, busy) = (shared.clone(), Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        let (n, b) = (calls.clone(), busy.clone());
        let fps = settings::get().record_fps;
        let (c1, t1) = (cpu_time(), Instant::now());
        let rec = plat::record(
            region,
            fps,
            true,
            Box::new(move |px, stride, t| {
                let s = Instant::now();
                lock(&sink).writer.as_mut().unwrap().push(px, stride, t).unwrap();
                n.fetch_add(1, Ordering::Relaxed);
                b.fetch_add(s.elapsed().as_micros() as u64, Ordering::Relaxed);
            }),
            |_| {},
        )
        .unwrap();
        let mut samples = Vec::new();
        while t1.elapsed() < Duration::from_secs(30) {
            pump(500);
            samples.push(private_bytes());
        }
        rec.stop();
        let secs = t1.elapsed().as_secs_f64();
        let cpu = (cpu_time() - c1).as_secs_f64() / secs;
        let frames = lock(&shared).writer.take().unwrap().finish().unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        let done = private_bytes();
        if let Some(win) = show {
            win.close();
        }
        pump(500);
        let end = private_bytes();
        let calls = calls.load(Ordering::Relaxed);
        let peak = *samples.iter().max().unwrap();
        let avg = |v: &[usize]| v.iter().sum::<usize>() / v.len();
        let (first, last) = (avg(&samples[..10]), avg(&samples[samples.len() - 10..]));
        // GIF 无限循环：NETSCAPE2.0 扩展里的循环次数是 0
        let bytes = std::fs::read(&path).unwrap();
        let ext = bytes.windows(11).position(|x| x == b"NETSCAPE2.0").map(|i| &bytes[i + 11..i + 16]);
        let looping = ext.is_some_and(|e| e[0] == 3 && e[1] == 1 && e[2] == 0 && e[3] == 0);
        let report = format!(
            "{name}：{w}×{h} 录 {secs:.1} 秒（设的 {fps} fps）\n\
             实际截了 {calls} 帧 = {:.1} fps，GIF 里 {frames} 帧（没变的帧合并），每帧编码平均 {:.1} ms\n\
             文件 {:.2} MB（{:.0} KB/秒）；无限循环：{looping}\n\
             CPU（占一个核的比例）：录制中 {:.0}%，减去画面本身 {:.0}% 后录屏占 {:.0}%\n\
             私有内存：空闲 {:.1} MB，开始录前 {:.1} MB，录制中峰值 {:.1} MB（多 {:.1} MB），前 5 秒均值 {:.1} → 最后 5 秒均值 {:.1} MB\n\
             停止后 {:.1} MB，关掉画面窗口后 {:.1} MB\n",
            calls as f64 / secs,
            busy.load(Ordering::Relaxed) as f64 / calls.max(1) as f64 / 1000.0,
            size as f64 / (1 << 20) as f64,
            size as f64 / 1024.0 / secs,
            cpu * 100.0,
            base * 100.0,
            (cpu - base) * 100.0,
            mb(idle),
            mb(before),
            mb(peak),
            mb(peak.saturating_sub(before)),
            mb(first),
            mb(last),
            mb(done),
            mb(end),
        );
        print!("{report}");
        std::fs::write(dir.join(format!("{name}.txt")), report).unwrap();
        assert!(looping, "GIF 不是无限循环");
    }

    #[test]
    #[ignore]
    #[cfg(windows)]
    fn measure_1_still() {
        measure(Scene::Still, "still");
    }

    #[test]
    #[ignore]
    #[cfg(windows)]
    fn measure_2_scroll() {
        measure(Scene::Scroll, "scroll");
    }

    #[test]
    #[ignore]
    #[cfg(windows)]
    fn measure_3_video() {
        measure(Scene::Video, "video");
    }

    /// 同时开 5 个动图钉图（滚动那段 1280 × 720 的 GIF），播 3 秒，量每个多占多少内存，和「≤ 2 帧图片数据」比；关掉以后回到哪
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn measure_4_pins() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("measure");
        let path = dir.join("scroll.gif");
        assert!(path.exists(), "先跑 measure_2_scroll");
        let pump = |ms: u64| {
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_millis(ms) {
                if !plat::pump() {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        };
        pump(300);
        let before = private_bytes();
        let m = plat::monitor_at(plat::cursor_pos()).work;
        for i in 0..5 {
            let at = Point { x: m.left + 40 + i * 120, y: m.top + 40 + i * 80 };
            pin::create_gif(path.clone(), at, None).unwrap();
        }
        let (c0, t0) = (cpu_time(), Instant::now());
        pump(3000);
        let cpu = (cpu_time() - c0).as_secs_f64() / t0.elapsed().as_secs_f64();
        let open = private_bytes();
        let pins = Win::all(plat::Kind::Pin);
        assert_eq!(pins.len(), 5);
        pins.into_iter().for_each(Win::close);
        pump(500);
        let after = private_bytes();
        let frame = 1280.0 * 720.0 * 4.0 / (1 << 20) as f64;
        let each = mb(open.saturating_sub(before)) / 5.0;
        let report = format!(
            "5 个动图钉图（1280×720）：开之前 {:.1} MB，开着 {:.1} MB，每个多 {each:.1} MB（一帧图片数据 {frame:.1} MB，上限 2 帧 = {:.1} MB）；\
             5 个一起播 CPU {:.0}%；全关掉后 {:.1} MB\n",
            mb(before),
            mb(open),
            frame * 2.0,
            cpu * 100.0,
            mb(after),
        );
        print!("{report}");
        std::fs::write(dir.join("pins.txt"), report).unwrap();
    }

    /// 只测编码器：不开窗口、不截屏，把「视频」那种画面直接编 450 帧（30 秒的量），每 30 帧记一次私有内存，看涨不涨
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn measure_5_encoder() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("measure");
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (1280usize, 720usize);
        let sin: Vec<usize> = (0..256).map(|i| ((i as f32 / 256.0 * std::f32::consts::TAU).sin() * 127.5 + 127.5) as usize).collect();
        let mut buf = vec![0u8; w * h * 4];
        let idle = private_bytes();
        let mut wr = Writer::create(&dir.join("encoder.gif"), w as i32, h as i32).unwrap();
        let mut line = Vec::new();
        for f in 0..450usize {
            let k = f * 4;
            for y in 0..h {
                for x in 0..w {
                    let v = sin[(x / 5 + k) & 255] + sin[(y / 4 + 2 * k) & 255] + sin[((x + y) / 7 + 3 * k) & 255];
                    let i = (y * w + x) * 4;
                    buf[i..i + 4].copy_from_slice(&[(v / 3) as u8, sin[(v / 2 + k) & 255] as u8, (255 - v / 3) as u8, 255]);
                }
            }
            wr.push(&buf, w * 4, Duration::from_millis(f as u64 * 66)).unwrap();
            if f % 30 == 29 {
                line.push(format!("{:.1}", mb(private_bytes())));
            }
        }
        wr.finish().unwrap();
        let done = private_bytes();
        let report = format!("只编码 450 帧 1280×720「视频」：空闲 {:.1} MB，每 30 帧：{} MB，写完 {:.1} MB\n", mb(idle), line.join(" "), mb(done));
        print!("{report}");
        std::fs::write(dir.join("encoder.txt"), report).unwrap();
    }
}
