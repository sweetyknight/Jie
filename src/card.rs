//! 文字卡片：OCR 和翻译结果共用，标题栏「识别 | 翻译」两个标签，同一张卡片里切换、就地翻译。
//! 置顶的透明窗口，白底、2 墨色描边、6,6 实心阴影、圆角 12，标题栏奶油色；
//! 正文自绘，自己做拖选、Ctrl+A、Ctrl+C、滚轮滚动，选中底色才能是浅紫。
//! 正文按原图的版面结构排（doc.rs）：标题、代码块、带格线的表格，图片的位置贴上截图里裁出的那块；
//! 有表格、图片时卡片自动加宽。选中、复制用纯文本，单元格之间是 Tab。
//! 尺寸按设计稿（DIP）乘卡片所在显示器的缩放比例。

use crate::doc::{self, Doc, Node};
use crate::ocr::{self, Outcome};
use crate::plat::{self, Button, Canvas, Color, Cursor, Edge, Event, Face, Handler, Hit, Image, Key, Monitor, Point, Rect, RectF, Spec, Stroke, Text, Win};
use crate::settings;
use crate::toast::{self, Kind};
use crate::translate;
use crate::ui::{self, Hud, rectf, rgb, rgba};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const T_SPIN: usize = 1;
/// 标题栏的复制按钮变成对勾 1.5 秒
const T_COPIED: usize = 2;
/// 标题栏按钮的工具提示：悬停 400ms 后出现
const T_TIP: usize = 3;
const FRAME_MS: u32 = 16;

// 设计稿尺寸（DIP），第 4 版
const WIDTH: f32 = 380.0;
const HEAD: f32 = 50.0;
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
/// 右下的实心阴影：窗口比卡片大出这一圈
const SHADOW: f32 = 6.0;

const TEXT: u32 = ui::INK;
const TEXT2: u32 = ui::MUTED;
const TEXT3: u32 = ui::FAINT;
const SELECTION: u32 = 0xE7D5FB;
const ERR: u32 = 0xC62828;
const ERR_BG: u32 = 0xFFE9E6;
/// 表格格线、图片边框
const GRID: u32 = 0xD3CCD9;
/// 图片占位框的底
const MISSING: u32 = 0xF3F1F4;

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
    /// 标题栏的「识别」「翻译」两个标签（true 是翻译）
    Tab(bool),
    /// 标题栏的复制：取字结果或译文
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
    Text(Doc),
    /// 还没配置翻译服务（true 是大模型取字的服务）：去设置的入口
    NoService(bool),
    Translate(Tr),
}

/// 翻译卡片的正文：译文在上，原文在下（默认折叠）
struct Tr {
    orig: Doc,
    /// 原文的语言（认不出是 None），原文开关行「原文 · English · 3 行」用
    src: Option<translate::Lang>,
    open: bool,
    state: TrState,
}

enum TrState {
    Busy,
    /// 译文（用时写在标题栏附注里）
    Done(Doc),
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
    /// 折行宽度：点在哪一段附近按它算
    w: f32,
    layout: Text,
    color: u32,
    part: Part,
    /// 这段字在那一部分纯文本里的起点和长度（UTF-16）：一部分的字分在好几段里（表格每格一段）
    start: u32,
    len: u32,
}

impl Block {
    fn new(x: f32, y: f32, w: f32, layout: Text, color: u32, part: Part, run: &doc::Run) -> Block {
        Block { x, y, w, layout, color, part, start: run.start, len: run.text.encode_utf16().count() as u32 }
    }
}

/// 不是字的东西：格线、表头和代码块的底色、图片；坐标和 Block 一样
struct Deco {
    r: RectF,
    kind: DecoKind,
    part: Part,
}

enum DecoKind {
    Fill(u32),
    /// 裁出的第几张图；没有这张（坐标不对，或者译文里的序号对不上）画占位框
    Img(Option<usize>),
}

struct Card {
    scale: f32,
    /// 所在显示器的工作区，卡片夹在里面
    work: Rect,
    /// 识别完接着翻译
    translate: bool,
    /// 现在是「翻译」标签（否则「识别」）
    tab_tr: bool,
    /// 另一个标签的正文和它的附注、语言标签：切回去不用重来
    other: Option<(Body, String, String)>,
    /// 翻译的语言标签「英 → 中」，空的不画
    pair: String,
    /// 标题栏附注「3 行 · 1.4 秒」
    meta: String,
    body: Body,
    blocks: Vec<Block>,
    decos: Vec<Deco>,
    /// 截图里裁出的图片，识别结果和译文共用
    images: Vec<Image>,
    /// 选区的屏幕坐标：卡片加宽时按它重新挑位置
    src: Rect,
    /// 卡片最后一次被摆到的左上角；窗口不在这儿了说明用户拖过，`moved` 之后就不再替它挪
    at: Point,
    moved: bool,
    /// 没拖过宽度时卡片的宽（DIP）：380，有表格、图片时加宽
    fit_w: f32,
    /// 正文排版的总高度（像素，含内边距）
    content_h: f32,
    /// 原文开关行的顶边（像素，相对正文区，没减滚动量）
    row_y: f32,
    /// 标题栏的复制刚点过，显示对勾
    copied: bool,
    scroll: f32,
    /// 选中范围在哪一段；起点和光标是这一段的 UTF-16 下标
    sel_part: Part,
    anchor: u32,
    caret: u32,
    selecting: bool,
    hot: Option<Btn>,
    down: Option<Btn>,
    /// 显示提示的按钮
    tip: Option<Btn>,
    angle: f32,
    /// 拖边框定下的宽、高（DIP）；没拖过的方向是 None：宽 380，高按内容
    user_w: Option<f32>,
    user_h: Option<f32>,
}

/// 取字：先弹出「识别中」的卡片，在工作线程里识别，结果交回卡片。`anchor` 是选区的屏幕坐标。
pub fn ocr(img: Image, anchor: Rect, mon: Monitor) {
    let s = settings::get();
    if !s.ocr_service().ready() {
        create(false, Body::NoService(true), anchor, mon);
    } else if let Some(w) = create(false, Body::Loading, anchor, mon) {
        recognize(w, img);
    }
}

/// 翻译：先取字，原文显示出来以后再请求翻译（只发文字）。翻译服务或识别服务没配好（不是本机服务又没填 Key）时不识别，直接给出去设置的入口。
pub fn translate(img: Image, anchor: Rect, mon: Monitor) {
    let s = settings::get();
    if !s.translate_service().ready() {
        create(true, Body::NoService(false), anchor, mon);
    } else if !s.ocr_service().ready() {
        create(true, Body::NoService(true), anchor, mon);
    } else if let Some(w) = create(true, Body::Loading, anchor, mon) {
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

/// 表格每列不折行要多宽（像素，含格内左右各 8 的留白）；`px` 是格里的字号
fn table_cols(rows: &[Vec<doc::Run>], head: bool, px: f32) -> Option<Vec<f32>> {
    let mut w = vec![0.0f32; rows.iter().map(Vec::len).max().unwrap_or(0)];
    for (r, row) in rows.iter().enumerate() {
        for (c, run) in row.iter().enumerate().filter(|(_, run)| !run.text.is_empty()) {
            w[c] = w[c].max(Text::new(Face::Ui, px, head && r == 0, &run.text)?.width());
        }
    }
    // 多留 1 像素：格线取整会让列窄掉不到 1 像素，刚好放得下的字就折行了
    Some(w.into_iter().map(|w| w.ceil() + 1.0 + 16.0 * px / 14.0).collect())
}

/// 列宽：放得下就按各列自然宽；放不下时不比平均宽的列照旧，其余按自然宽的比例分剩下的
fn fit_cols(natural: &[f32], width: f32) -> Vec<f32> {
    let total: f32 = natural.iter().sum();
    if total <= width {
        return natural.to_vec();
    }
    let fair = width / natural.len() as f32;
    let (narrow, wide) = natural.iter().fold((0.0, 0.0), |(n, w), &c| if c <= fair { (n + c, w) } else { (n, w + c) });
    natural.iter().map(|&c| if c <= fair { c } else { c * (width - narrow) / wide }).collect()
}

/// 排一张表，返回高度：格里折行，一行高按最高那格，上下留 5；表头一行粗体、奶油色底；格线 1 DIP
#[allow(clippy::too_many_arguments)]
fn layout_table(rows: &[Vec<doc::Run>], head: bool, (x, y): (f32, f32), width: f32, px: f32, color: u32, part: Part, v: &mut Vec<Block>, deco: &mut Vec<Deco>) -> Option<f32> {
    let u = px / 14.0;
    let (pad, vpad, line) = (8.0 * u, 5.0 * u, u.round().max(1.0));
    let cols = fit_cols(&table_cols(rows, head, px)?, width);
    // 格线落在整像素上，不发虚
    let mut xs = vec![x.round()];
    for c in &cols {
        xs.push((xs.last().unwrap() + c).round());
    }
    let mut ys = vec![y.round()];
    for (r, row) in rows.iter().enumerate() {
        let (top, bold) = (*ys.last().unwrap(), head && r == 0);
        let mut h = px * 1.5;
        for (c, run) in row.iter().enumerate().filter(|(_, run)| !run.text.is_empty()) {
            let w = xs[c + 1] - xs[c] - 2.0 * pad;
            let layout = Text::wrapped(Face::Ui, px, bold, &run.text, w, Some(px * 1.5))?;
            h = h.max(layout.height());
            v.push(Block::new(xs[c] + pad, top + vpad, w, layout, color, part, run));
        }
        let bottom = (top + h + 2.0 * vpad).round();
        if bold {
            deco.push(Deco { r: rectf(xs[0], top, *xs.last().unwrap(), bottom), kind: DecoKind::Fill(ui::CREAM), part });
        }
        ys.push(bottom);
    }
    let (l, r, t, b) = (xs[0], *xs.last().unwrap(), ys[0], *ys.last().unwrap());
    let fill = |r: RectF| Deco { r, kind: DecoKind::Fill(GRID), part };
    // 最右、最下那条往里收一线，整张表不超出 width
    deco.extend(ys.iter().map(|&gy| fill(rectf(l, gy.min(b - line), r, gy.min(b - line) + line))));
    deco.extend(xs.iter().map(|&gx| fill(rectf(gx.min(r - line), t, gx.min(r - line) + line, b))));
    Some(b - y)
}

fn create(translate: bool, body: Body, anchor: Rect, mon: Monitor) -> Option<Win> {
    let loading = matches!(body, Body::Loading);
    let mut card = Card::new(mon.scale(), mon.work, translate, body);
    card.src = anchor;
    card.relayout();
    let (w, h) = (card.width(), card.height());
    let others: Vec<Point> = Win::all(plat::Kind::Card).iter().map(|c| c.rect()).map(|r| Point { x: r.left, y: r.top }).collect();
    let at = place(anchor, w, h, mon.work, mon.scale(), &others);
    card.at = at;
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
    fn new(scale: f32, work: Rect, translate: bool, body: Body) -> Card {
        Card {
            scale,
            work,
            translate,
            tab_tr: translate,
            other: None,
            pair: String::new(),
            meta: String::new(),
            body,
            blocks: Vec::new(),
            decos: Vec::new(),
            images: Vec::new(),
            src: Rect::default(),
            at: Point::default(),
            moved: false,
            fit_w: WIDTH,
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
            tip: None,
            angle: 0.0,
            user_w: None,
            user_h: None,
        }
    }

    fn px(&self, dip: f32) -> f32 {
        dip * self.scale
    }

    fn pxi(&self, dip: f32) -> i32 {
        self.px(dip).round() as i32
    }

    /// 卡片宽（DIP），正文按它折行
    fn w_dip(&self) -> f32 {
        self.user_w.unwrap_or(self.fit_w)
    }

    /// 卡片本体的宽（像素，不含阴影）
    fn body_w(&self) -> i32 {
        self.pxi(self.w_dip())
    }

    fn shadow(&self) -> i32 {
        self.pxi(SHADOW)
    }

    /// 窗口宽高：卡片本体加右下的阴影
    fn width(&self) -> i32 {
        self.body_w() + self.shadow()
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
        self.head_h() + self.body_h() + self.foot_h() + self.shadow()
    }

    fn body_rect(&self) -> Rect {
        let top = self.head_h();
        Rect { left: 0, top, right: self.body_w(), bottom: top + self.body_h() }
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

    /// 能选中的一部分的纯文本：Main 是取字结果或译文，Orig 是原文
    fn part_text(&self, part: Part) -> Option<&str> {
        match (&self.body, part) {
            (Body::Text(d), Part::Main) | (Body::Translate(Tr { state: TrState::Done(d), .. }), Part::Main) => Some(&d.plain),
            (Body::Translate(tr), Part::Orig) => Some(&tr.orig.plain),
            _ => None,
        }
    }

    /// 这一部分排出来了没有；原文折叠时没有
    fn has(&self, part: Part) -> bool {
        self.blocks.iter().any(|b| b.part == part)
    }

    /// 按当前内容和缩放比例重新排版
    fn relayout(&mut self) {
        self.fit_w = self.fit_width();
        (self.blocks, self.decos, self.content_h, self.row_y) = self.layout_body().unwrap_or_default();
        // 拖高了、内容填不满：原文开关行（连同展开的原文）贴到最底下，空白留在译文下面
        if self.user_h.is_some() && matches!(self.body, Body::Translate(_)) {
            let extra = self.body_h() as f32 - self.content_h;
            if extra > 0.0 {
                self.row_y += extra;
                self.blocks.iter_mut().filter(|b| b.part == Part::Orig).for_each(|b| b.y += extra);
                for d in self.decos.iter_mut().filter(|d| d.part == Part::Orig) {
                    (d.r.top, d.r.bottom) = (d.r.top + extra, d.r.bottom + extra);
                }
                self.content_h += extra;
            }
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    /// 没拖过宽度时卡片的宽（DIP）：内容里最宽的表格、图片不折行放得下，最宽到选区宽和工作区宽（减边距）里窄的那个，最少 380
    fn fit_width(&self) -> f32 {
        let s = self.scale;
        let main = match &self.body {
            Body::Text(d) | Body::Translate(Tr { state: TrState::Done(d), .. }) => self.natural_w(d, 15.0 * s) + 2.0 * PAD * s,
            _ => 0.0,
        };
        let orig = match &self.body {
            Body::Translate(tr) if tr.open => self.natural_w(&tr.orig, 13.0 * s) + 54.0 * s,
            _ => 0.0,
        };
        let cap = ((self.src.right - self.src.left) as f32 + 2.0 * PAD * s).min((self.work.right - self.work.left) as f32 - 16.0 * s);
        (main.max(orig).min(cap) / s).max(WIDTH)
    }

    /// 文档里表格不折行、图片按原大小时最宽的那个有多宽（像素）；只有字是 0
    fn natural_w(&self, d: &Doc, px: f32) -> f32 {
        d.nodes
            .iter()
            .map(|n| match n {
                Node::Table { rows, head } => table_cols(rows, *head, px * 14.0 / 15.0).map_or(0.0, |c| c.iter().sum()),
                Node::Image(Some(i)) => self.images.get(*i).map_or(0.0, |m| m.w as f32),
                _ => 0.0,
            })
            .fold(0.0, f32::max)
    }

    /// 正文的各段文字、格线图片这些装饰、总高度、原文开关行的顶边。正文是界面字 15 号（Windows 上中日文由 DirectWrite 按 zh-CN 回落到微软雅黑），
    /// 段内只换行，空行分开的段、表格、图片之间空 0.6 个字高。
    fn layout_body(&self) -> Option<(Vec<Block>, Vec<Deco>, f32, f32)> {
        let s = self.scale;
        let wrap = self.px(self.w_dip() - 2.0 * PAD);
        let (mut v, mut d) = (Vec::new(), Vec::new());
        // 放一段说明文字，返回它的高度
        let plain = |v: &mut Vec<Block>, x: f32, y: f32, layout: Text, color: u32| {
            let h = layout.height();
            v.push(Block { x, y, w: layout.width(), layout, color, part: Part::Plain, start: 0, len: 0 });
            h
        };
        let small = |t: &str, width: f32| Text::wrapped(Face::Ui, 13.0 * s, false, t, width, Some(13.0 * 1.5 * s));
        let mut row_y = 0.0;
        let h = match &self.body {
            Body::Loading => 0.0,
            // 上 2、下 18
            Body::Text(t) => 2.0 * s + self.layout_doc(t, (PAD * s, 2.0 * s), wrap, 15.0 * s, TEXT, Part::Main, &mut v, &mut d)? + 18.0 * s,
            // 标题 14/600，空 4，说明 13 号、行高 1.5，下 16
            Body::NoService(ocr) => {
                let text = if *ocr { NO_OCR_SERVICE } else { NO_SERVICE };
                let y = plain(&mut v, PAD * s, 0.0, Text::wrapped(Face::Ui, 14.0 * s, true, text[0], wrap, None)?, TEXT) + 4.0 * s;
                y + plain(&mut v, PAD * s, y, small(text[1], wrap)?, TEXT2) + 16.0 * s
            }
            Body::Translate(tr) => {
                let mut y = match &tr.state {
                    // 上 8 + 转圈 16 + 下 22
                    TrState::Busy => 46.0 * s,
                    TrState::Done(t) => 2.0 * s + self.layout_doc(t, (PAD * s, 2.0 * s), wrap, 15.0 * s, TEXT, Part::Main, &mut v, &mut d)? + 14.0 * s,
                    // 浅红提示条：外边距上 2、左右 16、下 14，内边距 10 / 12，警告图标 16 + 间距 8；原因 13 号、行高 1.5
                    TrState::Failed(e) => {
                        let y = 12.0 * s;
                        y + plain(&mut v, 52.0 * s, y, small(e, self.px(self.w_dip() - 80.0))?, ERR) + 24.0 * s
                    }
                };
                // 原文开关行；展开时原文在它下面：13 号灰字、行高 1.5，左边竖线 2 + 空 12，下 16
                row_y = y;
                y += ORIG_ROW * s;
                if tr.open {
                    y += self.layout_doc(&tr.orig, (34.0 * s, y), self.px(self.w_dip() - 54.0), 13.0 * s, TEXT2, Part::Orig, &mut v, &mut d)? + 16.0 * s;
                }
                y
            }
        };
        Some((v, d, h, row_y))
    }

    /// 排一份文档：左上角 (x, y)、宽 `width`、正文 `px` 号字（都是像素），返回高度。
    /// 标题粗、大 2 号；代码等宽、小 2 号、奶油色底、内边距 10；表格小 1 号；图片按原大小，太宽缩到放得下。
    #[allow(clippy::too_many_arguments)]
    fn layout_doc(&self, d: &Doc, (x, y): (f32, f32), width: f32, px: f32, color: u32, part: Part, v: &mut Vec<Block>, deco: &mut Vec<Deco>) -> Option<f32> {
        let u = px / 15.0;
        let mut cy = y;
        for (i, n) in d.nodes.iter().enumerate() {
            if i > 0 {
                cy += 9.0 * u;
            }
            match n {
                Node::Text(kind, run) => {
                    let (face, size, bold, inset) = match kind {
                        doc::Kind::Para => (Face::Ui, px, false, 0.0),
                        doc::Kind::Heading => (Face::Ui, px + 2.0 * u, true, 0.0),
                        doc::Kind::Code => (Face::Mono, px - 2.0 * u, false, 10.0 * u),
                    };
                    let w = width - 2.0 * inset;
                    let layout = Text::wrapped(face, size, bold, &run.text, w, Some(size * 1.5))?;
                    let h = layout.height() + 2.0 * inset;
                    if inset > 0.0 {
                        deco.push(Deco { r: rectf(x, cy, x + width, cy + h), kind: DecoKind::Fill(ui::CREAM), part });
                    }
                    v.push(Block::new(x + inset, cy + inset, w, layout, color, part, run));
                    cy += h;
                }
                Node::Table { rows, head } => cy += layout_table(rows, *head, (x, cy), width, px - u, color, part, v, deco)?,
                Node::Image(i) => {
                    let img = i.filter(|&i| i < self.images.len());
                    let (w, h) = match img.map(|i| &self.images[i]) {
                        Some(m) => {
                            let w = (m.w as f32).min(width);
                            (w, m.h as f32 * w / m.w as f32)
                        }
                        None => ((120.0 * u).min(width), 48.0 * u),
                    };
                    deco.push(Deco { r: rectf(x, cy, x + w, cy + h), kind: DecoKind::Img(img), part });
                    cy += h;
                }
            }
        }
        Some(cy - y)
    }

    fn buttons(&self) -> Vec<(Btn, Rect)> {
        let (w, b) = (self.body_w(), self.pxi(30.0));
        let top = (self.head_h() - b) / 2;
        // 右边：复制、关闭，30 见方、间距 3、离右边 10
        let close = Rect { left: w - self.pxi(10.0) - b, top, right: w - self.pxi(10.0), bottom: top + b };
        let copy = Rect { left: close.left - self.pxi(3.0) - b, right: close.left - self.pxi(3.0), ..close };
        let tw = |label: &str| Text::new(Face::Ui, 13.0 * self.scale, false, label).map_or(0.0, |l| l.width());
        // 左边：「识别 | 翻译」分段，外框离左边 10、高 34，里面两个标签高 28、字左右各 12、间距 2
        let (tt, th) = ((self.head_h() - self.pxi(28.0)) / 2, self.pxi(28.0));
        let t0 = Rect { left: self.pxi(13.0), top: tt, right: self.pxi(13.0) + (tw("识别") + self.px(24.0)).round() as i32, bottom: tt + th };
        let t1 = Rect { left: t0.right + self.pxi(2.0), right: t0.right + self.pxi(2.0) + (tw("翻译") + self.px(24.0)).round() as i32, ..t0 };
        let mut v = vec![(Btn::Tab(false), t0), (Btn::Tab(true), t1), (Btn::Copy, copy), (Btn::Close, close)];
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

    /// 能点的按钮：复制要有内容；标签只能点另一个，并且要已经有识别出的文字（或者另一个标签有内容）
    fn enabled(&self, b: Btn) -> bool {
        match b {
            Btn::Copy => self.part_text(Part::Main).is_some(),
            Btn::Tab(t) => t != self.tab_tr && (matches!(self.body, Body::Text(_) | Body::Translate(_)) || self.other.is_some()),
            _ => true,
        }
    }

    /// 标题栏按钮的提示：(名字, 快捷键)；标签没有提示
    fn tip_text(&self, b: Btn) -> Option<(&'static str, &'static str)> {
        Some(match b {
            Btn::Copy => (if self.tab_tr { "复制译文" } else { "复制全文" }, ""),
            Btn::Close => ("关闭", "Esc"),
            Btn::CopyOrig => ("复制原文", ""),
            _ => return None,
        })
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
        let has = |part: Part| self.has(part).then_some(part);
        match self.body {
            Body::Text(_) => Some(Part::Main),
            Body::Translate(_) if y < self.row_y => has(Part::Main),
            Body::Translate(_) if y >= self.row_y + self.px(ORIG_ROW) => has(Part::Orig),
            _ => None,
        }
    }

    /// 点对应的选中那部分纯文本里的位置：先找上下最近、再找左右最近的一段（表格同一行里按列），取那段里最近的字
    fn pos_at(&self, p: Point) -> u32 {
        let (x, y) = (p.x as f32, p.y as f32 - self.body_rect().top as f32 + self.scroll);
        let off = |lo: f32, hi: f32, v: f32| (lo - v).max(v - hi).max(0.0);
        let near = |b: &&Block| (off(b.y, b.y + b.layout.height(), y), off(b.x, b.x + b.w, x));
        let blocks = self.blocks.iter().filter(|b| b.part == self.sel_part);
        let Some(b) = blocks.min_by(|a, b| {
            let (a, b) = (near(a), near(b));
            a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
        }) else {
            return 0;
        };
        b.start + b.layout.hit(x - b.x, y - b.y)
    }

    fn selection(&self) -> (u32, u32) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    /// 选中的文字；原文折叠着就没有
    fn selected(&self) -> String {
        let (Some(t), true) = (self.part_text(self.sel_part), self.has(self.sel_part)) else { return String::new() };
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

    /// 内容变了：重新排版，窗口改大小，位置不动（夹回工作区）；
    /// 用户没拖过卡片、又因为表格图片变宽了，就按新大小重新挑位置，免得盖住选区
    fn refit(&mut self, win: Win) {
        let old_w = self.width();
        self.relayout();
        let r = win.rect();
        self.moved |= (r.left, r.top) != (self.at.x, self.at.y);
        let (w, h, m) = (self.width(), self.height(), self.pxi(8.0));
        self.at = if w != old_w && !self.moved {
            let others: Vec<Point> = Win::all(plat::Kind::Card).into_iter().filter(|&c| c != win).map(|c| c.rect()).map(|r| Point { x: r.left, y: r.top }).collect();
            place(self.src, w, h, self.work, self.scale, &others)
        } else {
            Point { x: r.left.min(self.work.right - m - w).max(self.work.left + m), y: r.top.min(self.work.bottom - m - h).max(self.work.top + m) }
        };
        win.set_rect(Rect { left: self.at.x, top: self.at.y, right: self.at.x + w, bottom: self.at.y + h });
        win.invalidate_all();
    }

    /// 拖边框：系统给的新窗口夹到最小、最大之间；拖了哪个方向就定下哪个方向的大小，
    /// 之后内容变了也不再自动改；正文按新宽度重新折行
    fn drag_size(&mut self, edge: Edge, r: Rect) -> Rect {
        // 窗口连阴影，记下的宽高不连
        let (w, m, sh) = (self.work, self.pxi(8.0), self.shadow());
        let min = (self.pxi(MIN_WIDTH) + sh, self.pxi(HEAD + FOOT + 3.0 * LINE) + sh);
        let max = (w.right - w.left - 2 * m, w.bottom - w.top - 2 * m);
        let mut r = fit_edge(edge, r, min, max);
        let (left, right, top, bottom) = edge.sides();
        if left || right {
            self.user_w = Some((r.right - r.left - sh) as f32 / self.scale);
        }
        if top || bottom {
            self.user_h = Some((r.bottom - r.top - sh) as f32 / self.scale);
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
            Outcome::Text { text, took, images } => {
                let d = doc::parse(&text);
                // 只认出了图片也算没有字
                if d.plain.trim().is_empty() {
                    toast::show(&w.monitor(), Kind::Err, "没有识别到文字");
                    return true;
                }
                self.images = images;
                if self.translate {
                    // 翻译：译文的位置转圈，原文开关已经能点
                    let src = translate::detect(&d.plain);
                    let open = ORIG_OPEN.load(Ordering::Relaxed);
                    self.body = Body::Translate(Tr { orig: d, src, open, state: TrState::Busy });
                    self.request(w);
                } else {
                    self.meta = format!("{} 行 · {took}", d.lines());
                    self.body = Body::Text(d);
                }
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
        let to = translate::pick_target(&tr.orig.plain, cfg.target).short();
        self.pair = tr.src.map_or(format!("→ {to}"), |l| format!("{} → {to}", l.short()));
        self.meta.clear();
        tr.state = TrState::Busy;
        if self.sel_part == Part::Main {
            (self.anchor, self.caret) = (0, 0);
        }
        // 发模型给的原样文字：表格、图片标记跟着带过去，译文排出来还是原来的结构
        let text = tr.orig.src.clone();
        std::thread::spawn(move || {
            let t0 = Instant::now();
            let out: Translated = translate::translate(&cfg, &text, translate::TIMEOUT)
                .map(|t| (t, format!("{:.1} 秒", t0.elapsed().as_secs_f32())))
                .map_err(|e| e.to_string());
            w.post(move |c: &mut Card, w| c.translated(w, out));
        });
        w.timer(T_SPIN, FRAME_MS);
    }

    /// 译文到了；翻译中切到了「识别」标签的话，记到另一个标签上
    fn translated(&mut self, w: Win, out: Translated) {
        let (tr, meta) = match (&mut self.body, &mut self.other) {
            (Body::Translate(tr), _) => (tr, &mut self.meta),
            (_, Some((Body::Translate(tr), meta, _))) => (tr, meta),
            _ => return,
        };
        tr.state = match out {
            Ok((t, took)) => {
                *meta = took;
                TrState::Done(doc::parse(&t))
            }
            Err(e) => TrState::Failed(e),
        };
        w.kill_timer(T_SPIN);
        self.refit(w);
    }

    /// 切到「识别」（`tr` 为 false）或「翻译」标签：另一个标签已经有内容就换回来，没有就拿识别出的文字现做（翻译要请求一次）
    fn switch_tab(&mut self, w: Win, tr: bool) {
        if tr == self.tab_tr {
            return;
        }
        let cur = (std::mem::replace(&mut self.body, Body::Loading), std::mem::take(&mut self.meta), std::mem::take(&mut self.pair));
        let text = match &cur.0 {
            Body::Text(t) => Some(t.clone()),
            Body::Translate(t) => Some(t.orig.clone()),
            _ => None,
        };
        (self.tab_tr, self.copied, self.sel_part, self.anchor, self.caret, self.scroll) = (tr, false, Part::Main, 0, 0, 0.0);
        // 「没配置」的那页不留着：可能刚去设置里配好
        match (self.other.take().filter(|(b, ..)| !matches!(b, Body::NoService(_))), text) {
            (Some((b, m, p)), _) => (self.body, self.meta, self.pair) = (b, m, p),
            (None, Some(orig)) if tr => {
                if settings::get().translate_service().ready() {
                    let (src, open) = (translate::detect(&orig.plain), ORIG_OPEN.load(Ordering::Relaxed));
                    self.body = Body::Translate(Tr { orig, src, open, state: TrState::Busy });
                    self.request(w);
                } else {
                    self.body = Body::NoService(false);
                }
            }
            (None, Some(orig)) => {
                self.meta = format!("{} 行", orig.lines());
                self.body = Body::Text(orig);
            }
            (None, None) => {}
        }
        self.other = Some(cur);
        self.refit(w);
    }

    fn toggle(&mut self, w: Win) {
        let Body::Translate(tr) = &mut self.body else { return };
        tr.open = !tr.open;
        ORIG_OPEN.store(tr.open, Ordering::Relaxed);
        self.refit(w);
    }

    /// 画卡片本体（w × h 不含阴影，阴影画在外面）：奶油色标题栏、白色正文，外面一圈 2 墨色描边
    fn draw(&self, c: &Canvas, w: f32, h: f32) {
        let s = self.scale;
        let (lw, head, rad) = (ui::line_w(s), self.head_h() as f32, 12.0 * s);
        ui::block(c, rectf(0.0, 0.0, w, h), 12.0, SHADOW, ui::CREAM, s);
        // 标题栏下面换成白底（下边两个圆角跟着外框），再画标题栏下的分隔线
        c.fill_round(rectf(lw, head, w - lw, h - lw), (rad - lw).max(0.0), rgb(ui::SURF));
        c.fill_rect(rectf(lw, head, w - lw, (head + rad).min(h - lw)), rgb(ui::SURF));
        c.fill_rect(rectf(lw, head - lw, w - lw, head), rgb(ui::INK));
        let buttons = self.buttons();
        let _ = (|| -> Option<()> {
            let find = |b: Btn| buttons.iter().find(|(x, _)| *x == b).map(|(_, r)| *r);
            let (t0, t1) = (find(Btn::Tab(false))?, find(Btn::Tab(true))?);
            // 「识别 | 翻译」分段：白底、墨色描边，当前标签墨底白字
            let p = self.px(3.0).round();
            let seg = rectf(t0.left as f32 - p, t0.top as f32 - p, t1.right as f32 + p, t0.bottom as f32 + p);
            c.fill_round(seg, 7.0 * s, rgb(ui::SURF));
            c.stroke_round(rectf(seg.left + lw / 2.0, seg.top + lw / 2.0, seg.right - lw / 2.0, seg.bottom - lw / 2.0), 7.0 * s - lw / 2.0, rgb(ui::INK), lw);
            for (tab, r, name) in [(false, t0, "识别"), (true, t1, "翻译")] {
                let b = Btn::Tab(tab);
                let rf = rectf(r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
                let fg = if tab == self.tab_tr {
                    c.fill_round(rf, 5.0 * s, rgb(ui::INK));
                    0xFFFFFF
                } else if !self.enabled(b) {
                    TEXT3
                } else if self.hot == Some(b) {
                    c.fill_round(rf, 5.0 * s, rgb(if self.down == Some(b) { ui::PRESS } else { ui::HOVER }));
                    TEXT
                } else {
                    TEXT2
                };
                let l = Text::new(Face::Ui, 13.0 * s, true, name)?;
                c.text(&l, (rf.left + rf.right - l.width()) / 2.0, (rf.top + rf.bottom - l.height()) / 2.0, rgb(fg));
            }
            let mut x = seg.right + 8.0 * s;
            // 翻译的语言标签：柠檬黄底、墨色描边、高 22
            if !self.pair.is_empty() {
                let l = Text::new(Face::Ui, 12.0 * s, true, &self.pair)?;
                let cw = l.width() + 16.0 * s;
                let r = rectf(x, head / 2.0 - 11.0 * s, x + cw, head / 2.0 + 11.0 * s);
                c.fill_round(r, 6.0 * s, rgb(ui::PICK));
                c.stroke_round(rectf(r.left + lw / 2.0, r.top + lw / 2.0, r.right - lw / 2.0, r.bottom - lw / 2.0), 6.0 * s - lw / 2.0, rgb(ui::INK), lw);
                c.text(&l, x + 8.0 * s, (head - l.height()) / 2.0, rgb(TEXT));
                x += cw + 8.0 * s;
            }
            if !self.meta.is_empty() {
                let meta = Text::tabular(Face::Ui, 12.0 * s, false, &self.meta)?;
                meta.truncate(find(Btn::Copy)?.left as f32 - 8.0 * s - x);
                c.text(&meta, x, (head - meta.height()) / 2.0, rgb(TEXT2));
            }
            // 正文裁在自己的区域里，跟着滚动的原文开关行也是
            let body = self.body_rect();
            c.push_clip(rectf(lw, body.top as f32, w - lw, body.bottom as f32));
            let r = self.draw_body(c, w, &buttons);
            c.pop_clip();
            r?;
            for (b, r) in &buttons {
                if !matches!(b, Btn::Toggle | Btn::CopyOrig | Btn::Tab(_)) {
                    self.draw_button(c, *b, r)?;
                }
            }
            // 提示：按钮正下方，夹在卡片里
            if let Some(b) = self.tip
                && let (Some((name, key)), Some(r)) = (self.tip_text(b), find(b))
            {
                let parts: &[(&str, u32)] = if key.is_empty() { &[(name, 0xFFFFFF)] } else { &[(name, 0xFFFFFF), (key, ui::HUD_TEXT2)] };
                let hud = Hud::new(parts, s)?;
                let x = ((r.left + r.right - hud.w) / 2).clamp(lw as i32, (w as i32 - lw as i32 - hud.w).max(lw as i32));
                hud.draw(c, x, r.bottom + self.pxi(6.0));
            }
            Some(())
        })();
    }

    /// 转圈 + 一行灰字，转圈左上角在 (20, y + 8)
    fn draw_busy(&self, c: &Canvas, y: f32, text: &str) -> Option<()> {
        let s = self.scale;
        ui::spinner(c, PAD * s, y + 8.0 * s, s, self.angle, rgb(0xEBDDFB), rgb(ui::ACCENT));
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
                            let lw = ui::line_w(s);
                            c.fill_round(rectf(16.0 * s, y0, w - 16.0 * s, y1), 8.0 * s, rgb(ERR_BG));
                            c.stroke_round(rectf(16.0 * s + lw / 2.0, y0 + lw / 2.0, w - 16.0 * s - lw / 2.0, y1 - lw / 2.0), 8.0 * s - lw / 2.0, rgb(ui::INK), lw);
                            ui::icon(c, "warn", 28.0 * s, oy + b.y + (13.0 * 1.5 - 16.0) / 2.0 * s, 16.0 * s, rgb(ERR));
                        }
                    }
                    TrState::Done(_) => {}
                }
                // 开关行：白底墨色描边的一条「> 原文 · English · 3 行」，悬停浅紫、字变深
                if let Some((_, r)) = buttons.iter().find(|(b, _)| *b == Btn::Toggle) {
                    let hot = self.hot == Some(Btn::Toggle);
                    let (l, t, rr, bb) = (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
                    let fill = if !hot { ui::SURF } else if self.down == Some(Btn::Toggle) { ui::PRESS } else { ui::HOVER };
                    let lw = ui::line_w(s);
                    c.fill_round(rectf(l, t, rr, bb), 7.0 * s, rgb(fill));
                    c.stroke_round(rectf(l + lw / 2.0, t + lw / 2.0, rr - lw / 2.0, bb - lw / 2.0), 7.0 * s - lw / 2.0, rgb(ui::INK), lw);
                    let col = rgb(if hot { TEXT } else { TEXT2 });
                    ui::icon(c, if tr.open { "chev_down" } else { "chev" }, l + 6.0 * s, (t + bb) / 2.0 - 8.0 * s, 16.0 * s, col);
                    let label = match tr.src {
                        Some(l) => format!("原文 · {} · {} 行", l.label(), tr.orig.lines()),
                        None => format!("原文 · {} 行", tr.orig.lines()),
                    };
                    let lt = Text::new(Face::Ui, 12.0 * s, false, &label)?;
                    lt.truncate(rr - 6.0 * s - (l + 26.0 * s));
                    c.text(&lt, l + 26.0 * s, (t + bb - lt.height()) / 2.0, col);
                }
                for (b, r) in buttons.iter().filter(|(b, _)| *b == Btn::CopyOrig) {
                    self.draw_button(c, *b, r)?;
                }
                // 展开的原文左边一条 2 宽的浅紫竖线，从原文最上到最下
                let orig = self.blocks.iter().filter(|b| b.part == Part::Orig).map(|b| (b.y, b.y + b.layout.height()));
                let orig = orig.chain(self.decos.iter().filter(|d| d.part == Part::Orig).map(|d| (d.r.top, d.r.bottom)));
                if let Some((y0, y1)) = orig.reduce(|a, b| (a.0.min(b.0), a.1.max(b.1))) {
                    c.fill_rect(rectf(PAD * s, oy + y0, PAD * s + 2.0 * s, oy + y1), rgb(0xE6D6F8));
                }
            }
            Body::Text(_) | Body::NoService(_) => {}
        }
        // 底色、格线、图片在字下面；图片滚出正文区的不画（每次画都要建一张位图）
        for d in &self.decos {
            let r = rectf(d.r.left, oy + d.r.top, d.r.right, oy + d.r.bottom);
            match d.kind {
                DecoKind::Fill(col) => c.fill_rect(r, rgb(col)),
                _ if r.bottom < top || r.top > bottom => {}
                DecoKind::Img(i) => self.draw_image(c, r, i.map(|i| &self.images[i]))?,
            }
        }
        // 各段文字；选中的部分先铺浅紫底
        let (a, z) = self.selection();
        for b in &self.blocks {
            let (lo, hi) = (a.max(b.start), z.min(b.start + b.len));
            if b.part == self.sel_part && hi > lo {
                for (l, t, mw, mh) in b.layout.range_rects(lo - b.start, hi - b.start) {
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

    /// 截图里裁出的图，外面一圈 1 DIP 格线色的框；没有图时浅灰底、灰框，中间写「图片」
    fn draw_image(&self, c: &Canvas, r: RectF, img: Option<&Image>) -> Option<()> {
        let line = self.scale.round().max(1.0);
        let inner = rectf(r.left + line / 2.0, r.top + line / 2.0, r.right - line / 2.0, r.bottom - line / 2.0);
        match img {
            Some(m) => c.pixels(&m.px, m.w as u32, m.h as u32, r),
            None => {
                c.fill_rect(r, rgb(MISSING));
                let l = Text::centered(Face::Ui, 12.0 * self.scale, false, "图片", r.right - r.left, r.bottom - r.top)?;
                c.text(&l, r.left, r.top, rgb(TEXT3));
            }
        }
        c.stroke_rect(inner, rgb(if img.is_some() { GRID } else { TEXT3 }), line, Stroke::Plain);
        Some(())
    }

    /// 标题栏的图标按钮（复制、关闭）、原文旁的复制原文；底栏的文字按钮（主按钮紫底白字，其余白底），样子和操作栏一样
    fn draw_button(&self, c: &Canvas, b: Btn, r: &Rect) -> Option<()> {
        let s = self.scale;
        let rf = rectf(r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
        let st = ui::state(self.enabled(b), false, self.hot == Some(b), self.down == Some(b));
        if let Some(&(_, label, primary)) = self.foot_buttons().iter().find(|f| f.0 == b) {
            let face = if primary { ui::primary(c, rf, st, s) } else { ui::secondary(c, rf, st, s) };
            let label = Text::new(Face::Ui, 13.0 * s, true, label)?;
            c.text(&label, (face.left + face.right - label.width()) / 2.0, (face.top + face.bottom - label.height()) / 2.0, rgb(if primary { 0xFFFFFF } else { TEXT }));
            return Some(());
        }
        let (dy, fg) = ui::button_bg(c, rf, st, s);
        let name = match b {
            Btn::Close => "close",
            Btn::Copy if self.copied => "check",
            _ => "copy",
        };
        let size = if b == Btn::CopyOrig { 16.0 } else { 18.0 } * s;
        ui::icon(c, name, (rf.left + rf.right - size) / 2.0, (rf.top + rf.bottom - size) / 2.0 + dy, size, fg);
        Some(())
    }
}

impl Handler for Card {
    fn event(&mut self, w: Win, e: Event) {
        match e {
            Event::Down { p, button: Button::Left, mods, .. } => {
                self.tip = None;
                w.kill_timer(T_TIP);
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
                    // 换了按钮：提示先收起，停 400ms 再出来
                    (self.hot, self.tip) = (hot, None);
                    w.kill_timer(T_TIP);
                    if hot.is_some_and(|b| self.tip_text(b).is_some()) && self.down.is_none() {
                        w.timer(T_TIP, 400);
                    }
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
                self.tip = None;
                if self.hot.take().is_some() {
                    w.invalidate_all();
                }
            }
            Event::Up { button: Button::Left, .. } => {
                let clicked = self.down.take().filter(|b| self.hot == Some(*b));
                self.selecting = false;
                w.invalidate_all();
                match clicked {
                    Some(Btn::Tab(t)) => self.switch_tab(w, t),
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
                    Some(Btn::Settings) => Win::later(crate::settings_ui::show_service),
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
                    if let Some(part) = [self.sel_part, Part::Main, Part::Orig].into_iter().find(|p| self.has(*p)) {
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
            Event::Timer(T_TIP) => {
                w.kill_timer(T_TIP);
                self.tip = self.hot;
                w.invalidate_all();
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
                self.at = Point { x: r.left, y: r.top };
                w.set_rect(Rect { left: r.left, top: r.top, right: r.left + self.width(), bottom: r.top + self.height() });
                w.invalidate_all();
            }
            _ => {}
        }
    }

    fn paint(&mut self, _w: Win, c: &Canvas) {
        c.clear(Color::default());
        self.draw(c, self.body_w() as f32, (self.height() - self.shadow()) as f32);
    }

    fn cursor(&mut self, _w: Win, p: Point) -> Cursor {
        if self.selecting || (self.btn_at(p).is_none() && self.in_text(p).is_some()) { Cursor::IBeam } else { Cursor::Arrow }
    }

    /// 离边 6 以内拖边框改大小；标题栏空白处按住拖动
    fn hit(&mut self, _w: Win, p: Point) -> Hit {
        // 按卡片本体算边；右边、下边的阴影也算在边上
        if let Some(e) = edge_at(p, self.body_w(), self.height() - self.shadow(), self.pxi(EDGE)) {
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
        let w = create(false, Body::Loading, anchor, mon).unwrap();
        let c = w.peek::<Card>();
        // 右边往右拉 dx，或者下边往下拉 dy
        let drag = |edge: Edge, dx: i32, dy: i32| {
            let mut r = w.rect();
            (r.right, r.bottom) = (r.right + dx, r.bottom + dy);
            let r = w.with(|c: &mut Card| c.sizing(w, edge, r)).unwrap();
            w.set_rect(r);
        };
        w.with(|c: &mut Card| c.arrive(w, Outcome::Text { text: "一段会自动折行的文字，".repeat(20), took: "0.1 秒".into(), images: Vec::new() }));
        let (w0, content0) = (size(w).0, c.content_h);
        drag(Edge::Right, 300, 0);
        assert!(size(w).0 == w0 + 300 && c.user_w.is_some() && c.user_h.is_none(), "{:?}", size(w));
        assert!(c.content_h < content0, "拖宽以后没有重新折行");
        assert_eq!(size(w), (c.width(), c.height()));
        // 往上拖到比最小还矮：停在标题栏 + 底栏 + 3 行
        drag(Edge::Bottom, 0, -2000);
        assert_eq!(size(w).1, c.pxi(HEAD + FOOT + 3.0 * LINE) + c.shadow());
        drag(Edge::Bottom, 0, 200);
        let fixed = size(w);
        assert!(c.user_h.is_some());
        w.with(|c: &mut Card| c.arrive(w, Outcome::Text { text: "短".into(), took: "0.1 秒".into(), images: Vec::new() }));
        assert_eq!(size(w), fixed);
        // 右边框上是改大小，标题栏空白处是拖动
        let (cw, _) = size(w);
        assert_eq!(c.hit(w, Point { x: cw - 2, y: 100 }), Hit::Edge(Edge::Right));
        assert_eq!(c.hit(w, Point { x: 200, y: 20 }), Hit::Caption);
        w.close();
    }

    /// 拖高的翻译卡片：内容填不满时原文开关行贴在正文区最底下，展开的原文跟着它；内容放得满时还接在译文后面
    #[test]
    fn orig_row_sticks_to_bottom() {
        let tr = |open: bool| Body::Translate(Tr { orig: doc::parse("Hello world"), src: None, open, state: TrState::Done(doc::parse("你好世界")) });
        let mut c = Card::new(1.0, Rect { left: 0, top: 0, right: 1920, bottom: 1080 }, true, tr(false));
        c.relayout();
        let natural = c.row_y;
        // 没拖过高度：卡片跟着内容，开关行接在译文后面
        assert_eq!(c.row_y + c.px(ORIG_ROW), c.content_h);
        for open in [false, true] {
            c.body = tr(open);
            c.user_h = Some(600.0);
            c.relayout();
            let bottom = c.body_h() as f32;
            assert!(c.row_y > natural && (c.content_h - bottom).abs() < 1.0, "开关行没贴底：{} / {bottom}", c.row_y);
            assert_eq!(c.max_scroll(), 0.0);
            if open {
                let orig = c.blocks.iter().find(|b| b.part == Part::Orig).unwrap();
                assert!(orig.y > c.row_y, "展开的原文该在开关行下面");
            } else {
                assert!((c.row_y + c.px(ORIG_ROW) - bottom).abs() < 1.0);
            }
        }
        // 译文长、卡片矮：放不下就照常接在后面、滚动
        c.body = Body::Translate(Tr { orig: doc::parse("Hello"), src: None, open: false, state: TrState::Done(doc::parse(&"一段会自动折行的文字，".repeat(20))) });
        c.user_h = None;
        c.relayout();
        let natural = c.row_y;
        c.user_h = Some(HEAD + FOOT + 3.0 * LINE);
        c.relayout();
        assert!(c.row_y == natural && c.max_scroll() > 0.0);
    }

    #[test]
    fn columns_share_width() {
        assert_eq!(fit_cols(&[50.0, 100.0], 300.0), [50.0, 100.0]);
        // 放不下：不比平均宽的列照旧，宽列分剩下的
        assert_eq!(fit_cols(&[50.0, 300.0, 100.0], 300.0), [50.0, 150.0, 100.0]);
        assert_eq!(fit_cols(&[400.0, 200.0], 300.0), [200.0, 100.0]);
    }

    const TABLE: &str = "价格表\n\n| 名称 | 价格 | 说明 |\n|---|---|---|\n| 苹果 | 3 | 一种很常见的水果，又甜又脆，放久了会变面，适合直接吃或者做成果汁 |\n| 香蕉 | 2 | 黄色 |\n\n![图片](#0)\n\n```\nfn main() {\n    println!(\"hi\");\n}\n```\n![图片](#9)";

    /// 表格每格一段；点在格里选到那格的字；全选复制出 Tab 分格的纯文本；宽表格让卡片加宽、最宽到选区；序号对不上的图片画占位框
    #[test]
    fn table_layout_and_copy() {
        let mut c = Card::new(1.0, WORK, false, Body::Text(doc::parse(TABLE)));
        c.images = vec![Image { w: 200, h: 80, px: vec![200; 200 * 80 * 4] }];
        c.src = r(100, 100, 700, 600);
        c.relayout();
        assert!(c.fit_w > WIDTH && c.fit_w <= 600.0 + 2.0 * PAD, "{}", c.fit_w);
        // 标题 + 9 格 + 代码
        assert_eq!(c.blocks.iter().filter(|b| b.part == Part::Main).count(), 11);
        let imgs: Vec<_> = c.decos.iter().filter_map(|d| if let DecoKind::Img(i) = d.kind { Some((i, d.r.right - d.r.left)) } else { None }).collect();
        assert_eq!(imgs, [(Some(0), 200.0), (None, 120.0)]);
        // 表格没超出正文宽
        let right = c.decos.iter().map(|d| d.r.right).fold(0.0, f32::max);
        assert!(right <= c.px(c.w_dip() - PAD) + 0.5, "{right}");

        let plain: Vec<u16> = c.part_text(Part::Main).unwrap().encode_utf16().collect();
        let cell = |ch: char| c.blocks.iter().find(|b| b.len == 1 && plain[b.start as usize] == ch as u16).unwrap();
        let top = c.body_rect().top as f32 - c.scroll;
        let two = cell('2');
        assert_eq!(c.pos_at(Point { x: (two.x + 1.0) as i32, y: (top + two.y + 5.0) as i32 }), two.start);
        // 同一行、表格右边很远：落到最后一格「黄色」的末尾
        let yellow = c.blocks.iter().find(|b| b.y == two.y && b.x > two.x).unwrap();
        assert_eq!(c.pos_at(Point { x: 5000, y: (top + two.y + 5.0) as i32 }), yellow.start + yellow.len);

        (c.anchor, c.caret) = (0, plain.len() as u32);
        assert_eq!(
            c.selected(),
            "价格表\n名称\t价格\t说明\n苹果\t3\t一种很常见的水果，又甜又脆，放久了会变面，适合直接吃或者做成果汁\n香蕉\t2\t黄色\nfn main() {\n    println!(\"hi\");\n}"
        );
        assert_eq!(c.part_text(Part::Main).map(|t| t.lines().count()), Some(7));
    }

    /// 把带表格、图片、代码的取字卡片和译文卡片（展开原文）画成 PNG，看排版：
    /// cargo test structured_card -- --ignored，图在 target/test-tmp/card-structure
    #[test]
    #[ignore]
    fn structured_card() {
        use crate::plat::Bitmap;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("card-structure");
        std::fs::create_dir_all(&dir).unwrap();
        let photo = Image { w: 240, h: 90, px: (0..240 * 90).flat_map(|i| [(i % 240) as u8, 120, (i / 240 * 2) as u8, 255]).collect() };
        let en = "Price list\n\n| Name | Price | Notes |\n|---|---|---|\n| Apple | 3 | A very common fruit, sweet and crisp |\n| Banana | 2 | Yellow |\n\n![Image](#0)";
        for (s, name, body) in [
            (1.0, "ocr-100.png", Body::Text(doc::parse(TABLE))),
            (1.5, "ocr-150.png", Body::Text(doc::parse(TABLE))),
            (1.0, "translate.png", Body::Translate(Tr { orig: doc::parse(en), src: Some(translate::Lang::En), open: true, state: TrState::Done(doc::parse(TABLE)) })),
        ] {
            let tr = matches!(body, Body::Translate(_));
            let mut c = Card::new(s, WORK, tr, body);
            c.images = vec![Image { w: photo.w, h: photo.h, px: photo.px.clone() }];
            c.src = r(100, 100, 900, 700);
            // 译文卡片拖高，原文整个露出来
            c.user_h = tr.then_some(900.0);
            c.relayout();
            let bmp = Bitmap::new(c.width(), c.height()).unwrap();
            {
                let canvas = Canvas::new(&bmp, bmp.full(), false);
                canvas.clear(rgb(0x808080));
                c.draw(&canvas, c.body_w() as f32, (c.height() - c.shadow()) as f32);
            }
            std::fs::write(dir.join(name), plat::encode(&bmp.crop(&bmp.full()), false).unwrap()).unwrap();
        }
        println!("卡片截图在 {}", dir.display());
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
            // 和窗口里一样：本体不含阴影，画在灰底上看得出阴影。画完（canvas 释放）才能读像素
            {
                let canvas = Canvas::new(&bmp, bmp.full(), false);
                canvas.clear(rgb(0x808080));
                c.draw(&canvas, c.body_w() as f32, (c.height() - c.shadow()) as f32);
            }
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
        let Body::Translate(tr) = &c.body else { panic!() };
        assert_eq!(sent["messages"][1]["content"].as_str(), Some(tr.orig.src.as_str()));
        assert_eq!(c.pair, "英 → 中");
        assert!(c.meta.ends_with(" 秒") && c.foot_buttons().is_empty(), "{:?}", c.meta);
        // 默认折叠：只有译文
        assert!(!c.has(Part::Orig) && c.has(Part::Main));
        snap(w, "3-done.png");

        // 展开原文，下一张卡片也记得；再折叠回来
        click(w, Btn::Toggle);
        assert!(ORIG_OPEN.load(Ordering::Relaxed) && w.peek::<Card>().has(Part::Orig));
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

        // 带表格和图片坐标的结果：图片从截图里裁出来，卡片加宽后还在选区旁边；翻译时把结构原样发过去
        let table = r#"{"choices":[{"message":{"content":"| 名称 | 价格 |\n|---|---|\n| 苹果 | 3 |\n\n![图片](100, 100, 500, 600)"}}]}"#;
        let (vl, _req) = mock(Some(http("200 OK", table)));
        let (good, req) = mock(Some(http("200 OK", r#"{"choices":[{"message":{"content":"| Name | Price |\n|---|---|\n| Apple | 3 |\n\n![图片](#0)"}}]}"#)));
        use_service(&good, "m2", &vl);
        translate(img(), anchor, mon);
        let w = card();
        wait(w, |c| matches!(c.body, Body::Translate(Tr { state: TrState::Done(..), .. })));
        let c = w.peek::<Card>();
        assert_eq!(c.images.iter().map(|m| (m.w, m.h)).collect::<Vec<_>>(), [(224, 100)]);
        let sent: serde_json::Value = serde_json::from_str(req.recv().unwrap().split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(sent["messages"][1]["content"], "| 名称 | 价格 |\n|---|---|\n| 苹果 | 3 |\n\n![图片](#0)");
        assert!(c.decos.iter().any(|d| matches!(d.kind, DecoKind::Img(Some(0)))));
        assert_eq!(c.part_text(Part::Main), Some("Name\tPrice\nApple\t3"));
        assert!(w.rect().left >= anchor.right, "卡片盖住了选区：{:?}", w.rect());
        snap(w, "7-structured.png");
        w.close();
        println!("卡片截图在 {}", dir.display());
    }
}
