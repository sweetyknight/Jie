//! 文字卡片：OCR 和翻译结果共用。置顶的弹出窗口，圆角交给系统（Win10 上是直角）；
//! 正文自绘，自己做拖选、Ctrl+A、Ctrl+C、滚轮滚动，选中底色才能是浅紫。
//! 尺寸按设计稿（DIP）乘卡片所在显示器的缩放比例。

use crate::ocr::{self, Outcome};
use crate::plat::{self, Button, Canvas, Cursor, Edge, Event, Face, Handler, Hit, Image, Key, Monitor, Point, Rect, Spec, Stroke, Text, Win};
use crate::settings;
use crate::toast::{self, Kind};
use crate::translate;
use crate::ui::{self, rectf, rgb, rgba};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const T_SPIN: usize = 1;
/// 标题栏「已复制」显示 1.5 秒
const T_COPIED: usize = 2;
const FRAME_MS: u32 = 16;

// 设计稿尺寸（DIP），第 3 版
const WIDTH: f32 = 380.0;
const HEAD: f32 = 44.0;
/// 底栏：按钮高 30 + 下边距 12，只在出错和没配服务时有
const FOOT: f32 = 42.0;
/// 正文最高 380，超出滚动
const BODY_MAX: f32 = 380.0;
/// 译文下面的原文开关行
const ORIG_ROW: f32 = 36.0;
/// 正文 15 号，行高 1.5
const LINE: f32 = 15.0 * 1.5;
/// 正文左右留白
const PAD: f32 = 20.0;
/// 离边这么近算拖边框改大小；拖出来的宽最小 300
const EDGE: f32 = 6.0;
const MIN_WIDTH: f32 = 300.0;

const TEXT: u32 = 0x1F1D23;
const TEXT2: u32 = 0x6B6475;
const TEXT3: u32 = 0xA9A3B2;
const ACCENT: u32 = 0x9C44EC;
/// 语言标签、「已复制」的深紫字
const ACCENT_TEXT: u32 = 0x7A3CC0;
const SELECTION: u32 = 0xE7D5FB;
const ERR: u32 = 0xC62828;
const ERR_BG: u32 = 0xFDF1F1;
const BORDER: u32 = 0xE3DEEA;
const LINE_COLOR: u32 = 0xECE8F1;
/// 图标按钮悬停、按下的底色
const HOVER: [u32; 3] = [0, 0xF1EDF5, 0xE8E2EF];

const NO_SERVICE: [&str; 2] = [
    "还没有配置翻译服务",
    "在设置里填上 API Key 就能用，默认用 DeepSeek。也可以换成通义千问、智谱、Kimi、OpenAI、本地 Ollama 这类 OpenAI 兼容接口。",
];

const NO_OCR_SERVICE: [&str; 2] = [
    "还没有配置文字识别服务",
    "在设置里填上 API Key 就能用：文字识别页留空就沿用翻译页的服务，也可以单独填一家能看图的模型。",
];

/// 翻译卡片的原文展开还是折叠，本次运行内记住；每次启动从折叠开始
static ORIG_OPEN: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    /// 标题栏带字的「复制」：取字结果或译文
    Copy,
    Close,
    /// 原文开关行：折叠或展开原文
    Toggle,
    CopyOrig,
    // 底栏
    Retry,
    Settings,
}

enum Body {
    Loading,
    /// 识别结果：可以选中、复制、滚动
    Text(String),
    /// 还没配置翻译服务（true 是大模型取字的服务）：去设置的入口
    NoService(bool),
    Translate(Tr),
}

/// 翻译卡片的正文：译文在上，原文在下（默认折叠）
struct Tr {
    orig: String,
    /// 原文的语言（认不出是 None）和行数，原文开关行「原文 · English · 3 行」用
    src: Option<translate::Lang>,
    lines: usize,
    open: bool,
    state: TrState,
}

enum TrState {
    Busy,
    /// 译文（用时写在标题栏附注里）
    Done(String),
    /// 失败原因
    Failed(String),
}

/// 翻译线程发回的结果：(译文, 用时) 或失败原因
type Translated = Result<(String, String), String>;

/// 正文里的一段是什么：取字结果或译文、原文都能选中，其余是说明文字
#[derive(Clone, Copy, PartialEq)]
enum Part {
    Main,
    Orig,
    Plain,
}

/// 排好版的一段；坐标相对正文区左上角，还没减滚动量
struct Block {
    x: f32,
    y: f32,
    layout: Text,
    color: u32,
    part: Part,
}

struct Card {
    scale: f32,
    /// 所在显示器的工作区，卡片夹在里面
    work: Rect,
    title: &'static str,
    /// 识别完接着翻译
    translate: bool,
    /// 翻译的语言标签「英 → 中」，空的不画
    pair: String,
    /// 标题栏附注「3 行 · 1.4 秒」
    meta: String,
    body: Body,
    blocks: Vec<Block>,
    /// 正文排版的总高度（像素，含内边距）
    content_h: f32,
    /// 原文开关行的顶边（像素，相对正文区，没减滚动量）
    row_y: f32,
    /// 标题栏的「复制」刚点过，显示「已复制」
    copied: bool,
    scroll: f32,
    /// 选中范围在哪一段；起点和光标是这一段的 UTF-16 下标
    sel_part: Part,
    anchor: u32,
    caret: u32,
    selecting: bool,
    hot: Option<Btn>,
    down: Option<Btn>,
    angle: f32,
    /// 拖边框定下的宽、高（DIP）；没拖过的方向是 None：宽 380，高按内容
    user_w: Option<f32>,
    user_h: Option<f32>,
}

/// 取字：先弹出「识别中」的卡片，在工作线程里识别，结果交回卡片。`anchor` 是选区的屏幕坐标。
pub fn ocr(img: Image, anchor: Rect, mon: Monitor) {
    let s = settings::get();
    if !s.ocr_service().ready() {
        create("取字", false, Body::NoService(true), anchor, mon);
    } else if let Some(w) = create("取字", false, Body::Loading, anchor, mon) {
        recognize(w, img);
    }
}

/// 翻译：先取字，原文显示出来以后再请求翻译（只发文字）。翻译服务或识别服务没配好（不是本机服务又没填 Key）时不识别，直接给出去设置的入口。
pub fn translate(img: Image, anchor: Rect, mon: Monitor) {
    let s = settings::get();
    if !s.translate_service().ready() {
        create("翻译", true, Body::NoService(false), anchor, mon);
    } else if !s.ocr_service().ready() {
        create("翻译", true, Body::NoService(true), anchor, mon);
    } else if let Some(w) = create("翻译", true, Body::Loading, anchor, mon) {
        recognize(w, img);
    }
}

fn recognize(w: Win, img: Image) {
    let cfg = settings::get().ocr_service();
    std::thread::spawn(move || {
        let out = ocr::recognize(&img, &cfg);
        drop(img);
        w.post(move |c: &mut Card, w| {
            if c.arrive(w, out) {
                w.close();
            }
        });
    });
}

/// 托盘菜单：当前卡片数量
pub fn count() -> usize {
    Win::all(plat::Kind::Card).len()
}

/// 托盘菜单「关闭所有文字卡片」
pub fn close_all() {
    Win::all(plat::Kind::Card).into_iter().for_each(Win::close);
}

/// 新卡片的位置：anchor 右侧 12、顶边对齐；右边放不下换左侧；两侧都不行放下方，再不行放上方；
/// 最后夹回工作区，离边至少 8。左上角和已有卡片差不到 8 时向右下错开 24，最多 20 次。
fn place(anchor: Rect, w: i32, h: i32, work: Rect, s: f32, others: &[Point]) -> Point {
    let px = |d: f32| (d * s).round() as i32;
    let (g, m) = (px(12.0), px(8.0));
    let clamp = |x: i32, y: i32| (x.min(work.right - m - w).max(work.left + m), y.min(work.bottom - m - h).max(work.top + m));
    let (x, y) = if anchor.right + g + w <= work.right - m {
        (anchor.right + g, anchor.top)
    } else if anchor.left - g - w >= work.left + m {
        (anchor.left - g - w, anchor.top)
    } else if anchor.bottom + g + h <= work.bottom - m {
        (anchor.left, anchor.bottom + g)
    } else {
        (anchor.left, anchor.top - g - h)
    };
    let (mut x, mut y) = clamp(x, y);
    for _ in 0..20 {
        if !others.iter().any(|o| (o.x - x).abs() < m && (o.y - y).abs() < m) {
            break;
        }
        (x, y) = (x + px(24.0), y + px(24.0));
    }
    let (x, y) = clamp(x, y);
    Point { x, y }
}

/// 卡片里的点 p 在哪条边、哪个角上（离边不到 e）：拖这里改大小
fn edge_at(p: Point, w: i32, h: i32, e: i32) -> Option<Edge> {
    let (l, r, t, b) = (p.x < e, p.x >= w - e, p.y < e, p.y >= h - e);
    Some(match (l, r, t, b) {
        (true, _, true, _) => Edge::TopLeft,
        (_, true, true, _) => Edge::TopRight,
        (true, _, _, true) => Edge::BottomLeft,
        (_, true, _, true) => Edge::BottomRight,
        (true, ..) => Edge::Left,
        (_, true, ..) => Edge::Right,
        (_, _, true, _) => Edge::Top,
        (.., true) => Edge::Bottom,
        _ => return None,
    })
}

/// 拖边框时的新窗口 r 夹到 min、max（宽, 高）之间：只挪拖着的那边，对边不动；max 比 min 还小时取 min
fn fit_edge(edge: Edge, mut r: Rect, min: (i32, i32), max: (i32, i32)) -> Rect {
    let fit = |len: i32, lo: i32, hi: i32| len.min(hi).max(lo);
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let (left, right, top, bottom) = edge.sides();
    if left {
        r.left = r.right - fit(w, min.0, max.0);
    } else if right {
        r.right = r.left + fit(w, min.0, max.0);
    }
    if top {
        r.top = r.bottom - fit(h, min.1, max.1);
    } else if bottom {
        r.bottom = r.top + fit(h, min.1, max.1);
    }
    r
}

fn create(title: &'static str, translate: bool, body: Body, anchor: Rect, mon: Monitor) -> Option<Win> {
    let loading = matches!(body, Body::Loading);
    let mut card = Card {
        scale: mon.scale(),
        work: mon.work,
        title,
        translate,
        pair: String::new(),
        meta: String::new(),
        body,
        blocks: Vec::new(),
        content_h: 0.0,
        row_y: 0.0,
        copied: false,
        scroll: 0.0,
        sel_part: Part::Main,
        anchor: 0,
        caret: 0,
        selecting: false,
        hot: None,
        down: None,
        angle: 0.0,
        user_w: None,
        user_h: None,
    };
    card.relayout();
    let (w, h) = (card.width(), card.height());
    let others: Vec<Point> = Win::all(plat::Kind::Card).iter().map(|c| c.rect()).map(|r| Point { x: r.left, y: r.top }).collect();
    let at = place(anchor, w, h, mon.work, mon.scale(), &others);
    let rect = Rect { left: at.x, top: at.y, right: at.x + w, bottom: at.y + h };
    let win = Win::open(Spec { kind: plat::Kind::Card, rect, title: "Jie 文字卡片", owner: None }, card)?;
    win.show();
    if loading {
        win.timer(T_SPIN, FRAME_MS);
    }
    Some(win)
}

fn contains(r: &Rect, p: Point) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

impl Card {
    fn px(&self, dip: f32) -> f32 {
        dip * self.scale
    }

    fn pxi(&self, dip: f32) -> i32 {
        self.px(dip).round() as i32
    }

    /// 卡片宽（DIP），正文按它折行
    fn w_dip(&self) -> f32 {
        self.user_w.unwrap_or(WIDTH)
    }

    fn width(&self) -> i32 {
        self.pxi(self.w_dip())
    }

    fn head_h(&self) -> i32 {
        self.pxi(HEAD)
    }

    fn body_h(&self) -> i32 {
        // 拖过高度：正文占标题栏和底栏剩下的，放不下就滚动
        if let Some(h) = self.user_h {
            return (self.pxi(h) - self.head_h() - self.foot_h()).max(0);
        }
        let h = match self.body {
            // 上 8 + 转圈 16 + 下 22
            Body::Loading => self.px(46.0),
            Body::Text(_) | Body::Translate(_) => self.content_h.min(self.px(BODY_MAX)),
            Body::NoService(_) => self.content_h,
        };
        h.ceil() as i32
    }

    fn foot_h(&self) -> i32 {
        if self.foot_buttons().is_empty() { 0 } else { self.pxi(FOOT) }
    }

    fn height(&self) -> i32 {
        self.head_h() + self.body_h() + self.foot_h()
    }

    fn body_rect(&self) -> Rect {
        let top = self.head_h();
        Rect { left: 0, top, right: self.width(), bottom: top + self.body_h() }
    }

    fn max_scroll(&self) -> f32 {
        (self.content_h - self.body_h() as f32).max(0.0)
    }

    /// 底栏按钮，从左到右：(按钮, 文字, 是不是主按钮)；正常结果没有底栏
    fn foot_buttons(&self) -> &'static [(Btn, &'static str, bool)] {
        match &self.body {
            Body::NoService(_) => &[(Btn::Settings, "打开设置", true)],
            Body::Translate(Tr { state: TrState::Failed(_), .. }) => &[(Btn::Settings, "设置", false), (Btn::Retry, "重试", true)],
            _ => &[],
        }
    }

    /// 能选中的一段的文字：Main 是取字结果或译文，Orig 是原文
    fn part_text(&self, part: Part) -> Option<&str> {
        match (&self.body, part) {
            (Body::Text(t), Part::Main) | (Body::Translate(Tr { state: TrState::Done(t), .. }), Part::Main) => Some(t),
            (Body::Translate(tr), Part::Orig) => Some(&tr.orig),
            _ => None,
        }
    }

    /// 这一段排好的版；原文折叠时没有
    fn block(&self, part: Part) -> Option<&Block> {
        self.blocks.iter().find(|b| b.part == part)
    }

    /// 按当前内容和缩放比例重新排版
    fn relayout(&mut self) {
        (self.blocks, self.content_h, self.row_y) = self.layout_body().unwrap_or_default();
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    /// 正文的各段文字、总高度、原文开关行的顶边。正文是界面字 15 号（Windows 上中日文由 DirectWrite 按 zh-CN 回落到微软雅黑）。
    /// shortcut: 设计稿的段间距 4 没做，DirectWrite 没有段间距，要做得把每段拆成单独的排版、跨段选中再自己拼
    fn layout_body(&self) -> Option<(Vec<Block>, f32, f32)> {
        let s = self.scale;
        let wrap = self.px(self.w_dip() - 2.0 * PAD);
        let mut v = Vec::new();
        // 放一段，返回它的高度
        let mut put = |x: f32, y: f32, layout: Text, color: u32, part: Part| {
            let h = layout.height();
            v.push(Block { x, y, layout, color, part });
            h
        };
        let small = |t: &str, width: f32| Text::wrapped(Face::Ui, 13.0 * s, false, t, width, Some(13.0 * 1.5 * s));
        let mut row_y = 0.0;
        let h = match &self.body {
            Body::Loading => 0.0,
            // 上 2、下 18
            Body::Text(t) => 2.0 * s + put(PAD * s, 2.0 * s, Text::wrapped(Face::Ui, 15.0 * s, false, t, wrap, Some(LINE * s))?, TEXT, Part::Main) + 18.0 * s,
            // 标题 14/600，空 4，说明 13 号、行高 1.5，下 16
            Body::NoService(ocr) => {
                let text = if *ocr { NO_OCR_SERVICE } else { NO_SERVICE };
                let y = put(PAD * s, 0.0, Text::wrapped(Face::Ui, 14.0 * s, true, text[0], wrap, None)?, TEXT, Part::Plain) + 4.0 * s;
                y + put(PAD * s, y, small(text[1], wrap)?, TEXT2, Part::Plain) + 16.0 * s
            }
            Body::Translate(tr) => {
                let mut y = match &tr.state {
                    // 上 8 + 转圈 16 + 下 22
                    TrState::Busy => 46.0 * s,
                    TrState::Done(t) => 2.0 * s + put(PAD * s, 2.0 * s, Text::wrapped(Face::Ui, 15.0 * s, false, t, wrap, Some(LINE * s))?, TEXT, Part::Main) + 14.0 * s,
                    // 浅红提示条：外边距上 2、左右 16、下 14，内边距 10 / 12，警告图标 16 + 间距 8；原因 13 号、行高 1.5
                    TrState::Failed(e) => {
                        let y = 12.0 * s;
                        y + put(52.0 * s, y, small(e, self.px(self.w_dip() - 80.0))?, ERR, Part::Plain) + 24.0 * s
                    }
                };
                // 原文开关行；展开时原文在它下面：13 号灰字、行高 1.5，左边竖线 2 + 空 12，下 16
                row_y = y;
                y += ORIG_ROW * s;
                if tr.open {
                    y += put(34.0 * s, y, small(&tr.orig, self.px(self.w_dip() - 54.0))?, TEXT2, Part::Orig) + 16.0 * s;
                }
                y
            }
        };
        Some((v, h, row_y))
    }

    /// 标题栏「复制」按钮的字
    fn copy_label(&self) -> &'static str {
        if self.copied { "已复制" } else { "复制" }
    }

    fn buttons(&self) -> Vec<(Btn, Rect)> {
        let (w, b) = (self.width(), self.pxi(28.0));
        let top = (self.head_h() - b) / 2;
        let close = Rect { left: w - self.pxi(8.0) - b, top, right: w - self.pxi(8.0), bottom: top + b };
        // 「复制」：左 8 + 图标 16 + 空 5 + 字 + 右 10，离关闭 4
        let tw = |label: &str| Text::new(Face::Ui, 13.0 * self.scale, false, label).map_or(0.0, |l| l.width());
        let right = close.left - self.pxi(4.0);
        let copy = Rect { left: right - (self.px(39.0) + tw(self.copy_label())).round() as i32, right, ..close };
        let mut v = vec![(Btn::Copy, copy), (Btn::Close, close)];
        if let Body::Translate(tr) = &self.body {
            // 原文开关行跟着正文滚动：左 12 起整条是开关（高 28），展开时右边多一个 24 见方的复制原文（离右边 8）
            let oy = self.body_rect().top + (self.row_y - self.scroll).round() as i32;
            let (rh, c) = (self.pxi(ORIG_ROW), self.pxi(24.0));
            let right = w - self.pxi(8.0);
            let toggle_right = if tr.open { right - c - self.pxi(4.0) } else { right };
            let tt = oy + (rh - b) / 2;
            v.push((Btn::Toggle, Rect { left: self.pxi(12.0), top: tt, right: toggle_right, bottom: tt + b }));
            if tr.open {
                let ct = oy + (rh - c) / 2;
                v.push((Btn::CopyOrig, Rect { left: right - c, top: ct, right, bottom: ct + c }));
            }
        }
        // 底栏按钮高 30，靠右排，离右边 12、间距 8，字左右各留 14
        let foot = self.foot_buttons();
        if !foot.is_empty() {
            let (top, bh) = (self.body_rect().bottom, self.pxi(30.0));
            let mut right = w - self.pxi(12.0);
            for (btn, label, _) in foot.iter().rev() {
                let left = right - (tw(label) + self.px(28.0)).round() as i32;
                v.push((*btn, Rect { left, top, right, bottom: top + bh }));
                right = left - self.pxi(8.0);
            }
        }
        v
    }

    fn enabled(&self, b: Btn) -> bool {
        b != Btn::Copy || self.part_text(Part::Main).is_some()
    }

    fn btn_at(&self, p: Point) -> Option<Btn> {
        let body = self.body_rect();
        // 原文开关行滚出正文区的部分点不到
        let visible = |b: Btn| !matches!(b, Btn::Toggle | Btn::CopyOrig) || contains(&body, p);
        self.buttons().into_iter().find(|(b, r)| contains(r, p) && self.enabled(*b) && visible(*b)).map(|(b, _)| b)
    }

    /// 点在哪一段能选中的文字上：开关行上面是译文，下面是原文
    fn in_text(&self, p: Point) -> Option<Part> {
        let body = self.body_rect();
        if !contains(&body, p) {
            return None;
        }
        let y = (p.y - body.top) as f32 + self.scroll;
        let has = |part: Part| self.block(part).map(|b| b.part);
        match self.body {
            Body::Text(_) => Some(Part::Main),
            Body::Translate(_) if y < self.row_y => has(Part::Main),
            Body::Translate(_) if y >= self.row_y + self.px(ORIG_ROW) => has(Part::Orig),
            _ => None,
        }
    }

    /// 点对应的选中那段里的位置；点在那段外面时取最近的位置
    fn pos_at(&self, p: Point) -> u32 {
        let Some(b) = self.block(self.sel_part) else { return 0 };
        let oy = self.body_rect().top as f32 - self.scroll + b.y;
        b.layout.hit(p.x as f32 - b.x, p.y as f32 - oy)
    }

    fn selection(&self) -> (u32, u32) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    /// 选中的文字；原文折叠着就没有
    fn selected(&self) -> String {
        let (Some(t), Some(_)) = (self.part_text(self.sel_part), self.block(self.sel_part)) else { return String::new() };
        let (a, b) = self.selection();
        let u: Vec<u16> = t.encode_utf16().collect();
        String::from_utf16_lossy(&u[(a as usize).min(u.len())..(b as usize).min(u.len())])
    }

    fn scroll_by(&mut self, w: Win, d: f32) {
        let s = (self.scroll + d).clamp(0.0, self.max_scroll());
        if s != self.scroll {
            self.scroll = s;
            w.invalidate(self.body_rect());
        }
    }

    /// 内容变了：重新排版，窗口改高度，位置不动（夹回工作区）
    fn refit(&mut self, win: Win) {
        self.relayout();
        let r = win.rect();
        let (w, h, m) = (self.width(), self.height(), self.pxi(8.0));
        let x = r.left.min(self.work.right - m - w).max(self.work.left + m);
        let y = r.top.min(self.work.bottom - m - h).max(self.work.top + m);
        win.set_rect(Rect { left: x, top: y, right: x + w, bottom: y + h });
        win.invalidate_all();
    }

    /// 拖边框：系统给的新窗口夹到最小、最大之间；拖了哪个方向就定下哪个方向的大小，
    /// 之后内容变了也不再自动改；正文按新宽度重新折行
    fn drag_size(&mut self, edge: Edge, r: Rect) -> Rect {
        let (w, m) = (self.work, self.pxi(8.0));
        let min = (self.pxi(MIN_WIDTH), self.pxi(HEAD + FOOT + 3.0 * LINE));
        let max = (w.right - w.left - 2 * m, w.bottom - w.top - 2 * m);
        let mut r = fit_edge(edge, r, min, max);
        let (left, right, top, bottom) = edge.sides();
        if left || right {
            self.user_w = Some((r.right - r.left) as f32 / self.scale);
        }
        if top || bottom {
            self.user_h = Some((r.bottom - r.top) as f32 / self.scale);
        } else {
            // 只拖了宽：高度还按内容，跟着重新折行变
            self.relayout();
            r.bottom = r.top + self.height();
        }
        self.relayout();
        r
    }

    /// 复制成功返回 true；`toast_ok` 为 false 时成功不出胶囊（标题栏按钮自己显示「已复制」），失败总出
    fn copy(&self, w: Win, text: &str, toast_ok: bool) -> bool {
        if text.is_empty() {
            return false;
        }
        let mon = w.monitor();
        match plat::copy_text(text) {
            Ok(()) => {
                if toast_ok {
                    toast::show(&mon, Kind::Ok, "已复制");
                }
                true
            }
            Err(e) => {
                toast::show(&mon, Kind::Err, &format!("复制失败：{e}"));
                false
            }
        }
    }

    /// 识别结果到了；返回 true 表示卡片该关了（没认出字、识别出错）
    fn arrive(&mut self, w: Win, out: Outcome) -> bool {
        w.kill_timer(T_SPIN);
        match out {
            // 翻译：译文的位置转圈，原文开关已经能点
            Outcome::Text { text, lines, .. } if self.translate => {
                let src = translate::detect(&text);
                let open = ORIG_OPEN.load(Ordering::Relaxed);
                self.body = Body::Translate(Tr { orig: text, src, lines, open, state: TrState::Busy });
                self.request(w);
            }
            Outcome::Text { text, took, lines } => {
                self.meta = format!("{lines} 行 · {took}");
                self.body = Body::Text(text);
            }
            Outcome::Empty => {
                toast::show(&w.monitor(), Kind::Err, "没有识别到文字");
                return true;
            }
            Outcome::Failed(e) => {
                toast::show(&w.monitor(), Kind::Err, &format!("取字失败：{e}"));
                return true;
            }
        }
        self.refit(w);
        false
    }

    /// 在工作线程里发翻译请求，只发原文文字。每次都读当前设置：改完设置点「重试」就用新的。调用方负责 refit。
    fn request(&mut self, w: Win) {
        let Body::Translate(tr) = &mut self.body else { return };
        let s = settings::get();
        let cfg = s.translate_service();
        let to = translate::pick_target(&tr.orig, cfg.target).short();
        self.pair = tr.src.map_or(format!("→ {to}"), |l| format!("{} → {to}", l.short()));
        self.meta.clear();
        tr.state = TrState::Busy;
        if self.sel_part == Part::Main {
            (self.anchor, self.caret) = (0, 0);
        }
        let text = tr.orig.clone();
        std::thread::spawn(move || {
            let t0 = Instant::now();
            let out: Translated = translate::translate(&cfg, &text, translate::TIMEOUT)
                .map(|t| (t, format!("{:.1} 秒", t0.elapsed().as_secs_f32())))
                .map_err(|e| e.to_string());
            w.post(move |c: &mut Card, w| c.translated(w, out));
        });
        w.timer(T_SPIN, FRAME_MS);
    }

    fn translated(&mut self, w: Win, out: Translated) {
        let Body::Translate(tr) = &mut self.body else { return };
        tr.state = match out {
            Ok((t, took)) => {
                self.meta = took;
                TrState::Done(t)
            }
            Err(e) => TrState::Failed(e),
        };
        w.kill_timer(T_SPIN);
        self.refit(w);
    }

    fn toggle(&mut self, w: Win) {
        let Body::Translate(tr) = &mut self.body else { return };
        tr.open = !tr.open;
        ORIG_OPEN.store(tr.open, Ordering::Relaxed);
        self.refit(w);
    }

    fn draw(&self, c: &Canvas, w: f32, h: f32) {
        let s = self.scale;
        c.clear(rgb(0xFFFFFF));
        let buttons = self.buttons();
        let _ = (|| -> Option<()> {
            // 标题栏：标题 14/600；翻译加语言标签（高 20、圆角 10、字左右 7）；附注 12 浅灰；右边「复制」和关闭
            let head = self.head_h() as f32;
            let title = Text::new(Face::Ui, 14.0 * s, true, self.title)?;
            c.text(&title, 18.0 * s, (head - title.height()) / 2.0, rgb(TEXT));
            let mut x = 18.0 * s + title.width() + 8.0 * s;
            if !self.pair.is_empty() {
                let l = Text::new(Face::Ui, 12.0 * s, false, &self.pair)?;
                let cw = l.width() + 14.0 * s;
                c.fill_round(rectf(x, head / 2.0 - 10.0 * s, x + cw, head / 2.0 + 10.0 * s), 10.0 * s, rgb(0xF4EEFB));
                c.text(&l, x + 7.0 * s, (head - l.height()) / 2.0, rgb(ACCENT_TEXT));
                x += cw + 6.0 * s;
            }
            if !self.meta.is_empty() {
                let meta = Text::tabular(Face::Ui, 12.0 * s, false, &self.meta)?;
                meta.truncate(buttons[0].1.left as f32 - 8.0 * s - x);
                c.text(&meta, x, (head - meta.height()) / 2.0, rgb(TEXT3));
            }
            // 正文裁在自己的区域里，跟着滚动的原文开关行也是
            let body = self.body_rect();
            c.push_clip(rectf(0.0, body.top as f32, w, body.bottom as f32));
            let r = self.draw_body(c, w, &buttons);
            c.pop_clip();
            r?;
            for (b, r) in &buttons {
                if !matches!(b, Btn::Toggle | Btn::CopyOrig) {
                    self.draw_button(c, *b, r)?;
                }
            }
            Some(())
        })();
        // 1px 外框；Win11 上系统会在同一个位置再画一圈同色的圆角边
        let lw = s.round().max(1.0);
        c.stroke_rect(rectf(lw / 2.0, lw / 2.0, w - lw / 2.0, h - lw / 2.0), rgb(BORDER), lw, Stroke::Plain);
    }

    /// 转圈 + 一行灰字，转圈左上角在 (20, y + 8)
    fn draw_busy(&self, c: &Canvas, y: f32, text: &str) -> Option<()> {
        let s = self.scale;
        ui::spinner(c, PAD * s, y + 8.0 * s, s, self.angle, rgb(0xEBDDFB), rgb(ACCENT));
        let l = Text::new(Face::Ui, 13.0 * s, false, text)?;
        c.text(&l, 46.0 * s, y + 16.0 * s - l.height() / 2.0, rgb(TEXT2));
        Some(())
    }

    fn draw_body(&self, c: &Canvas, w: f32, buttons: &[(Btn, Rect)]) -> Option<()> {
        let s = self.scale;
        let body = self.body_rect();
        let (top, bottom) = (body.top as f32, body.bottom as f32);
        let oy = top - self.scroll;
        match &self.body {
            Body::Loading => self.draw_busy(c, top, "正在识别文字…")?,
            Body::Translate(tr) => {
                match tr.state {
                    TrState::Busy => self.draw_busy(c, oy, "正在翻译…")?,
                    // 浅红提示条包住原因，图标和第一行文字上下居中
                    TrState::Failed(_) => {
                        if let Some(b) = self.blocks.iter().find(|b| b.part == Part::Plain) {
                            let (y0, y1) = (oy + b.y - 10.0 * s, oy + b.y + b.layout.height() + 10.0 * s);
                            c.fill_round(rectf(16.0 * s, y0, w - 16.0 * s, y1), 6.0 * s, rgb(ERR_BG));
                            ui::icon(c, "warn", 28.0 * s, oy + b.y + (13.0 * 1.5 - 16.0) / 2.0 * s, 16.0 * s, rgb(ERR));
                        }
                    }
                    TrState::Done(_) => {}
                }
                // 开关行：上面一条分隔线；「> 原文 · English · 3 行」，悬停时浅底、字变深
                let ry = oy + self.row_y;
                c.fill_rect(rectf(0.0, ry, w, ry + s.round().max(1.0)), rgb(LINE_COLOR));
                if let Some((_, r)) = buttons.iter().find(|(b, _)| *b == Btn::Toggle) {
                    let hot = self.hot == Some(Btn::Toggle);
                    let (l, t, rr, bb) = (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
                    if hot {
                        c.fill_round(rectf(l, t, rr, bb), 6.0 * s, rgb(HOVER[if self.down == Some(Btn::Toggle) { 2 } else { 1 }]));
                    }
                    let col = rgb(if hot { TEXT } else { TEXT2 });
                    ui::icon(c, if tr.open { "chev_down" } else { "chev" }, l + 6.0 * s, (t + bb) / 2.0 - 8.0 * s, 16.0 * s, col);
                    let label = match tr.src {
                        Some(l) => format!("原文 · {} · {} 行", l.label(), tr.lines),
                        None => format!("原文 · {} 行", tr.lines),
                    };
                    let lt = Text::new(Face::Ui, 12.0 * s, false, &label)?;
                    lt.truncate(rr - 6.0 * s - (l + 26.0 * s));
                    c.text(&lt, l + 26.0 * s, (t + bb - lt.height()) / 2.0, col);
                }
                for (b, r) in buttons.iter().filter(|(b, _)| *b == Btn::CopyOrig) {
                    self.draw_button(c, *b, r)?;
                }
                // 展开的原文左边一条 2 宽的浅紫竖线
                if let Some(b) = self.block(Part::Orig) {
                    let y = oy + b.y;
                    c.fill_rect(rectf(PAD * s, y, PAD * s + 2.0 * s, y + b.layout.height()), rgb(0xE6D6F8));
                }
            }
            Body::Text(_) | Body::NoService(_) => {}
        }
        // 各段文字；选中的那段先铺浅紫底
        let (a, z) = self.selection();
        for b in &self.blocks {
            if b.part == self.sel_part && z > a {
                for (l, t, mw, mh) in b.layout.range_rects(a, z) {
                    let (x, y) = (b.x + l, oy + b.y + t);
                    c.fill_rect(rectf(x, y, x + mw, y + mh), rgb(SELECTION));
                }
            }
            c.text(&b.layout, b.x, oy + b.y, rgb(b.color));
        }
        // 能滚动时右边画一条细滚动条
        let max = self.max_scroll();
        if max > 0.0 {
            let view = bottom - top;
            let th = (view * view / (view + max)).max(24.0 * s);
            let ty = top + (view - th) * self.scroll / max;
            c.fill_round(rectf(w - 8.0 * s, ty + 2.0 * s, w - 4.0 * s, ty + th - 2.0 * s), 2.0 * s, rgba(0x000000, 0.22));
        }
        Some(())
    }

    /// 标题栏的「复制」（图标 + 字）和图标按钮；底栏的文字按钮（主按钮紫底白字，其余白底灰边），圆角都是 6
    fn draw_button(&self, c: &Canvas, b: Btn, r: &Rect) -> Option<()> {
        let s = self.scale;
        let (l, t, rr, bb) = (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
        let on = self.enabled(b);
        let state = match (on && self.hot == Some(b), self.down == Some(b)) {
            (true, true) => 2,
            (true, false) => 1,
            _ => 0,
        };
        let text = |t: &str| Text::new(Face::Ui, 13.0 * s, false, t);
        if let Some(&(_, label, primary)) = self.foot_buttons().iter().find(|f| f.0 == b) {
            let (bg, fg) = if primary { ([ACCENT, 0xA95CF0, 0x8733D4][state], 0xFFFFFF) } else { ([0xFFFFFF, 0xF7F4FA, 0xEFEAF4][state], TEXT) };
            c.fill_round(rectf(l, t, rr, bb), 6.0 * s, rgb(bg));
            if !primary {
                // 整像素的灰边画在像素中心才不糊
                let (lw, hw) = (s.round().max(1.0), s.round().max(1.0) / 2.0);
                c.stroke_round(rectf(l + hw, t + hw, rr - hw, bb - hw), 6.0 * s, rgb(0xDCD6E3), lw);
            }
            let label = text(label)?;
            c.text(&label, (l + rr - label.width()) / 2.0, (t + bb - label.height()) / 2.0, rgb(fg));
            return Some(());
        }
        if state > 0 {
            c.fill_round(rectf(l, t, rr, bb), 6.0 * s, rgb(HOVER[state]));
        }
        let fg = rgb(match (on, self.copied && b == Btn::Copy, state > 0) {
            (false, ..) => TEXT3,
            (true, true, _) => ACCENT_TEXT,
            (true, false, true) => TEXT,
            _ => TEXT2,
        });
        let iy = (t + bb) / 2.0 - 8.0 * s;
        match b {
            Btn::Copy => {
                ui::icon(c, if self.copied { "check" } else { "copy" }, l + 8.0 * s, iy, 16.0 * s, fg);
                let label = text(self.copy_label())?;
                c.text(&label, l + 29.0 * s, (t + bb - label.height()) / 2.0, fg);
            }
            _ => ui::icon(c, if b == Btn::Close { "close" } else { "copy" }, (l + rr) / 2.0 - 8.0 * s, iy, 16.0 * s, fg),
        }
        Some(())
    }
}

impl Handler for Card {
    fn event(&mut self, w: Win, e: Event) {
        match e {
            Event::Down { p, button: Button::Left, mods, .. } => {
                if let Some(b) = self.btn_at(p) {
                    self.down = Some(b);
                } else if let Some(part) = self.in_text(p) {
                    // Shift 点击只在同一段里延长选区
                    let extend = part == self.sel_part && mods.shift;
                    self.sel_part = part;
                    let pos = self.pos_at(p);
                    if !extend {
                        self.anchor = pos;
                    }
                    self.caret = pos;
                    self.selecting = true;
                }
                w.invalidate_all();
            }
            Event::Move { p, .. } => {
                let hot = self.btn_at(p);
                if hot != self.hot {
                    self.hot = hot;
                    w.invalidate_all();
                }
                if self.selecting {
                    // 拖到正文上下边外面时跟着滚
                    let body = self.body_rect();
                    if p.y < body.top {
                        self.scroll_by(w, (p.y - body.top) as f32);
                    } else if p.y >= body.bottom {
                        self.scroll_by(w, (p.y - body.bottom + 1) as f32);
                    }
                    let caret = self.pos_at(p);
                    if caret != self.caret {
                        self.caret = caret;
                        w.invalidate(body);
                    }
                }
            }
            Event::Leave => {
                if self.hot.take().is_some() {
                    w.invalidate_all();
                }
            }
            Event::Up { button: Button::Left, .. } => {
                let clicked = self.down.take().filter(|b| self.hot == Some(*b));
                self.selecting = false;
                w.invalidate_all();
                match clicked {
                    Some(Btn::Close) => w.close(),
                    Some(Btn::Copy) => {
                        if self.copy(w, self.part_text(Part::Main).unwrap_or_default(), false) {
                            self.copied = true;
                            w.timer(T_COPIED, 1500);
                        }
                    }
                    Some(Btn::CopyOrig) => _ = self.copy(w, self.part_text(Part::Orig).unwrap_or_default(), true),
                    Some(Btn::Toggle) => self.toggle(w),
                    Some(Btn::Retry) => {
                        self.request(w);
                        self.refit(w);
                    }
                    Some(Btn::Settings) => {
                        let ocr = matches!(self.body, Body::NoService(true));
                        Win::later(move || crate::settings_ui::show_service(ocr));
                    }
                    None => {}
                }
            }
            Event::CaptureLost => {
                self.selecting = false;
                self.down = None;
            }
            Event::Wheel { delta, .. } => self.scroll_by(w, -delta as f32 / 120.0 * 3.0 * LINE * self.scale),
            Event::Key { key, mods } => match key {
                Key::Esc => w.close(),
                // Ctrl+A：全选刚才选过的那段，没有就选译文（取字结果）、再没有就选原文
                Key::Char(b'A') if mods.cmd => {
                    if let Some(part) = [self.sel_part, Part::Main, Part::Orig].into_iter().find(|p| self.block(*p).is_some()) {
                        self.sel_part = part;
                        (self.anchor, self.caret) = (0, self.part_text(part).map_or(0, |t| t.encode_utf16().count() as u32));
                        w.invalidate_all();
                    }
                }
                Key::Char(b'C') if mods.cmd => _ = self.copy(w, &self.selected(), true),
                _ => {}
            },
            Event::Timer(T_SPIN) => {
                // 0.8 秒一圈
                self.angle = (self.angle + 360.0 * FRAME_MS as f32 / 800.0) % 360.0;
                w.invalidate(self.body_rect());
            }
            Event::Timer(T_COPIED) => {
                w.kill_timer(T_COPIED);
                self.copied = false;
                w.invalidate_all();
            }
            // 拖到缩放比例不同的显示器上：按新比例重排，用系统建议的位置
            Event::Dpi { scale, rect: r } => {
                self.scale = scale;
                self.work = plat::monitor_at(Point { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 }).work;
                self.relayout();
                w.set_rect(Rect { left: r.left, top: r.top, right: r.left + self.width(), bottom: r.top + self.height() });
                w.invalidate_all();
            }
            _ => {}
        }
    }

    fn paint(&mut self, _w: Win, c: &Canvas) {
        self.draw(c, self.width() as f32, self.height() as f32);
    }

    fn cursor(&mut self, _w: Win, p: Point) -> Cursor {
        if self.selecting || (self.btn_at(p).is_none() && self.in_text(p).is_some()) { Cursor::IBeam } else { Cursor::Arrow }
    }

    /// 离边 6 以内拖边框改大小；标题栏空白处按住拖动
    fn hit(&mut self, _w: Win, p: Point) -> Hit {
        if let Some(e) = edge_at(p, self.width(), self.height(), self.pxi(EDGE)) {
            return Hit::Edge(e);
        }
        if p.y >= 0 && p.y < self.head_h() && self.btn_at(p).is_none() { Hit::Caption } else { Hit::Client }
    }

    fn sizing(&mut self, _w: Win, edge: Edge, r: Rect) -> Rect {
        self.drag_size(edge, r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plat::Mods;

    const WORK: Rect = Rect { left: 0, top: 0, right: 1920, bottom: 1032 };

    fn r(l: i32, t: i32, rr: i32, b: i32) -> Rect {
        Rect { left: l, top: t, right: rr, bottom: b }
    }

    fn at(p: Point) -> (i32, i32) {
        (p.x, p.y)
    }

    #[test]
    fn sides_then_below_then_above() {
        // 右侧放得下：右侧 12、顶边对齐
        assert_eq!(at(place(r(100, 200, 500, 400), 380, 300, WORK, 1.0, &[])), (512, 200));
        // 贴右边：换到左侧
        assert_eq!(at(place(r(1600, 200, 1900, 400), 380, 300, WORK, 1.0, &[])), (1208, 200));
        // 两侧都放不下：下方
        assert_eq!(at(place(r(10, 100, 1910, 400), 380, 300, WORK, 1.0, &[])), (10, 412));
        // 下方也不行：上方
        assert_eq!(at(place(r(10, 500, 1910, 1000), 380, 300, WORK, 1.0, &[])), (10, 188));
        // 都不行（全屏选区）：夹回工作区，离边 8
        assert_eq!(at(place(r(0, 0, 1920, 1080), 380, 300, WORK, 1.0, &[])), (8, 8));
        // 底部的选区：右侧，顶边对齐会出屏，往上推
        assert_eq!(at(place(r(100, 900, 500, 1000), 380, 300, WORK, 1.0, &[])), (512, 724));
        // 缩放 150%：间距 18、边距 12
        assert_eq!(at(place(r(100, 200, 500, 400), 570, 450, WORK, 1.5, &[])), (518, 200));
    }

    #[test]
    fn staggers_over_existing_cards() {
        let a = r(100, 200, 500, 400);
        let others = [Point { x: 512, y: 200 }, Point { x: 538, y: 222 }];
        assert_eq!(at(place(a, 380, 300, WORK, 1.0, &others)), (560, 248));
        // 错开后出了工作区就夹回来（这时会和旧卡片重叠，接受）
        let edge = [Point { x: 1532, y: 724 }];
        assert_eq!(at(place(r(1400, 800, 1520, 900), 380, 300, WORK, 1.0, &edge)), (1532, 724));
    }

    #[test]
    fn edges_and_corners() {
        let at = |x, y| edge_at(Point { x, y }, 380, 200, 6);
        // 角优先于边；离边不到 6 才算
        assert_eq!([at(0, 0), at(379, 0), at(0, 199), at(379, 199)], [Some(Edge::TopLeft), Some(Edge::TopRight), Some(Edge::BottomLeft), Some(Edge::BottomRight)]);
        assert_eq!([at(5, 100), at(374, 100), at(100, 5), at(100, 194)], [Some(Edge::Left), Some(Edge::Right), Some(Edge::Top), Some(Edge::Bottom)]);
        assert_eq!([at(6, 100), at(373, 100), at(100, 6), at(100, 193)], [None; 4]);
    }

    #[test]
    fn fit_edge_moves_only_dragged_side() {
        let (min, max) = ((300, 145), (1904, 1016));
        // 在范围内：原样
        assert_eq!(fit_edge(Edge::BottomRight, r(500, 300, 980, 600), min, max), r(500, 300, 980, 600));
        // 左边拖得太窄：停在 300，右边不动
        assert_eq!(fit_edge(Edge::Left, r(700, 300, 880, 500), min, max), r(580, 300, 880, 500));
        // 左上角：宽高都有下限，右下角不动
        assert_eq!(fit_edge(Edge::TopLeft, r(600, 420, 880, 500), min, max), r(580, 355, 880, 500));
        // 下边拖得比工作区还高：停在最大，上边不动
        assert_eq!(fit_edge(Edge::Bottom, r(500, 300, 880, 1400), min, max), r(500, 300, 880, 1316));
        // 工作区比最小还小：取最小
        assert_eq!(fit_edge(Edge::Right, r(0, 0, 100, 100), min, (200, 100)), r(0, 0, 300, 100));
    }

    fn size(w: Win) -> (i32, i32) {
        let r = w.rect();
        (r.right - r.left, r.bottom - r.top)
    }

    /// 拖边框改大小：只拖宽时正文重新折行、高度还按内容；拖过高度以后内容变了大小也不变；边框上是改大小的那一块。
    /// 每一步照系统改大小循环做的来：先问 sizing，再按改过的位置摆窗口。会弹出真的卡片窗口，平时不跑：
    /// cargo test resize_card -- --ignored
    #[test]
    #[ignore]
    fn resize_card() {
        let mon = plat::monitor_at(Point { x: 0, y: 0 });
        let anchor = Rect { left: mon.work.left + 100, top: mon.work.top + 100, right: mon.work.left + 300, bottom: mon.work.top + 200 };
        let w = create("取字", false, Body::Loading, anchor, mon).unwrap();
        let c = w.peek::<Card>();
        // 右边往右拉 dx，或者下边往下拉 dy
        let drag = |edge: Edge, dx: i32, dy: i32| {
            let mut r = w.rect();
            (r.right, r.bottom) = (r.right + dx, r.bottom + dy);
            let r = w.with(|c: &mut Card| c.sizing(w, edge, r)).unwrap();
            w.set_rect(r);
        };
        w.with(|c: &mut Card| c.arrive(w, Outcome::Text { text: "一段会自动折行的文字，".repeat(20), took: "0.1 秒".into(), lines: 1 }));
        let (w0, content0) = (size(w).0, c.content_h);
        drag(Edge::Right, 300, 0);
        assert!(size(w).0 == w0 + 300 && c.user_w.is_some() && c.user_h.is_none(), "{:?}", size(w));
        assert!(c.content_h < content0, "拖宽以后没有重新折行");
        assert_eq!(size(w), (c.width(), c.height()));
        // 往上拖到比最小还矮：停在标题栏 + 底栏 + 3 行
        drag(Edge::Bottom, 0, -2000);
        assert_eq!(size(w).1, c.pxi(HEAD + FOOT + 3.0 * LINE));
        drag(Edge::Bottom, 0, 200);
        let fixed = size(w);
        assert!(c.user_h.is_some());
        w.with(|c: &mut Card| c.arrive(w, Outcome::Text { text: "短".into(), took: "0.1 秒".into(), lines: 1 }));
        assert_eq!(size(w), fixed);
        // 右边框上是改大小，标题栏空白处是拖动
        let (cw, _) = size(w);
        assert_eq!(c.hit(w, Point { x: cw - 2, y: 100 }), Hit::Edge(Edge::Right));
        assert_eq!(c.hit(w, Point { x: 200, y: 20 }), Hit::Caption);
        w.close();
    }

    /// 翻译卡片走一遍：没配置 → 识别 → 本地 mock 回 401 → 换服务地址点重试 → 成功（原文默认折叠）→ 展开原文；
    /// 每一步把卡片画成 PNG 放进 target/test-tmp/card。会弹出真的卡片窗口，识别也用 mock，平时不跑：
    /// cargo test translate_card -- --ignored --nocapture
    #[test]
    #[ignore]
    fn translate_card() {
        use crate::plat::Bitmap;
        use crate::translate::tests::{http, mock};
        use std::time::Duration;

        /// 处理消息直到 done 成立，最多等 20 秒
        fn wait(w: Win, done: impl Fn(&Card) -> bool) {
            let t0 = Instant::now();
            while !done(w.peek::<Card>()) {
                assert!(t0.elapsed() < Duration::from_secs(20), "等超时了");
                if !plat::pump() {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        fn click(w: Win, b: Btn) {
            let r = w.peek::<Card>().buttons().into_iter().find(|x| x.0 == b).unwrap().1;
            let p = Point { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 };
            w.send(Event::Move { p, mods: Mods::default() });
            w.send(Event::Down { p, button: Button::Left, double: false, mods: Mods::default() });
            w.send(Event::Up { button: Button::Left });
        }
        /// 刚开的那张卡片
        fn card() -> Win {
            *Win::all(plat::Kind::Card).last().expect("没有卡片")
        }

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("card");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Jie")).unwrap();
        // 设置从这个目录读，不碰真的 %APPDATA%
        unsafe { std::env::set_var("APPDATA", &dir) };
        // 识别服务单独填 `ocr`，空的就沿用翻译的
        let use_service = |endpoint: &str, model: &str, ocr: &str| {
            std::fs::write(dir.join("Jie").join("settings.json"), format!(r#"{{"endpoint":"{endpoint}","model":"{model}","ocr_endpoint":"{ocr}","ocr_model":"vl"}}"#)).unwrap();
            settings::load();
        };
        let snap = |w: Win, name: &str| {
            let c = w.peek::<Card>();
            let bmp = Bitmap::new(c.width(), c.height()).unwrap();
            c.draw(&Canvas::new(&bmp, bmp.full(), false), bmp.w as f32, bmp.h as f32);
            std::fs::write(dir.join(name), plat::encode(&bmp.crop(&bmp.full()), false).unwrap()).unwrap();
        };
        // 识别服务回的文字
        let vl_reply = r#"{"choices":[{"message":{"content":"Most screenshot tools keep a GPU device alive all day.\nA frozen frame is only a bitmap."}}]}"#;
        let img = || crate::ocr::tests::render(Face::Segoe, 15.0, "Most screenshot tools keep a GPU device alive all day.\nA frozen frame is only a bitmap.");
        let mon = plat::monitor_at(Point { x: 0, y: 0 });
        let anchor = Rect { left: mon.work.left + 100, top: mon.work.top + 100, right: mon.work.left + 660, bottom: mon.work.top + 300 };

        // 没配置翻译服务：不识别，直接给出去设置的入口
        use_service("", "", "");
        translate(img(), anchor, mon);
        let w = card();
        assert!(matches!(w.peek::<Card>().body, Body::NoService(false)));
        snap(w, "0-no-service.png");
        w.close();

        // 识别完先显示原文、译文转圈；服务回 401
        let (bad, _req) = mock(Some(http("401 Unauthorized", r#"{"error":{"message":"bad key"}}"#)));
        let (vl, _vl_req) = mock(Some(http("200 OK", vl_reply)));
        use_service(&bad, "m1", &vl);
        translate(img(), anchor, mon);
        let w = card();
        wait(w, |c| matches!(c.body, Body::Translate(_)));
        snap(w, "1-busy.png");
        wait(w, |c| matches!(c.body, Body::Translate(Tr { state: TrState::Failed(_), .. })));
        assert!(matches!(&w.peek::<Card>().body, Body::Translate(Tr { state: TrState::Failed(e), .. }) if e == "API Key 无效（401），请到设置里检查。"));
        assert_eq!(w.peek::<Card>().foot_buttons().len(), 2);
        snap(w, "2-failed.png");

        // 改了设置再点「重试」：用新的地址和模型，只发原文文字
        let reply = r#"{"choices":[{"message":{"content":"大多数截图工具整天开着 GPU 设备。\n冻结的一帧只是一张位图。"}}]}"#;
        let (good, req) = mock(Some(http("200 OK", reply)));
        use_service(&good, "m2", "");
        click(w, Btn::Retry);
        wait(w, |c| matches!(c.body, Body::Translate(Tr { state: TrState::Done(..), .. })));
        let c = w.peek::<Card>();
        let sent: serde_json::Value = serde_json::from_str(req.recv().unwrap().split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(sent["model"], "m2");
        assert_eq!(sent["messages"][1]["content"].as_str(), c.part_text(Part::Orig));
        assert_eq!(c.pair, "英 → 中");
        assert!(c.meta.ends_with(" 秒") && c.foot_buttons().is_empty(), "{:?}", c.meta);
        // 默认折叠：只有译文
        assert!(c.block(Part::Orig).is_none() && c.block(Part::Main).is_some());
        snap(w, "3-done.png");

        // 展开原文，下一张卡片也记得；再折叠回来
        click(w, Btn::Toggle);
        assert!(ORIG_OPEN.load(Ordering::Relaxed) && w.peek::<Card>().block(Part::Orig).is_some());
        snap(w, "4-expanded.png");
        click(w, Btn::Toggle);
        assert!(!ORIG_OPEN.load(Ordering::Relaxed));
        w.close();

        // 取字：单独填了地址却没有 Key，给去设置的入口（翻到文字识别页）
        let set = |json: &str| {
            std::fs::write(dir.join("Jie").join("settings.json"), json).unwrap();
            settings::load();
        };
        set(r#"{"ocr_endpoint":"https://example.com/v1","ocr_model":"vl"}"#);
        ocr(img(), anchor, mon);
        let w = card();
        assert!(matches!(w.peek::<Card>().body, Body::NoService(true)));
        snap(w, "5-no-ocr-service.png");
        w.close();

        // 配好了：截图编成 PNG 发过去，回来的文字进卡片，标题栏写行数和用时
        let (vl, req) = mock(Some(http("200 OK", vl_reply)));
        set(&format!(r#"{{"ocr_endpoint":"{vl}","ocr_model":"vl"}}"#));
        ocr(img(), anchor, mon);
        let w = card();
        wait(w, |c| matches!(c.body, Body::Text(_)));
        let sent = req.recv().unwrap();
        assert!(sent.contains("data:image/png;base64,iVBORw0KGgo") && sent.contains(r#""model":"vl""#), "{}", &sent[..400.min(sent.len())]);
        let meta = &w.peek::<Card>().meta;
        assert!(meta.starts_with("2 行 · ") && meta.ends_with(" 秒"), "{meta}");
        snap(w, "6-llm-text.png");
        w.close();
        println!("卡片截图在 {}", dir.display());
    }
}
