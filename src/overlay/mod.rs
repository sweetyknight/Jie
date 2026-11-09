//! 截图遮罩：冻结画面、框选、吸附（页面里的边界、窗口、整块屏幕）、操作栏、标注。
//! 截图、录屏共用这一个遮罩，顶上的模式条（`modebar`）或 Tab 切换；两个快捷键只决定打开时是哪个模式。
//! 录屏模式只框选，框好后交给 record；有标注时切不到录屏（标注录不进去）。
//!
//! 坐标都是窗口内容区的物理像素，也就是截图里的像素。每次重画按要重画的那块现合成：
//! 暗图打底、亮区贴原图、画边框，选区里叠标注，再叠手柄、读数、操作栏这些装饰。

mod modebar;

use crate::annotate::{self, Doc, Editor, Pt, Tool};
use crate::card;
use crate::image_io;
use crate::longshot;
use crate::pin;
use crate::plat::{self, Bitmap, Button, Canvas, Cursor, Event, Handler, Image, Key, Kind as WinKind, Monitor, Point, Rect, Spec, Win};
use crate::record;
use crate::snap;
use crate::toast::{self, Kind};
use crate::toolbar::opts::{self, Item, Opts};
use crate::toolbar::{self, Act, Bar, act_of, tool_of};
use crate::ui::{self, Hud, rectf, rgb};
use modebar::ModeBar;
use std::time::Instant;

/// 8 个手柄，按 左上、上、右上、右、右下、下、左下、左；0/1/2 = 左（上）边、中间、右（下）边
const HANDLES: [(u8, u8); 8] = [(0, 0), (1, 0), (2, 0), (2, 1), (2, 2), (1, 2), (0, 2), (0, 1)];
/// 工具提示：悬停 400ms 后出现
const TIP_TIMER: usize = 1;

/// 遮罩现在是哪个模式
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    /// 截图：框选、标注、复制保存钉图取字翻译
    #[default]
    Shot,
    /// 录屏：只框选，选延时，主按钮是开始录制
    Record,
}

/// 操作栏和工具选项条上的按钮
#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Bar(Act),
    Opt(Item),
    /// 模式条的一段：false 截图，true 录屏
    Mode(bool),
}

/// 影响画面的状态；变了才重画，并且只重画变了的地方
#[derive(Clone, Copy, PartialEq, Default)]
struct View {
    mode: Mode,
    /// 能切到录屏：没有标注、没在输入文字
    rec_ok: bool,
    sel: Option<Rect>,
    /// 没有选区时鼠标下面的窗口：整块高亮，单击选中它
    hover: Option<Rect>,
    /// 正在框选新选区（已过拖拽阈值）：不画手柄
    framing: bool,
    /// 正在拖动选区（框选、移动、改大小）：不画操作栏
    dragging: bool,
    /// 鼠标下的按钮、按住的按钮、显示提示的按钮
    hot: Option<Btn>,
    down: Option<Btn>,
    tip: Option<Btn>,
    /// 标注编辑器的界面状态：当前工具、选项条（样子、有没有删除）、颜色和粗细、能不能撤销和重做
    tool: Option<Tool>,
    opt: Option<(annotate::Kind, bool)>,
    pen: (usize, usize),
    history: (bool, bool),
}

impl View {
    /// 不变暗的那块
    fn lit(&self) -> Option<Rect> {
        self.sel.or(self.hover)
    }
}

#[derive(Clone, Copy)]
enum Drag {
    None,
    /// 框选：按下的点；moved 表示移动已超过阈值
    New { p0: Point, moved: bool },
    Move { p0: Point, s0: Rect, moved: bool },
    Resize { handle: usize, s0: Rect },
    /// 按住了按钮，松开时还在它上面才算点击
    Button(Btn),
    /// 标注编辑器接手的拖动（画图形、移动标注）
    Edit,
}

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    /// 操作栏、工具选项条上（可能在按钮之间的空隙）
    Bar(Option<Act>),
    Opt(Option<Item>),
    Mode(Option<bool>),
    Handle(usize),
    Inside,
    Outside,
}

/// 画在底图上面的东西
enum Deco {
    /// 手柄圆心（浮点像素）
    Handle(f32, f32),
    Hud(Hud, i32, i32),
    Bar(Bar),
    Opts(Opts),
    Mode(ModeBar),
}

struct Overlay {
    mon: Monitor,
    frame: Bitmap,
    dim: Bitmap,
    /// 选区颜色（设置里改，每次截图时读）
    sel: u32,
    /// 吸附用的窗口，从上到下
    windows: Vec<Rect>,
    /// 鼠标下一层层的边界（从小到大，最后是窗口、整块屏幕），现在高亮第几层；滚轮换层，攒够 120 才走一格
    snaps: Vec<Rect>,
    level: usize,
    wheel_acc: i32,
    /// 层是滚轮换过的：鼠标挪动后新的一组里还有这个框，就留着它
    picked: bool,
    /// 标注；坐标就是截图的像素
    ed: Editor,
    v: View,
    drag: Drag,
}

/// 快捷键、托盘入口：冻结鼠标所在的显示器并打开遮罩；已经开着就不再开
pub fn show(mode: Mode) {
    if CAPTURING.with(|p| p.get()) || !Win::all(WinKind::Overlay).is_empty() {
        return;
    }
    let t0 = Instant::now();
    CAPTURING.with(|p| p.set(true));
    plat::request_capture(move |result| {
        CAPTURING.with(|p| p.set(false));
        if let Some((mon, frame)) = result {
            show_frame(mode, mon, frame, t0);
        }
    });
}

thread_local! {
    static CAPTURING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// 录屏延时开始几秒（0 是点了就录）；本次运行内记住，每次启动从不延时开始
    static DELAY: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

pub(crate) fn show_frame(mode: Mode, mon: Monitor, frame: Bitmap, t0: Instant) {
    let windows = plat::window_rects(&mon);
    let t1 = t0.elapsed();
    let Some(dim) = frame.dimmed() else { return };
    let t2 = t0.elapsed();
    let s = mon.scale();
    let (w, h) = (frame.w, frame.h);
    // 录屏选区和截图一样用设置里的选区颜色，录制中的边框也是它
    let sel = crate::settings::sel_color();
    let ed = Editor::new(Doc::default(), s);
    let v = View { mode, rec_ok: true, ..View::default() };
    let mut state = Overlay { mon, frame, dim, sel, windows, snaps: Vec::new(), level: 0, wheel_acc: 0, picked: false, ed, v, drag: Drag::None };
    let c = plat::cursor_pos();
    state.hover_at(Point { x: c.x - mon.rect.left, y: c.y - mon.rect.top });
    let title = if mode == Mode::Record { "Jie 录屏" } else { "Jie 截图" };
    let Some(win) = Win::open(Spec { kind: WinKind::Overlay, rect: mon.rect, title, owner: None }, state) else { return };
    // 显示时当场画完第一帧
    win.show();
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    eprintln!("截图 {w}×{h}：截屏 {:.1} ms，暗图 {:.1} ms，画完首帧 {:.1} ms", ms(t1), ms(t2 - t1), ms(t0.elapsed()));
}

fn pt(p: Point) -> Pt {
    Pt::new(p.x as f32, p.y as f32)
}

fn inflate(r: Rect, d: i32) -> Rect {
    Rect { left: r.left - d, top: r.top - d, right: r.right + d, bottom: r.bottom + d }
}

fn contains(r: &Rect, p: Point) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

fn intersect(a: &Rect, b: &Rect) -> Option<Rect> {
    let r = Rect { left: a.left.max(b.left), top: a.top.max(b.top), right: a.right.min(b.right), bottom: a.bottom.min(b.bottom) };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

/// a 去掉 b 剩下的部分，拆成最多 4 个矩形
fn minus(a: Rect, b: Rect) -> Vec<Rect> {
    let Some(i) = intersect(&a, &b) else { return vec![a] };
    [
        Rect { bottom: i.top, ..a },
        Rect { top: i.bottom, ..a },
        Rect { left: a.left, top: i.top, right: i.left, bottom: i.bottom },
        Rect { left: i.right, top: i.top, right: a.right, bottom: i.bottom },
    ]
    .into_iter()
    .filter(|r| r.right > r.left && r.bottom > r.top)
    .collect()
}

fn dist(a: Point, b: Point) -> f32 {
    ((a.x - b.x) as f32).hypot((a.y - b.y) as f32)
}

/// 四条边整理成正向矩形，夹在 w × h 里；没有面积时为 None
fn norm(x0: i32, y0: i32, x1: i32, y1: i32, w: i32, h: i32) -> Option<Rect> {
    let (x0, x1, y0, y1) = (x0.clamp(0, w), x1.clamp(0, w), y0.clamp(0, h), y1.clamp(0, h));
    let r = Rect { left: x0.min(x1), top: y0.min(y1), right: x0.max(x1), bottom: y0.max(y1) };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

/// 拖手柄改大小；拖过对边会翻过去，压成一条线时保持原样
fn resize(s0: Rect, handle: usize, p: Point, w: i32, h: i32) -> Option<Rect> {
    let (hx, hy) = HANDLES[handle];
    let (mut l, mut t, mut r, mut b) = (s0.left, s0.top, s0.right, s0.bottom);
    match hx {
        0 => l = p.x,
        2 => r = p.x,
        _ => {}
    }
    match hy {
        0 => t = p.y,
        2 => b = p.y,
        _ => {}
    }
    norm(l, t, r, b, w, h)
}

/// 平移选区，整块留在画面里
fn shift(s0: Rect, dx: i32, dy: i32, w: i32, h: i32) -> Rect {
    let (sw, sh) = (s0.right - s0.left, s0.bottom - s0.top);
    let (l, t) = ((s0.left + dx).clamp(0, w - sw), (s0.top + dy).clamp(0, h - sh));
    Rect { left: l, top: t, right: l + sw, bottom: t + sh }
}

/// 手柄中心：选区边框（在选区外侧，`t` 像素宽）的中心线上
fn handle_center(r: Rect, i: usize, t: i32) -> (f32, f32) {
    let h = t as f32 / 2.0;
    let pick = |k: u8, a: i32, b: i32| match k {
        0 => a as f32 - h,
        1 => (a + b) as f32 / 2.0,
        _ => b as f32 + h,
    };
    let (hx, hy) = HANDLES[i];
    (pick(hx, r.left, r.right), pick(hy, r.top, r.bottom))
}

/// 只让变了的像素重画：新旧亮区的异或，加上新旧两圈边框（选区边框在外侧 `t`，窗口吸附的边框在内侧 2）
fn invalidate_change(w: Win, old: Option<Rect>, new: Option<Rect>, t: i32) {
    let mut v = Vec::new();
    for r in [old, new].into_iter().flatten() {
        v.extend(minus(inflate(r, t), inflate(r, -2)));
    }
    match (old, new) {
        (Some(a), Some(b)) => {
            v.extend(minus(a, b));
            v.extend(minus(b, a));
        }
        (a, b) => v.extend(a.or(b)),
    }
    v.into_iter().for_each(|r| w.invalidate(r));
}

impl Deco {
    fn rect(&self, scale: f32) -> Rect {
        match self {
            Deco::Handle(x, y) => {
                let r = 5.0 * scale + 1.0;
                Rect { left: (x - r).floor() as i32, top: (y - r).floor() as i32, right: (x + r).ceil() as i32, bottom: (y + r).ceil() as i32 }
            }
            Deco::Hud(hud, x, y) => Rect { left: *x, top: *y, right: x + hud.w, bottom: y + hud.h },
            Deco::Bar(b) => b.rect,
            Deco::Opts(o) => o.rect,
            Deco::Mode(m) => m.rect,
        }
    }
}

impl Overlay {
    fn px(&self, dip: f32) -> f32 {
        dip * self.mon.scale()
    }

    /// 选区边框宽（像素）：2 DIP，画在选区外侧
    fn border(&self) -> i32 {
        ui::line_w(self.mon.scale()) as i32
    }

    /// 点下面一层层的边界：窗口里认出来的面板、卡片这些（从小到大），然后是最上层的窗口，最外面是整块屏幕；
    /// 录屏时窗口和整块屏幕之间还有「屏幕去掉任务栏」（录它时控制条放在任务栏上，见 record.rs）。
    /// 和默认高亮的那层（主要的面板，没有就是窗口）
    fn snaps_at(&self, p: Point) -> (Vec<Rect>, usize) {
        let full = Rect { left: 0, top: 0, right: self.frame.w, bottom: self.frame.h };
        let win = self.windows.iter().find(|r| contains(r, p)).copied().unwrap_or(full);
        let mut v = snap::boxes(self.frame.pixels(), self.frame.w, self.frame.h, p, win, self.mon.scale());
        let def = snap::default_level(&v, &win, self.mon.scale());
        let (m, wk) = (self.mon.rect, self.mon.work);
        let work = Rect { left: wk.left - m.left, top: wk.top - m.top, right: wk.right - m.left, bottom: wk.bottom - m.top };
        // 盖住任务栏的窗口（全屏应用）外面不套工作区，免得一层比一层小
        let around = |o: &Rect, i: &Rect| o.left <= i.left && o.top <= i.top && o.right >= i.right && o.bottom >= i.bottom;
        let work = (self.v.mode == Mode::Record && around(&work, &win)).then_some(work);
        for r in [Some(win), work, Some(full)].into_iter().flatten() {
            if v.last().is_none_or(|l| !snap::same(l, &r)) {
                v.push(r);
            }
        }
        // 认出来的最外层可能已经和窗口差不多大，窗口那层就是它
        let def = def.unwrap_or_else(|| v.iter().position(|r| snap::same(r, &win)).unwrap_or(v.len() - 1));
        (v, def)
    }

    /// 没有选区时鼠标挪到 p：换了一组边界就高亮默认那层，滚轮换过的那个框新的一组里也有就留着；还是那组就不动
    fn hover_at(&mut self, p: Point) {
        let (v, def) = self.snaps_at(p);
        if v != self.snaps {
            let kept = self.v.hover.filter(|_| self.picked).and_then(|h| v.iter().position(|r| snap::same(r, &h)));
            self.picked = kept.is_some();
            (self.snaps, self.level) = (v, kept.unwrap_or(def));
        }
        self.v.hover = self.snaps.get(self.level).copied();
    }

    /// 滚轮：往上（离开用户）换外面一层，往下换里面一层
    fn wheel(&mut self, w: Win, delta: i32) {
        self.wheel_acc += delta;
        let n = self.wheel_acc / 120;
        if n == 0 || self.snaps.is_empty() {
            return;
        }
        self.wheel_acc %= 120;
        let level = (self.level as i32 + n).clamp(0, self.snaps.len() as i32 - 1) as usize;
        self.update(w, |s| {
            (s.level, s.picked) = (level, true);
            s.v.hover = s.snaps.get(level).copied();
        });
    }

    /// 有选区、没在拖时才有操作栏；有工具选项条时给它留出位置
    fn bar(&self) -> Option<Bar> {
        let sel = self.v.sel?;
        if self.v.dragging {
            return None;
        }
        let s = self.mon.scale();
        let opt_h = self.ed.opts().map_or(0, |(k, t)| toolbar::opts_height(k, t, s));
        let rec = self.v.mode == Mode::Record;
        let items: &'static toolbar::Items = if rec { &toolbar::RECORD } else { &toolbar::SHOT };
        // 选区边框在外侧，操作栏从边框外面算
        Some(Bar::layout(inflate(sel, self.border()), self.frame.w, self.frame.h, s, opt_h, items, rec.then_some("延时")))
    }

    /// 工具选项条：跟着当前工具，见 `Bar::opts`
    fn opts(&self, bar: &Bar) -> Option<Opts> {
        let (kind, trash) = self.ed.opts()?;
        Some(bar.opts(kind, trash, self.ed.tool, self.frame.w))
    }

    /// 撤销、重做没有记录时置灰
    fn enabled(&self, a: Act) -> bool {
        match a {
            Act::Undo => self.ed.doc.can_undo(),
            Act::Redo => self.ed.doc.can_redo(),
            // 标注跟不了滚动的内容：有标注时和录屏一样不能用
            Act::Long => self.v.rec_ok,
            _ => true,
        }
    }

    /// 编辑器的界面状态抄进 View，变了才会重画操作栏和选项条
    fn sync(&mut self) {
        let ed = &self.ed;
        (self.v.tool, self.v.opt, self.v.pen, self.v.history) = (ed.tool, ed.opts(), ed.pen(), (ed.doc.can_undo(), ed.doc.can_redo()));
        self.v.rec_ok = ed.doc.anns.is_empty() && ed.text.is_none();
    }

    fn hit_at(&self, p: Point) -> Hit {
        if let Some(b) = self.bar() {
            if let Some(o) = self.opts(&b)
                && contains(&o.rect, p)
            {
                return Hit::Opt(o.hit(p));
            }
            if contains(&b.rect, p) {
                return Hit::Bar(b.hit(p));
            }
        }
        if let Some(m) = self.mode_bar()
            && contains(&m.rect, p)
        {
            return Hit::Mode(m.hit(p));
        }
        let Some(r) = self.v.sel else { return Hit::Outside };
        // 选了标注工具后选区锁定，没有手柄
        if self.ed.tool.is_none() {
            let reach = self.px(8.0); // 点击范围 16 × 16
            for i in 0..8 {
                let (x, y) = handle_center(r, i, self.border());
                if (p.x as f32 + 0.5 - x).abs() <= reach && (p.y as f32 + 0.5 - y).abs() <= reach {
                    return Hit::Handle(i);
                }
            }
        }
        if contains(&r, p) { Hit::Inside } else { Hit::Outside }
    }

    /// 尺寸读数：选区左上角上方 6 处；顶上放不下就放进选区左上角里面。
    /// 录屏选区前面标「录屏」；还没选、鼠标下有好几层时后面提示滚轮能换
    fn readout(&self) -> Option<Deco> {
        let r = self.v.lit()?;
        let size = format!("{} × {}", r.right - r.left, r.bottom - r.top);
        let text = if self.v.mode == Mode::Record { format!("录屏 · {size}") } else { size };
        let mut parts = vec![(text.as_str(), ui::INK)];
        if self.v.sel.is_none() && self.snaps.len() > 1 {
            parts.push(("滚轮调整范围", ui::MUTED));
        }
        let hud = Hud::readout(&parts, self.mon.scale())?;
        let (m, g) = (self.px(4.0) as i32, self.px(6.0) as i32 + self.border());
        let (mut x, mut y) = (r.left, r.top - g - hud.h);
        if y < m {
            (x, y) = (r.left + g, r.top + g);
        }
        let x = x.min(self.frame.w - hud.w - m).max(m);
        Some(Deco::Hud(hud, x, y))
    }

    /// 工具提示：按钮下方 6 处、水平居中；下面放不下就放上方
    fn tooltip(&self, bar: Option<&Bar>, opts: Option<&Opts>, mb: Option<&ModeBar>) -> Option<Deco> {
        let delayed: String;
        let (b, (name, key)) = match self.v.tip? {
            // 写字的延时分段、能点的模式段不用提示
            Btn::Bar(Act::Delay(_)) | Btn::Mode(false) => return None,
            Btn::Mode(true) if self.v.rec_ok => return None,
            Btn::Mode(true) => (mb?.seg(true), ("有标注时不能切到录屏", "撤销标注后可切换")),
            Btn::Bar(Act::Record) if DELAY.get() > 0 => {
                delayed = format!("{} 秒后开始录制", DELAY.get());
                (bar?.button(Act::Record), (delayed.as_str(), "Enter"))
            }
            Btn::Bar(a) => (bar?.button(a), bar?.tip(a)),
            Btn::Opt(i) => (opts?.button(i), opts::tip(self.ed.opts()?.0, i)),
        };
        let parts: &[(&str, u32)] = if key.is_empty() { &[(name, 0xFFFFFF)] } else { &[(name, 0xFFFFFF), (key, ui::HUD_TEXT2)] };
        let hud = Hud::new(parts, self.mon.scale())?;
        let (m, g) = (self.px(4.0) as i32, self.px(6.0) as i32);
        let mut y = b.bottom + g;
        if y + hud.h > self.frame.h - m {
            y = b.top - g - hud.h;
        }
        let x = ((b.left + b.right - hud.w) / 2).min(self.frame.w - hud.w - m).max(m);
        Some(Deco::Hud(hud, x, y))
    }

    /// 模式条：工作区顶上 18 处居中；挡着选区（连手柄）、读数或操作栏时挪到工作区底下 18 处，上下都挡就不出；拖选区时收起
    fn mode_bar(&self) -> Option<ModeBar> {
        if self.v.dragging {
            return None;
        }
        let (rec, cx, s, g) = (self.v.mode == Mode::Record, self.frame.w / 2, self.mon.scale(), self.px(18.0) as i32);
        let m = ModeBar::layout(rec, cx, self.mon.work.top - self.mon.rect.top + g, s)?;
        let Some(sel) = self.v.sel else { return Some(m) };
        let bar = self.bar();
        let busy = [
            Some(inflate(sel, self.border() + self.px(8.0) as i32)),
            self.readout().map(|d| d.rect(s)),
            bar.as_ref().map(|b| b.rect),
            bar.as_ref().and_then(|b| self.opts(b)).map(|o| o.rect),
        ];
        let free = |m: &ModeBar| busy.iter().flatten().all(|r| intersect(r, &m.rect).is_none());
        if free(&m) {
            return Some(m);
        }
        let m = ModeBar::layout(rec, cx, self.mon.work.bottom - self.mon.rect.top - g - (m.rect.bottom - m.rect.top), s)?;
        free(&m).then_some(m)
    }

    fn decos(&self) -> Vec<Deco> {
        let (mb, bar) = (self.mode_bar(), self.bar());
        let opts = bar.as_ref().and_then(|b| self.opts(b));
        let tip = self.tooltip(bar.as_ref(), opts.as_ref(), mb.as_ref());
        let mut v: Vec<Deco> = mb.into_iter().map(Deco::Mode).collect();
        if let Some(r) = self.v.sel
            && !self.v.framing
            && self.ed.tool.is_none()
        {
            let t = self.border();
            v.extend((0..8).map(|i| {
                let (x, y) = handle_center(r, i, t);
                Deco::Handle(x, y)
            }));
        }
        v.extend(self.readout());
        v.extend(bar.map(Deco::Bar));
        v.extend(opts.map(Deco::Opts));
        // 提示最后画，盖在选项条上面
        v.extend(tip);
        v
    }

    fn deco_rects(&self) -> Vec<Rect> {
        let s = self.mon.scale();
        self.decos().iter().map(|d| d.rect(s)).collect()
    }

    fn draw_deco(&self, c: &Canvas, d: &Deco) {
        let s = self.mon.scale();
        let bar_btn = |b: Option<Btn>| if let Some(Btn::Bar(a)) = b { Some(a) } else { None };
        let opt_btn = |b: Option<Btn>| if let Some(Btn::Opt(i)) = b { Some(i) } else { None };
        let mode_btn = |b: Option<Btn>| if let Some(Btn::Mode(r)) = b { Some(r) } else { None };
        match d {
            Deco::Handle(x, y) => {
                // 10 × 10 白底方块，2 选区颜色描边，圆角 2
                let (h, lw) = (5.0 * s, ui::line_w(s));
                let r = rectf(x - h, y - h, x + h, y + h);
                c.fill_round(r, 2.0 * s, rgb(0xFFFFFF));
                c.stroke_round(rectf(r.left + lw / 2.0, r.top + lw / 2.0, r.right - lw / 2.0, r.bottom - lw / 2.0), 1.5 * s, rgb(self.sel), lw);
            }
            Deco::Hud(hud, x, y) => hud.draw(c, *x, *y),
            Deco::Bar(b) => {
                // 录屏模式里「当前项」是选中的延时那段
                let on = if self.v.mode == Mode::Record { Some(Act::Delay(DELAY.get())) } else { self.ed.tool.map(act_of) };
                b.draw(c, bar_btn(self.v.hot), bar_btn(self.v.down), on, |a| self.enabled(a));
            }
            Deco::Opts(o) => o.draw(c, self.ed.pen(), opt_btn(self.v.hot), opt_btn(self.v.down)),
            Deco::Mode(m) => m.draw(c, mode_btn(self.v.hot), mode_btn(self.v.down), self.v.rec_ok),
        }
    }

    /// 改状态都走这里：前后对比，只把变了的地方标成要重画
    fn update(&mut self, w: Win, f: impl FnOnce(&mut Self)) {
        let before = (self.v, self.deco_rects(), self.ed.looks());
        f(self);
        self.updated(w, before);
    }

    /// `update` 改完以后：和改之前比，标重画。不跟着闭包泛型（十几个调用点各复制一份会多出好几 KB）
    #[inline(never)]
    fn updated(&mut self, w: Win, (v0, d0, l0): (View, Vec<Rect>, Vec<(Rect, usize, usize)>)) {
        self.sync();
        w.ime(self.ed.caret(|p| p));
        // 标注：变了的那几个，新旧范围都重画
        let l1 = self.ed.looks();
        for i in 0..l0.len().max(l1.len()) {
            if l0.get(i) != l1.get(i) {
                for (r, ..) in [l0.get(i), l1.get(i)].into_iter().flatten() {
                    w.invalidate(*r);
                }
            }
        }
        if self.v == v0 {
            return;
        }
        // 鼠标换了按钮：提示先收起，停 400ms 再出来
        if self.v.hot != v0.hot {
            self.v.tip = None;
            w.kill_timer(TIP_TIMER);
            if self.v.hot.is_some() && matches!(self.drag, Drag::None) {
                w.timer(TIP_TIMER, 400);
            }
        }
        // 亮区或边框样式（选区画在外侧、吸附画在内侧）变了
        if (self.v.lit(), self.v.sel.is_some()) != (v0.lit(), v0.sel.is_some()) {
            invalidate_change(w, v0.lit(), self.v.lit(), self.border());
        }
        for r in d0.iter().chain(&self.deco_rects()) {
            w.invalidate(*r);
        }
    }

    /// 合成 r 这块底图：暗图打底，亮区贴原图，再画边框
    fn compose(&self, c: &Canvas, r: Rect) {
        c.blit(&self.dim, r, r.left, r.top);
        let Some(l) = self.v.lit() else { return };
        if let Some(lit) = intersect(&r, &l) {
            c.blit(&self.frame, lit, lit.left, lit.top);
        }
        if self.v.sel.is_some() {
            for i in 1..=self.border() {
                ui::frame_px(c, inflate(l, i), self.sel);
            }
        } else {
            // 吸附高亮：2 宽，画在窗口内侧
            ui::frame_px(c, l, self.sel);
            ui::frame_px(c, inflate(l, -1), self.sel);
        }
    }

    fn mouse_down(&mut self, w: Win, p: Point) {
        let hit = self.hit_at(p);
        self.drag = match hit {
            Hit::Bar(Some(a)) if self.enabled(a) => Drag::Button(Btn::Bar(a)),
            Hit::Opt(Some(i)) => Drag::Button(Btn::Opt(i)),
            Hit::Mode(Some(r)) if !r || self.v.rec_ok => Drag::Button(Btn::Mode(r)),
            // 置灰的按钮、按钮之间的空隙：什么都不做
            Hit::Bar(_) | Hit::Opt(_) | Hit::Mode(_) => return,
            Hit::Handle(handle) => Drag::Resize { handle, s0: self.v.sel.unwrap() },
            _ => {
                // 先给标注编辑器：画图形、选中和拖动标注、结束文字输入
                let mut taken = false;
                self.update(w, |s| taken = s.v.sel.is_some() && s.ed.down(pt(p), hit == Hit::Inside));
                if taken {
                    Drag::Edit
                } else if hit == Hit::Inside {
                    Drag::Move { p0: p, s0: self.v.sel.unwrap(), moved: false }
                } else if self.ed.tool.is_none() && self.ed.doc.anns.is_empty() {
                    // 已有选区时在外面拖会换一个新选区；只是单击则保留原来的。有标注或选着工具时不换
                    Drag::New { p0: p, moved: false }
                } else {
                    return;
                }
            }
        };
        let drag = self.drag;
        self.update(w, |s| {
            s.v.tip = None;
            if let Drag::Button(b) = drag {
                s.v.down = Some(b);
            }
        });
        w.kill_timer(TIP_TIMER);
    }

    fn mouse_move(&mut self, w: Win, p: Point, square: bool) {
        let (fw, fh) = (self.frame.w, self.frame.h);
        let (new_thr, move_thr) = (self.px(4.0), self.px(2.0));
        let mut drag = self.drag;
        self.update(w, |s| match &mut drag {
            Drag::New { p0, moved } => {
                if *moved || dist(p, *p0) >= new_thr {
                    *moved = true;
                    s.v.sel = norm(p0.x, p0.y, p.x, p.y, fw, fh);
                    s.v.hover = None;
                    (s.v.framing, s.v.dragging) = (true, true);
                }
            }
            Drag::Move { p0, s0, moved } => {
                if *moved || dist(p, *p0) >= move_thr {
                    *moved = true;
                    s.v.sel = Some(shift(*s0, p.x - p0.x, p.y - p0.y, fw, fh));
                    s.v.dragging = true;
                }
            }
            Drag::Resize { handle, s0 } => {
                if let Some(r) = resize(*s0, *handle, p, fw, fh) {
                    s.v.sel = Some(r);
                    s.v.dragging = true;
                }
            }
            Drag::Edit => s.ed.drag_to(pt(p), square),
            Drag::Button(_) | Drag::None => {
                s.v.hot = match s.hit_at(p) {
                    Hit::Bar(a) => a.filter(|a| s.enabled(*a)).map(Btn::Bar),
                    Hit::Opt(i) => i.map(Btn::Opt),
                    // 置灰的「录屏」也算：鼠标停上去出提示
                    Hit::Mode(m) => m.map(Btn::Mode),
                    _ => None,
                };
                if s.v.sel.is_none() {
                    s.hover_at(p);
                }
            }
        });
        self.drag = drag;
    }

    fn mouse_up(&mut self, w: Win) {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        let min = self.px(3.0) as i32;
        let clicked = match drag {
            Drag::Button(b) if self.v.hot == Some(b) => Some(b),
            _ => None,
        };
        let cur = w.cursor_pos();
        self.update(w, |s| {
            (s.v.framing, s.v.dragging, s.v.down) = (false, false, None);
            match (drag, s.v.sel) {
                (Drag::Edit, _) => s.ed.up(),
                // 没拖动就是单击：选中高亮的窗口
                (Drag::New { moved: false, .. }, None) => s.v.sel = s.v.hover.take(),
                // 拖出来太小的选区（手抖）不算
                (Drag::New { moved: true, .. }, Some(r)) if r.right - r.left < min || r.bottom - r.top < min => {
                    s.v.sel = None;
                }
                _ => {}
            }
            if s.v.sel.is_none() {
                s.hover_at(cur);
            }
        });
        // 拖完松手时鼠标可能正停在刚出现的操作栏上
        self.mouse_move(w, cur, false);
        if let Some(b) = clicked {
            self.act(w, b);
        }
    }

    /// 操作栏、选项条上的按钮
    fn act(&mut self, w: Win, b: Btn) {
        let a = match b {
            Btn::Bar(a) => a,
            Btn::Mode(r) => return self.set_mode(w, if r { Mode::Record } else { Mode::Shot }),
            Btn::Opt(i) => {
                return self.update(w, |s| match i {
                    Item::Size(n) => s.ed.set_size(n),
                    Item::Color(c) => s.ed.set_color(c),
                    Item::Delete => _ = s.ed.delete(),
                });
            }
        };
        if let Some(t) = tool_of(a) {
            return self.update(w, |s| s.ed.set_tool(t));
        }
        match a {
            Act::Undo => self.update(w, |s| _ = s.ed.undo()),
            Act::Redo => self.update(w, |s| _ = s.ed.redo()),
            Act::Copy => self.copy(w),
            Act::Cancel => w.close(),
            Act::Save => self.save(w),
            Act::Pin => self.pin(w),
            Act::Record => self.record(w),
            Act::Long => self.long(w),
            Act::Delay(d) => {
                DELAY.set(d);
                // 选中的那段画在操作栏里面，操作栏的范围没变，自己标重画
                if let Some(b) = self.bar() {
                    w.invalidate(b.rect);
                }
            }
            Act::Ocr => self.recognize(w, true),
            Act::Translate => self.recognize(w, false),
            _ => {}
        }
    }

    /// 选区连同标注（不带选中框），设置里开了截图边框就外面加一圈；正在输入的文字先提交
    fn export(&mut self) -> Option<Image> {
        self.ed.finish_text();
        let sel = self.v.sel?;
        let out = self.frame.sub(&sel)?;
        self.ed.burn(&out, sel.left, sel.top, &self.frame);
        let st = crate::settings::get();
        Some(out.crop(&out.full()).framed(st.frame_px(self.mon.scale(), st.shot_frame), st.sel_color))
    }

    fn copy(&mut self, w: Win) {
        let Some(img) = self.export() else { return };
        // 先藏起来，编码 PNG 的时间用户感觉不到
        w.hide();
        match plat::copy_image(&img) {
            Ok(()) => toast::show(&self.mon, Kind::Ok, &format!("已复制 {} × {}", img.w, img.h)),
            Err(e) => toast::show(&self.mon, Kind::Err, &format!("复制失败：{e}")),
        }
        w.close();
    }

    /// 保存：对话框期间遮罩不能操作，写入成功后再关；取消或写入失败都保留截图。
    fn save(&mut self, w: Win) {
        // 正在输入的文字先提交，对话框后面看到的就是存下来的样子
        self.update(w, |s| s.ed.finish_text());
        let Some(img) = self.export() else { return };
        let mon = self.mon;
        image_io::ask_save(w, move |path| {
            toast::show(&mon, Kind::Busy, "正在保存…");
            image_io::save_async(img, path, mon, move || {
                w.with(|_: &mut Overlay| w.close());
            });
        });
    }

    /// 钉图保存原图和标注对象：标注挪到钉图自己的坐标里，撤销记录从钉图开始。先关遮罩释放截图帧，再出钉图
    fn pin(&mut self, w: Win) {
        self.ed.finish_text();
        let Some(r) = self.v.sel else { return };
        let Some(img) = self.frame.sub(&r) else { return };
        let mut anns = std::mem::take(&mut self.ed.doc.anns);
        anns.iter_mut().for_each(|a| a.offset(-r.left as f32, -r.top as f32));
        let (at, scale) = (Point { x: self.mon.rect.left + r.left, y: self.mon.rect.top + r.top }, self.mon.scale());
        w.close();
        Win::later(move || pin::create(img, at, Editor::new(Doc::with(anns), scale)));
    }

    /// 录屏：遮罩关掉（释放截图帧）以后，在选区这块活的屏幕上开始录（按「延时开始」选的先倒数）
    fn record(&mut self, w: Win) {
        let Some(r) = self.v.sel else { return };
        let (dx, dy, mon) = (self.mon.rect.left, self.mon.rect.top, self.mon);
        let region = Rect { left: r.left + dx, top: r.top + dy, right: r.right + dx, bottom: r.bottom + dy };
        w.close();
        let delay = std::time::Duration::from_secs(DELAY.get().into());
        Win::later(move || record::start(mon, region, delay));
    }

    /// 长截图：遮罩关掉（释放截图帧）以后，在选区这块活的屏幕上边滚边截
    fn long(&mut self, w: Win) {
        let Some(r) = self.v.sel else { return };
        let (dx, dy, mon) = (self.mon.rect.left, self.mon.rect.top, self.mon);
        let region = Rect { left: r.left + dx, top: r.top + dy, right: r.right + dx, bottom: r.bottom + dy };
        w.close();
        Win::later(move || longshot::start(mon, region));
    }

    /// 取字、翻译：遮罩立刻关闭、释放截图帧，卡片单独留在屏幕上；按原图识别，不带标注
    fn recognize(&mut self, w: Win, ocr: bool) {
        let Some(r) = self.v.sel else { return };
        let (dx, dy) = (self.mon.rect.left, self.mon.rect.top);
        let (img, mon) = (self.frame.crop(&r), self.mon);
        let anchor = Rect { left: r.left + dx, top: r.top + dy, right: r.right + dx, bottom: r.bottom + dy };
        w.close();
        Win::later(move || if ocr { card::ocr(img, anchor, mon) } else { card::translate(img, anchor, mon) });
    }

    /// 换模式：选区留着；切到录屏时放下标注工具（有标注时切不过去）
    fn set_mode(&mut self, w: Win, mode: Mode) {
        if mode == self.v.mode || (mode == Mode::Record && !self.v.rec_ok) {
            return;
        }
        let cur = w.cursor_pos();
        self.update(w, |s| {
            s.v.mode = mode;
            s.ed.tool = None;
            // 录屏多一层「屏幕去掉任务栏」，重新算吸附的几层
            if s.v.sel.is_none() {
                s.snaps.clear();
                s.hover_at(cur);
            }
        });
    }

    /// 右键：有选区先清选区（标注一起清掉），没有就退出
    fn cancel(&mut self, w: Win) {
        if self.v.sel.is_none() {
            return w.close();
        }
        self.drag = Drag::None;
        let cur = w.cursor_pos();
        self.update(w, |s| {
            s.ed = Editor::new(Doc::default(), s.mon.scale());
            s.v = View { mode: s.v.mode, ..View::default() };
            s.hover_at(cur);
        });
    }
}

impl Handler for Overlay {
    fn event(&mut self, w: Win, e: Event) {
        // 正在输入文字：键盘和输入法先给文字框
        if self.ed.text.is_some() {
            let mut rects = None;
            self.update(w, |s| rects = s.ed.text_event(&e, |p| p));
            if let Some(rects) = rects {
                rects.into_iter().for_each(|r| w.invalidate(r));
                return;
            }
        }
        match e {
            // 双击选区复制（录屏选区里是开始录制）只在没选工具、没点在标注上时生效；其余当成又一次按下（连续放序号）
            Event::Down { p, button: Button::Left, double: true, .. } if self.hit_at(p) == Hit::Inside && self.ed.tool.is_none() && self.ed.hit(pt(p)).is_none() => {
                if self.v.mode == Mode::Record { self.record(w) } else { self.copy(w) }
            }
            Event::Down { p, button: Button::Left, .. } => self.mouse_down(w, p),
            Event::Move { p, mods } => self.mouse_move(w, p, mods.shift),
            Event::Up { button: Button::Left, .. } => self.mouse_up(w),
            // 丢了鼠标（比如切走了）就当松手，但不触发按钮
            Event::CaptureLost => {
                if !matches!(self.drag, Drag::None) {
                    let edit = matches!(self.drag, Drag::Edit);
                    self.drag = Drag::None;
                    if edit {
                        self.update(w, |s| s.ed.up());
                    }
                    self.mouse_up(w);
                }
            }
            Event::Timer(TIP_TIMER) => {
                w.kill_timer(TIP_TIMER);
                self.update(w, |s| s.v.tip = s.v.hot);
            }
            // 在抬起时处理：按下就关的话，抬起会落到下面的窗口上，弹出它的右键菜单
            Event::Up { button: Button::Right, .. } => self.cancel(w),
            Event::Wheel { delta, .. } if self.v.sel.is_none() => self.wheel(w, delta),
            // 录屏选区没有标注：Esc 退出，Enter 开始录，Tab 回截图
            Event::Key { key, .. } if self.v.mode == Mode::Record => match key {
                Key::Esc => w.close(),
                Key::Enter => self.record(w),
                Key::Tab => self.set_mode(w, Mode::Shot),
                _ => {}
            },
            // Tab 切到录屏；有标注切不过去，提示出在「录屏」下面
            Event::Key { key: Key::Tab, .. } if !self.v.rec_ok => self.update(w, |s| s.v.tip = Some(Btn::Mode(true))),
            Event::Key { key: Key::Tab, .. } => self.set_mode(w, Mode::Record),
            Event::Key { key, mods } => {
                // 标注快捷键：工具、撤销重做、删除
                let mut used = false;
                if self.v.sel.is_some() {
                    self.update(w, |s| used = s.ed.key(key, mods));
                }
                match key {
                    _ if used => {}
                    Key::Esc => w.close(),
                    Key::Enter => self.copy(w),
                    Key::Char(b'C') if mods.cmd => self.copy(w),
                    Key::Char(b'S') if mods.cmd && self.v.sel.is_some() => self.save(w),
                    _ => {}
                }
            }
            // 遮罩固定盖住截图的那块显示器，缩放变了也不挪
            _ => {}
        }
    }

    /// 合成 c.area() 这块：底图、裁在选区里的标注、碰到这块的装饰
    fn paint(&mut self, _w: Win, c: &Canvas) {
        let a = c.area();
        self.compose(c, a);
        if let Some(sel) = self.v.sel
            && (!self.ed.doc.anns.is_empty() || self.ed.text.is_some())
            && let Some(r) = intersect(&a, &sel)
        {
            c.push_clip(rectf(r.left as f32, r.top as f32, r.right as f32, r.bottom as f32));
            self.ed.draw(c, &self.frame, Some(self.sel));
            c.pop_clip();
        }
        let s = self.mon.scale();
        for d in self.decos() {
            if let Some(r) = intersect(&d.rect(s), &a) {
                c.push_clip(rectf(r.left as f32, r.top as f32, r.right as f32, r.bottom as f32));
                self.draw_deco(c, &d);
                c.pop_clip();
            }
        }
    }

    fn cursor(&mut self, _w: Win, p: Point) -> Cursor {
        match self.hit_at(p) {
            Hit::Bar(_) | Hit::Opt(_) | Hit::Mode(_) => Cursor::Arrow,
            Hit::Handle(i) => match HANDLES[i] {
                (0, 0) | (2, 2) => Cursor::SizeNWSE,
                (2, 0) | (0, 2) => Cursor::SizeNESW,
                (1, _) => Cursor::SizeNS,
                _ => Cursor::SizeWE,
            },
            hit => {
                if self.v.sel.is_some()
                    && let Some(c) = self.ed.cursor(pt(p), hit == Hit::Inside)
                {
                    return c;
                }
                match hit {
                    Hit::Inside => Cursor::Move,
                    // 选区外面：还能换新选区时是十字
                    _ if self.ed.tool.is_none() && self.ed.doc.anns.is_empty() => Cursor::Cross,
                    _ => Cursor::Arrow,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::Shape;
    use crate::plat::Mods;

    fn open() -> Win {
        show(Mode::Shot);
        *Win::all(WinKind::Overlay).last().expect("遮罩没出来")
    }

    fn down(w: Win, x: i32, y: i32) {
        w.send(Event::Down { p: Point { x, y }, button: Button::Left, double: false, mods: Mods::default() });
    }

    fn mv(w: Win, x: i32, y: i32) {
        w.send(Event::Move { p: Point { x, y }, mods: Mods::default() });
    }

    fn up(w: Win) {
        w.send(Event::Up { button: Button::Left });
    }

    /// 按下、经过中间点、在终点松开
    fn drag(w: Win, from: (i32, i32), to: (i32, i32)) {
        down(w, from.0, from.1);
        mv(w, (from.0 + to.0) / 2, (from.1 + to.1) / 2);
        mv(w, to.0, to.1);
        up(w);
    }

    fn click(w: Win, r: Rect) {
        let (x, y) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
        mv(w, x, y);
        down(w, x, y);
        up(w);
    }

    fn key(w: Win, k: Key) {
        w.send(Event::Key { key: k, mods: Mods::default() });
    }

    fn chars(w: Win, t: &str) {
        t.chars().for_each(|c| w.send(Event::Char(c)));
    }

    /// 截到的画面换成 `f` 给的图案（不留屏幕内容），暗图跟着重做，整窗重画
    fn fill(w: Win, f: impl Fn(i32, i32) -> [u8; 4]) {
        let s = w.peek::<Overlay>();
        let fw = s.frame.w;
        for (i, p) in s.frame.pixels_mut().chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(&f(i as i32 % fw, i as i32 / fw));
        }
        s.dim = s.frame.dimmed().unwrap();
        w.invalidate_all();
    }

    /// 遮罩现在的画面（整块重新合成一遍）
    fn screen(w: Win) -> Bitmap {
        let s = w.peek::<Overlay>();
        let b = Bitmap::new(s.frame.w, s.frame.h).unwrap();
        s.paint(w, &Canvas::new(&b, b.full(), false));
        b
    }

    fn png(img: &plat::Image, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("overlay");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), plat::encode(img, false).unwrap()).unwrap();
    }

    /// 录屏选区：鼠标停在屏幕中间（默认那层、往外滚一格），再框一块。画面存成 target/test-tmp/overlay/record-*.png。
    /// 会截屏、盖住屏幕一会儿，平时不跑：cargo test record_mode_overlay -- --ignored --nocapture
    #[test]
    #[ignore]
    fn record_mode_overlay() {
        show(Mode::Record);
        let w = *Win::all(WinKind::Overlay).last().expect("遮罩没出来");
        let s = w.peek::<Overlay>();
        let (cx, cy) = (s.frame.w / 2, s.frame.h / 2);
        mv(w, cx, cy);
        println!("默认第 {} 层：{:?}", s.level, s.snaps);
        png(&screen(w).crop(&Rect { left: 0, top: 0, right: s.frame.w, bottom: s.frame.h }), "record-hover.png");
        let before = s.v.hover;
        w.send(Event::Wheel { delta: 120, p: Point { x: cx, y: cy }, mods: Mods::default() });
        assert!(s.picked && (s.level + 1 == s.snaps.len() || s.v.hover != before));
        // 挪一点，滚过的那层还在
        let kept = s.v.hover;
        mv(w, cx + 3, cy + 3);
        assert_eq!(s.v.hover, kept);
        drag(w, (cx - 300, cy - 200), (cx + 300, cy + 200));
        let sel = s.v.sel.expect("没框上");
        png(&screen(w).crop(&Rect { left: sel.left - 60, top: sel.top - 80, right: sel.right + 60, bottom: sel.bottom + 120 }), "record-selected.png");
        // 默认不延时；点「3 秒」那段换成 3 秒，再点「不延时」换回来
        assert_eq!(DELAY.get(), 0);
        click(w, s.bar().unwrap().button(Act::Delay(3)));
        assert_eq!(DELAY.get(), 3);
        let b = s.bar().unwrap().rect;
        png(&screen(w).crop(&Rect { left: b.left - 10, top: b.top - 10, right: b.right + 10, bottom: b.bottom + 50 }), "record-delay.png");
        click(w, s.bar().unwrap().button(Act::Delay(0)));
        assert_eq!(DELAY.get(), 0);
        // Tab 回截图：选区留着，操作栏换成截图的；画一个矩形后 Tab 切不过去，「录屏」下面出提示
        key(w, Key::Tab);
        assert!(s.v.mode == Mode::Shot && s.v.sel == Some(sel) && s.bar().unwrap().tip(Act::Copy).0 == "复制");
        key(w, Key::Char(b'R'));
        drag(w, (cx - 200, cy - 100), (cx - 50, cy));
        key(w, Key::Tab);
        assert!(s.v.mode == Mode::Shot && !s.v.rec_ok && s.v.tip == Some(Btn::Mode(true)));
        let m = s.mode_bar().unwrap().rect;
        png(&screen(w).crop(&Rect { left: m.left - 10, top: m.top - 10, right: m.right + 10, bottom: m.bottom + 50 }), "mode-bar-locked.png");
        // 撤销完就能切
        w.send(Event::Key { key: Key::Char(b'Z'), mods: Mods { cmd: true, ..Mods::default() } });
        key(w, Key::Tab);
        assert!(s.v.mode == Mode::Record && s.ed.tool.is_none());
        key(w, Key::Esc);
        assert!(!w.alive());
    }

    /// 模式条躲选区：框在它正下面（挡住上边的手柄）就挪到底下，框满整块屏幕就不出。平时不跑：
    /// cargo test mode_bar_dodges -- --ignored
    #[test]
    #[ignore]
    fn mode_bar_dodges() {
        let w = open();
        fill(w, |_, _| [0xF0; 4]);
        let s = w.peek::<Overlay>();
        let (fw, fh, top) = (s.frame.w, s.frame.h, s.mode_bar().unwrap().rect);
        drag(w, (fw / 2 - 200, top.bottom + 2), (fw / 2 + 200, top.bottom + 300));
        let m = s.mode_bar().unwrap().rect;
        assert!(m.top > fh / 2 && intersect(&m, &s.v.sel.unwrap()).is_none(), "{m:?}");
        w.send(Event::Up { button: Button::Right });
        drag(w, (0, 0), (fw, fh));
        assert!(s.mode_bar().is_none());
        w.close();
    }

    #[test]
    fn rect_minus() {
        let a = Rect { left: 0, top: 0, right: 10, bottom: 10 };
        // 不相交：整块；包含：空；右下角重叠：剩下 L 形的两块
        assert_eq!(minus(a, Rect { left: 20, top: 20, right: 30, bottom: 30 }), vec![a]);
        assert!(minus(a, Rect { left: -1, top: -1, right: 11, bottom: 11 }).is_empty());
        let area: i32 = minus(a, Rect { left: 5, top: 5, right: 15, bottom: 15 }).iter().map(|r| (r.right - r.left) * (r.bottom - r.top)).sum();
        assert_eq!(area, 75);
    }

    /// 遮罩上涂马赛克（截到的画面先换成图案，不留屏幕内容）：先斜着拖出一大片，再在这片范围里面横着涂，
    /// 每动一下真的重画一次窗口；打印每次移动连重画的耗时，最后检查新涂的那截是马赛克，
    /// 存 target/test-tmp/overlay/mosaic.png。平时不跑：cargo test overlay_mosaic -- --ignored --nocapture
    #[test]
    #[ignore]
    fn overlay_mosaic() {
        let w = open();
        fill(w, |x, y| [x as u8, y as u8, (x ^ y) as u8, 0]);
        let s = w.peek::<Overlay>();
        let (fw, fh) = (s.frame.w, s.frame.h);
        let p0 = (fw / 2 - 400, fh / 2 - 250);
        let at = |dx: i32, dy: i32| (p0.0 + dx, p0.1 + dy);
        drag(w, at(0, 0), at(700, 400));
        key(w, Key::Char(b'M'));
        let (x, y) = at(350, 200);
        assert_eq!(s.cursor(w, Point { x, y }), annotate::mosaic::cursor(&s.ed));
        w.repaint();
        let path = [(60, 60), (640, 340), (620, 300), (100, 300)];
        let (x, y) = at(path[0].0, path[0].1);
        down(w, x, y);
        let (t0, mut n) = (Instant::now(), 0);
        for seg in path.windows(2) {
            let ((ax, ay), (bx, by)) = (seg[0], seg[1]);
            for k in 1..=20 {
                let (x, y) = at(ax + (bx - ax) * k / 20, ay + (by - ay) * k / 20);
                mv(w, x, y);
                w.repaint();
                n += 1;
            }
        }
        let per = t0.elapsed() / n;
        up(w);
        // 最后一段整个在先前涂出的范围里，离前两段都远：每个像素都是所在格子中心的原图颜色
        let b = (annotate::BLOCK[1] * s.mon.scale()).round() as i32;
        let shot = screen(w);
        let rgb = |bmp: &Bitmap, x: i32, y: i32| bmp.pixels()[((y * fw + x) * 4) as usize..][..3].to_vec();
        for dx in [200, 300, 400] {
            let (x, y) = at(dx, 300);
            let (cx, cy) = ((x / b * b + b / 2).min(fw - 1), (y / b * b + b / 2).min(fh - 1));
            assert_eq!(rgb(&shot, x, y), rgb(&s.frame, cx, cy), "({dx}, 300) 不是马赛克");
        }
        png(&shot.crop(&Rect { left: p0.0 - 20, top: p0.1 - 20, right: p0.0 + 720, bottom: p0.1 + 420 }), "mosaic.png");
        println!("遮罩 {fw}×{fh}：涂马赛克每次移动连重画平均 {:.2} ms（{n} 次）", per.as_secs_f64() * 1000.0);
        w.close();
    }

    /// 打开真的遮罩（截到的画面先刷成浅灰，不留屏幕内容），用事件框选、选工具、画图形、选中改色、
    /// 打字、撤销重做，最后把画面和导出的图存成 target/test-tmp/overlay/*.png。会盖住屏幕几百毫秒，平时不跑：
    /// cargo test overlay_flow -- --ignored
    #[test]
    #[ignore]
    fn overlay_flow() {
        let w = open();
        fill(w, |_, _| [0xF0; 4]);
        let s = w.peek::<Overlay>();
        let p0 = (s.frame.w / 2 - 400, s.frame.h / 2 - 250);
        let at = |dx: i32, dy: i32| (p0.0 + dx, p0.1 + dy);

        // 框选，点操作栏的矩形工具，画一个；A 键换箭头，画一个
        drag(w, at(0, 0), at(700, 400));
        assert_eq!(s.v.sel, Some(Rect { left: p0.0, top: p0.1, right: p0.0 + 700, bottom: p0.1 + 400 }));
        click(w, s.bar().unwrap().button(Act::Rect));
        assert_eq!(s.ed.tool, Some(Tool::Rect));
        drag(w, at(40, 40), at(240, 160));
        key(w, Key::Char(b'A'));
        drag(w, at(300, 200), at(460, 80));
        // 选着箭头工具点矩形的边：选中它，选项条换成它的颜色，点紫色改掉
        let (x, y) = at(40, 100);
        click(w, Rect { left: x, top: y, right: x + 1, bottom: y + 1 });
        assert_eq!(s.ed.sel, Some(0));
        let o = s.opts(&s.bar().unwrap()).unwrap();
        click(w, o.button(Item::Color(3)));
        assert_eq!(s.ed.doc.anns[0].color, 3);
        // 画笔、序号两个、文字
        key(w, Key::Char(b'P'));
        drag(w, at(500, 300), at(650, 350));
        key(w, Key::Char(b'N'));
        for x in [80, 140] {
            let (x, y) = at(x, 320);
            down(w, x, y);
            up(w);
        }
        key(w, Key::Char(b'T'));
        let (x, y) = at(260, 300);
        down(w, x, y);
        up(w);
        chars(w, "标注 R");
        key(w, Key::Enter);
        chars(w, "第二行");
        // 打字时 R 不能切到矩形；Esc 只结束输入
        assert_eq!(s.ed.tool, Some(Tool::Text));
        key(w, Key::Esc);
        assert!(s.v.sel.is_some() && matches!(&s.ed.doc.anns.last().unwrap().shape, Shape::Text(_, t) if t == "标注 R\n第二行"));
        // 撤销文字再重做回来
        let n = s.ed.doc.anns.len();
        w.send(Event::Key { key: Key::Char(b'Z'), mods: Mods { cmd: true, ..Mods::default() } });
        assert_eq!(s.ed.doc.anns.len(), n - 1);
        w.send(Event::Key { key: Key::Char(b'Y'), mods: Mods { cmd: true, ..Mods::default() } });
        assert_eq!(s.ed.doc.anns.len(), 6);
        // 再选中矩形，画面上带选中框和选项条
        let (x, y) = at(240, 100);
        click(w, Rect { left: x, top: y, right: x + 1, bottom: y + 1 });

        let shot = Rect { left: p0.0 - 60, top: p0.1 - 60, right: p0.0 + 900, bottom: p0.1 + 560 };
        png(&screen(w).crop(&shot), "screen.png");
        // 导出：带标注、不带选中框
        png(&s.export().unwrap(), "export.png");
        w.close();
    }

    /// 文字输入：打字、光标移动后插入和删除、多行、粘贴（剪贴板里的文字测完放回去）、输入中改颜色粗细、
    /// 输入法组合串（直接发 Ime 事件）和候选窗位置（插入光标）。画面存成 target/test-tmp/overlay/text-*.png。平时不跑：
    /// cargo test overlay_text -- --ignored
    #[test]
    #[ignore]
    fn overlay_text() {
        let w = open();
        fill(w, |_, _| [0xF0; 4]);
        let s = w.peek::<Overlay>();
        let p0 = (s.frame.w / 2 - 400, s.frame.h / 2 - 250);
        let at = |dx: i32, dy: i32| (p0.0 + dx, p0.1 + dy);
        let shot = |name: &str| {
            let r = Rect { left: p0.0 + 20, top: p0.1 + 20, right: p0.0 + 620, bottom: p0.1 + 320 };
            png(&screen(w).crop(&r), name);
        };
        let text = || w.peek::<Overlay>().ed.text.as_ref().map(|e| e.text.clone());

        drag(w, at(0, 0), at(700, 400));
        key(w, Key::Char(b'T'));
        let (x, y) = at(60, 60);
        down(w, x, y);
        up(w);
        // 光标左移插入、Home + Delete、End + Backspace、Enter 换行
        chars(w, "截图ac");
        shot("text-1-typed.png");
        key(w, Key::Left);
        chars(w, "b");
        key(w, Key::Home);
        key(w, Key::Delete);
        key(w, Key::End);
        key(w, Key::Back);
        key(w, Key::Enter);
        chars(w, "第二行");
        assert_eq!(text().unwrap(), "图ab\n第二行");
        // Ctrl+V：剪贴板里是 \r\n，进来是 \n
        let saved = plat::paste_text();
        plat::copy_text("粘贴\n第三行").unwrap();
        w.send(Event::Key { key: Key::Char(b'V'), mods: Mods { cmd: true, ..Mods::default() } });
        if let Some(t) = saved {
            plat::copy_text(&t).unwrap();
        }
        assert_eq!(text().unwrap(), "图ab\n第二行粘贴\n第三行");
        shot("text-2-pasted.png");
        // 输入中点选项条改颜色、粗细：还在输入，马上换样子
        let o = s.opts(&s.bar().unwrap()).unwrap();
        click(w, o.button(Item::Color(3)));
        click(w, o.button(Item::Size(2)));
        assert!(s.ed.text.is_some() && s.ed.pen() == (3, 2));
        // 输入法：组合串插在「第三|行」的光标处，候选窗躲开光标那一行（插入光标的高是字体的上下伸，中点落在第三行里）
        key(w, Key::Left);
        w.send(Event::Ime { result: None, comp: "zu'he".into(), cursor: 5 });
        let (e, fs) = (s.ed.text.as_ref().unwrap().at, annotate::FONT[2] * s.mon.scale());
        let (r, lh) = (s.ed.caret(|p| p).unwrap(), fs * 1.3);
        let mid = (r.top + r.bottom) as f32 / 2.0;
        assert!(mid > e.y + 2.0 * lh && mid < e.y + 3.0 * lh && (r.bottom - r.top) as f32 > fs && r.left as f32 > e.x + 3.0 * fs, "{r:?}");
        shot("text-3-composing.png");
        // 上屏：字进内容、组合串清掉；Esc 提交，颜色粗细是改过的
        w.send(Event::Ime { result: Some("组合".into()), comp: String::new(), cursor: 0 });
        assert_eq!(text().unwrap(), "图ab\n第二行粘贴\n第三组合行");
        shot("text-4-committed.png");
        key(w, Key::Esc);
        let a = s.ed.doc.anns.last().unwrap();
        assert!(text().is_none() && matches!(&a.shape, Shape::Text(_, t) if t == "图ab\n第二行粘贴\n第三组合行") && (a.color, a.size) == (3, 2));
        shot("text-5-done.png");
        w.close();
    }
}
