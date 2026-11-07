//! 设置窗口：标准窗口 + 自绘控件（导航、开关、色块、取色器、分段、下拉、按钮）。
//! 只有输入框和快捷键框用系统控件，去掉系统边框、边框自己画。「确定」时统一保存并立即生效，
//! 快捷键注册失败就恢复原键、停在窗口里提示。尺寸按设计稿（DIP）乘窗口所在显示器的缩放比例。

use crate::plat::{self, Button, Canvas, Cursor, Event, Face, FieldKind, Handler, Key, MenuItem, Point, Rect, RectF, Spec, Stroke, Text, Win};
use crate::hotkeys;
use crate::settings::{self, FPS_CHOICES, Format, Hotkey, Settings};
use crate::toast::{self, Kind};
use crate::translate::{self, Lang};
use crate::ui::{self, rectf, rgb, rgba};
use std::time::{Duration, Instant};

const T_SPIN: usize = 1;
const FRAME_MS: u32 = 16;

// 设计稿尺寸（DIP）
const WIN_W: f32 = 640.0;
/// 内容区高度：放得下「截图」页展开取色器的样子
const MAIN_H: f32 = 428.0;
const FOOT_H: f32 = 53.0;
const NAV_W: f32 = 148.0;
/// 标签列、控件列的左边，内容区右边
const LX: f32 = 172.0;
const CX: f32 = 260.0;
const RIGHT: f32 = 616.0;
/// 第一行的顶
const ROW0: f32 = 58.0;
/// 色块那一行、取色器方框的顶
const SW_Y: f32 = 160.0;
const PICK_Y: f32 = 190.0;

const TEXT: u32 = 0x1A1A1A;
const HINT: u32 = 0x5F5A66;
const LINE: u32 = 0xCFCCD3;
const ACCENT: u32 = 0x9C44EC;
const ACCENT_HOVER: u32 = 0xA95CF0;
const ACCENT_PRESS: u32 = 0x8733D4;
const ACCENT_SOFT: u32 = 0xF3E9FD;
const ERR: u32 = 0xC42B1C;
const OK: u32 = 0x107C10;
/// 设置窗口的字一律 Segoe UI（不用界面字的 Variable 版），和系统输入框里的字一样
const FONT: Face = Face::Segoe;

const PAGES: [&str; 5] = ["截图", "录屏", "保存", "文字识别", "翻译"];
const SHOT_PAGE: usize = 0;
const REC_PAGE: usize = 1;
const SAVE_PAGE: usize = 2;
const OCR_PAGE: usize = 3;
const TR_PAGE: usize = 4;
const PRESETS: [(&str, u32); 7] =
    [("紫", 0x9C44EC), ("品红", 0xE0389A), ("红", 0xE5383B), ("橙", 0xF27A1A), ("黄", 0xF5B700), ("绿", 0x23A55A), ("白", 0xFFFFFF)];
const TARGETS: [(Lang, &str); 3] = [(Lang::Zh, "简体中文"), (Lang::En, "English"), (Lang::Ja, "日本語")];

/// 自绘控件
#[derive(Clone, Copy, PartialEq, Debug)]
enum Ctl {
    Nav(usize),
    Toggle,
    /// 0–6 是预设，7 是「自定义」
    Swatch(usize),
    Sv,
    Hue,
    Browse,
    Format(usize),
    /// 录屏帧率，FPS_CHOICES 里的第几个
    Fps(usize),
    /// 录屏页的「动图边框」开关
    GifFrame,
    Target,
    /// 模型框右边的「选择…」（翻译页和文字识别页各一个，用的是当前页的框）
    Models,
    Test,
    Ok,
    Cancel,
}

/// 系统控件：快捷键框和输入框
#[derive(Clone, Copy, PartialEq, Debug)]
enum Child {
    Hotkey,
    RecHotkey,
    SaveDir,
    Hex,
    Endpoint,
    Model,
    Key,
    /// 取字的服务，留空就用翻译的
    OcrEndpoint,
    OcrModel,
    OcrKey,
}

const CHILDREN: [Child; 10] = [
    Child::Hotkey,
    Child::RecHotkey,
    Child::SaveDir,
    Child::Hex,
    Child::Endpoint,
    Child::Model,
    Child::Key,
    Child::OcrEndpoint,
    Child::OcrModel,
    Child::OcrKey,
];

impl Child {
    fn page(self) -> usize {
        match self {
            Child::Hotkey | Child::Hex => SHOT_PAGE,
            Child::RecHotkey => REC_PAGE,
            Child::SaveDir => SAVE_PAGE,
            Child::OcrEndpoint | Child::OcrModel | Child::OcrKey => OCR_PAGE,
            _ => TR_PAGE,
        }
    }
}

/// 键盘焦点：在系统控件上，或者在某个自绘控件上（这时系统焦点在窗口本身）
#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Ctl(Ctl),
    Child(Child),
}

/// 测试连接那一行：测试连接和拉模型列表共用
enum Test {
    Idle,
    /// 转圈，带说明
    Busy(&'static str),
    Done(bool, String),
}

struct Ui {
    w: Win,
    scale: f32,
    page: usize,
    hot: Option<Ctl>,
    down: Option<Ctl>,
    /// 拖着取色方块或色相条
    drag: Option<Ctl>,
    focus: Option<Ctl>,
    /// 最近一次操作用的是键盘：只有这时才画自绘控件的焦点框
    kbd: bool,
    // 打开时的设置，和改到一半的值
    orig: Settings,
    autostart0: bool,
    autostart: bool,
    color: u32,
    hsv: (f32, f32, f32),
    picker: bool,
    save_dir: String,
    format: Format,
    fps: u32,
    gif_frame: bool,
    target: Lang,
    key0: String,
    ocr_key0: String,
    /// 哪个快捷键框有问题、原因
    hk_bad: Option<(Child, String)>,
    test: Test,
    test_id: u32,
    /// 「选择…」最近一次拉到的模型
    models: Vec<String>,
    angle: f32,
    /// 程序自己改色值输入框的文字时不回读
    syncing: bool,
}

/// 托盘菜单「设置」、快捷键冲突通知；已经开着就提到前面。`hotkey` 为 true 时直接去改快捷键。
pub fn show(hotkey: bool) {
    if let Some(&w) = Win::all(plat::Kind::Settings).first() {
        if hotkey {
            w.with(|ui: &mut Ui| {
                ui.set_page(SHOT_PAGE);
                w.focus_field(Some(Child::Hotkey as usize));
            });
        }
        w.foreground();
        return;
    }
    let mon = plat::monitor_at(plat::cursor_pos());
    let orig = settings::get();
    let autostart = plat::autostart();
    let s = mon.scale();
    let ui = Ui {
        w: Win::default(),
        scale: s,
        page: 0,
        hot: None,
        down: None,
        drag: None,
        focus: None,
        kbd: false,
        autostart0: autostart,
        autostart,
        color: orig.sel_color,
        hsv: hsv_of(orig.sel_color),
        picker: !PRESETS.iter().any(|p| p.1 == orig.sel_color),
        save_dir: orig.save_dir.clone(),
        format: orig.save_format,
        fps: orig.record_fps,
        gif_frame: orig.gif_frame,
        target: orig.target,
        key0: plat::api_key(settings::TRANSLATE_KEY),
        ocr_key0: plat::api_key(settings::OCR_KEY),
        hk_bad: None,
        test: Test::Idle,
        test_id: 0,
        models: Vec::new(),
        angle: 0.0,
        syncing: false,
        orig,
    };
    // 内容区在工作区里居中，标题栏另加
    let (w, h) = ((WIN_W * s).round() as i32, ((MAIN_H + FOOT_H) * s).round() as i32);
    let work = mon.work;
    let (x, y) = ((work.left + work.right - w) / 2, ((work.top + work.bottom - h) / 2).max(work.top));
    let rect = Rect { left: x, top: y, right: x + w, bottom: y + h };
    let Some(win) = Win::open(Spec { kind: plat::Kind::Settings, rect, title: "Jie 设置", owner: None }, ui) else { return };
    win.with(|ui: &mut Ui| {
        ui.w = win;
        ui.make_kids();
    });
    win.show();
    if hotkey {
        win.focus_field(Some(Child::Hotkey as usize));
    }
}

/// 文字卡片的「设置」：打开设置并翻到翻译页，`ocr` 为 true 时翻到文字识别页
pub fn show_service(ocr: bool) {
    show(false);
    if let Some(&w) = Win::all(plat::Kind::Settings).first() {
        w.with(|ui: &mut Ui| ui.set_page(if ocr { OCR_PAGE } else { PAGES.len() - 1 }));
    }
}

fn contains(r: &Rect, p: Point) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

fn rf(r: &Rect) -> RectF {
    rectf(r.left as f32, r.top as f32, r.right as f32, r.bottom as f32)
}

/// 0xRRGGBB → (色相 0–360, 饱和度 0–1, 明度 0–1)
fn hsv_of(c: u32) -> (f32, f32, f32) {
    let ch = |s: u32| ((c >> s) & 0xFF) as f32 / 255.0;
    let (r, g, b) = (ch(16), ch(8), ch(0));
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let d = mx - mn;
    let mut h = 0.0;
    if d > 0.0 {
        h = 60.0
            * if mx == r {
                ((g - b) / d) % 6.0
            } else if mx == g {
                (b - r) / d + 2.0
            } else {
                (r - g) / d + 4.0
            };
        if h < 0.0 {
            h += 360.0;
        }
    }
    (h, if mx > 0.0 { d / mx } else { 0.0 }, mx)
}

fn hsv_rgb((h, s, v): (f32, f32, f32)) -> u32 {
    let f = |n: f32| {
        let k = (n + h / 60.0) % 6.0;
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    let to = |x: f32| (x * 255.0).round().clamp(0.0, 255.0) as u32;
    (to(f(5.0)) << 16) | (to(f(3.0)) << 8) | to(f(1.0))
}

/// 「#9C44EC」或「9C44EC」
fn parse_hex(s: &str) -> Option<u32> {
    let s = s.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    (s.len() == 6 && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| u32::from_str_radix(s, 16).ok()).flatten()
}

fn hex(c: u32) -> String {
    format!("#{c:06X}")
}

impl Ui {
    /// DIP → 物理像素（取整）
    fn px(&self, d: f32) -> f32 {
        (d * self.scale).round()
    }

    fn rect(&self, l: f32, t: f32, r: f32, b: f32) -> Rect {
        let p = |v: f32| self.px(v) as i32;
        Rect { left: p(l), top: p(t), right: p(r), bottom: p(b) }
    }

    /// 文字宽度（DIP）
    fn tw(&self, text: &str, size: f32, bold: bool) -> f32 {
        Text::new(FONT, size * self.scale, bold, text).map_or(0.0, |l| l.width()) / self.scale
    }

    /// 普通按钮宽：文字 + 左右各 12，至少 72
    fn btn_w(&self, text: &str, bold: bool) -> f32 {
        (self.tw(text, 12.0, bold) + 24.0).max(72.0).ceil()
    }

    fn swatch(&self) -> usize {
        PRESETS.iter().position(|p| p.1 == self.color).unwrap_or(7)
    }

    fn format_index(&self) -> usize {
        usize::from(self.format == Format::Jpeg)
    }

    fn target_label(&self) -> &'static str {
        TARGETS.iter().find(|t| t.0 == self.target).map_or("简体中文", |t| t.1)
    }

    fn drop_w(&self, text: &str) -> f32 {
        // 左 10 + 文字 + 间距 12 + 箭头 14 + 右 6，至少 200
        (self.tw(text, 12.0, false) + 42.0).clamp(200.0, RIGHT - CX).ceil()
    }

    fn target_hint(&self) -> &'static str {
        if self.target == Lang::En { "原文已是英文时改译成中文" } else { "原文已是这种语言时改译成英文" }
    }

    /// 「译成」后面的提示在下拉框右边放不下，就折到下一行（设计稿的 flex-wrap）
    fn hint_wraps(&self) -> bool {
        CX + self.drop_w(self.target_label()) + 8.0 + self.tw(self.target_hint(), 12.0, false) > RIGHT
    }

    /// 分段按钮里一段的宽：文字 + 左右各 16
    fn seg_w(&self, t: &str) -> f32 {
        (self.tw(t, 12.0, true) + 32.0).ceil()
    }

    fn fps_index(&self) -> usize {
        FPS_CHOICES.iter().position(|f| *f == self.fps).unwrap_or(1)
    }

    /// 输入框的外框（DIP）
    fn field_dip(&self, c: Child) -> (f32, f32, f32, f32) {
        match c {
            Child::Hotkey | Child::RecHotkey => (CX, ROW0, RIGHT, ROW0 + 28.0),
            Child::SaveDir => (CX, ROW0, RIGHT - 8.0 - self.btn_w("浏览…", false), ROW0 + 28.0),
            Child::Hex => (CX + 47.0, PICK_Y + 159.0, CX + 143.0, PICK_Y + 187.0),
            Child::Endpoint => (CX, 58.0, RIGHT, 86.0),
            Child::Model => (CX, 98.0, RIGHT - 8.0 - self.btn_w("选择…", false), 126.0),
            Child::Key => (CX, 138.0, RIGHT, 166.0),
            Child::OcrEndpoint => (CX, 58.0, RIGHT, 86.0),
            Child::OcrModel => (CX, 98.0, RIGHT - 8.0 - self.btn_w("选择…", false), 126.0),
            Child::OcrKey => (CX, 138.0, RIGHT, 166.0),
        }
    }

    fn field(&self, c: Child) -> Rect {
        let (l, t, r, b) = self.field_dip(c);
        self.rect(l, t, r, b)
    }

    fn visible(&self, c: Child) -> bool {
        c.page() == self.page && (c != Child::Hex || self.picker)
    }

    /// 当前页能点的自绘控件和位置
    fn ctls(&self) -> Vec<(Ctl, Rect)> {
        let mut v: Vec<(Ctl, Rect)> =
            (0..PAGES.len()).map(|i| (Ctl::Nav(i), self.rect(8.0, 12.0 + 36.0 * i as f32, NAV_W - 8.0, 46.0 + 36.0 * i as f32))).collect();
        match self.page {
            SHOT_PAGE => {
                v.push((Ctl::Toggle, self.rect(CX, 124.0, CX + 40.0, 144.0)));
                for i in 0..8 {
                    let x = CX + 30.0 * i as f32;
                    v.push((Ctl::Swatch(i), self.rect(x, SW_Y, x + 24.0, SW_Y + 24.0)));
                }
                if self.picker {
                    v.push((Ctl::Sv, self.rect(CX + 11.0, PICK_Y + 11.0, CX + 211.0, PICK_Y + 131.0)));
                    v.push((Ctl::Hue, self.rect(CX + 11.0, PICK_Y + 139.0, CX + 211.0, PICK_Y + 151.0)));
                }
            }
            REC_PAGE => {
                let fw = self.seg_w("30");
                for i in 0..FPS_CHOICES.len() {
                    let x = CX + fw * i as f32;
                    v.push((Ctl::Fps(i), self.rect(x, 126.0, x + fw, 154.0)));
                }
                v.push((Ctl::GifFrame, self.rect(CX, 216.0, CX + 40.0, 236.0)));
            }
            SAVE_PAGE => {
                v.push((Ctl::Browse, self.rect(RIGHT - self.btn_w("浏览…", false), ROW0, RIGHT, ROW0 + 28.0)));
                let w0 = self.seg_w("PNG");
                v.push((Ctl::Format(0), self.rect(CX, 126.0, CX + w0, 154.0)));
                v.push((Ctl::Format(1), self.rect(CX + w0, 126.0, CX + w0 + self.seg_w("JPEG"), 154.0)));
            }
            OCR_PAGE => {
                v.push((Ctl::Models, self.rect(RIGHT - self.btn_w("选择…", false), 98.0, RIGHT, 126.0)));
                v.push((Ctl::Test, self.rect(CX, 198.0, CX + self.btn_w("测试连接", false), 226.0)));
            }
            _ => {
                v.push((Ctl::Models, self.rect(RIGHT - self.btn_w("选择…", false), 98.0, RIGHT, 126.0)));
                v.push((Ctl::Target, self.rect(CX, 178.0, CX + self.drop_w(self.target_label()), 206.0)));
                let y = if self.hint_wraps() { 240.0 } else { 218.0 };
                v.push((Ctl::Test, self.rect(CX, y, CX + self.btn_w("测试连接", false), y + 28.0)));
            }
        }
        let y = MAIN_H + 13.0;
        let cw = self.btn_w("取消", false);
        let ow = self.btn_w("确定", true);
        v.push((Ctl::Ok, self.rect(WIN_W - 16.0 - cw - 8.0 - ow, y, WIN_W - 16.0 - cw - 8.0, y + 28.0)));
        v.push((Ctl::Cancel, self.rect(WIN_W - 16.0 - cw, y, WIN_W - 16.0, y + 28.0)));
        v
    }

    fn ctl_rect(&self, c: Ctl) -> Option<Rect> {
        self.ctls().into_iter().find(|(k, _)| *k == c).map(|(_, r)| r)
    }

    fn ctl_at(&self, p: Point) -> Option<Ctl> {
        self.ctls().into_iter().find(|(_, r)| contains(r, p)).map(|(c, _)| c)
    }

    fn text(&self, c: Child) -> String {
        self.w.field_text(c as usize)
    }

    fn set_text(&self, c: Child, t: &str) {
        self.w.set_field_text(c as usize, t);
    }

    /// 快捷键框（截图的或录屏的）里的组合键；没按或者不带 Ctrl、Alt 时为 None
    fn read_hotkey(&self, c: Child) -> Option<Hotkey> {
        let (mods, vk) = self.w.hotkey_field(c as usize);
        let hk = Hotkey { mods, vk };
        hk.valid().then_some(hk)
    }

    /// 建系统控件、填好打开时的值
    fn make_kids(&mut self) {
        let w = self.w;
        let key_cue = format!("保存在{}，不写进配置文件", plat::KEY_STORE);
        for c in CHILDREN {
            let (kind, cue) = match c {
                Child::Hotkey | Child::RecHotkey => (FieldKind::Hotkey, ""),
                Child::SaveDir => (FieldKind::ReadOnly, ""),
                Child::Hex => (FieldKind::Hex, ""),
                Child::Endpoint => (FieldKind::Text, "例如 https://api.deepseek.com"),
                Child::Model => (FieldKind::Text, "例如 deepseek-flash"),
                Child::Key => (FieldKind::Password, key_cue.as_str()),
                Child::OcrEndpoint => (FieldKind::Text, "留空就用翻译页的服务地址和 API Key"),
                Child::OcrModel => (FieldKind::Text, "留空就用翻译页的模型（要能看图）"),
                Child::OcrKey => (FieldKind::Password, "服务地址留空时用翻译页的 Key"),
            };
            w.field(c as usize, kind, cue);
        }
        for (c, hk) in [(Child::Hotkey, self.orig.hotkey), (Child::RecHotkey, self.orig.record_hotkey)] {
            w.set_hotkey_field(c as usize, hk.mods, hk.vk);
        }
        let dir = if self.save_dir.is_empty() { plat::pictures_dir() } else { self.save_dir.clone() };
        self.set_text(Child::SaveDir, &dir);
        // 没配过服务就填好 DeepSeek，只差 API Key
        let (mut endpoint, mut model) = (self.orig.endpoint.clone(), self.orig.model.clone());
        if endpoint.is_empty() {
            endpoint = translate::DEEPSEEK.into();
            if model.is_empty() {
                model = translate::DEEPSEEK_MODEL.into();
            }
        }
        self.set_text(Child::Endpoint, &endpoint);
        self.set_text(Child::Model, &model);
        self.set_text(Child::Key, &self.key0);
        self.set_text(Child::OcrEndpoint, &self.orig.ocr_endpoint);
        self.set_text(Child::OcrModel, &self.orig.ocr_model);
        self.set_text(Child::OcrKey, &self.ocr_key0);
        self.sync_hex();
        self.place_kids();
        self.show_kids();
    }

    /// 按当前缩放比例摆系统控件
    fn place_kids(&self) {
        for c in CHILDREN {
            self.w.place_field(c as usize, self.field(c), self.scale);
        }
    }

    fn show_kids(&self) {
        for c in CHILDREN {
            self.w.show_field(c as usize, self.visible(c));
        }
        self.w.invalidate_all();
    }

    fn set_page(&mut self, i: usize) {
        if i != self.page {
            self.reset_test();
        }
        self.page = i;
        self.show_kids();
    }

    /// 测试连接那一行回到空白；还在等的结果回来时编号对不上，丢掉
    fn reset_test(&mut self) {
        self.test_id += 1;
        self.test = Test::Idle;
        self.w.kill_timer(T_SPIN);
    }

    fn set_picker(&mut self, on: bool) {
        self.picker = on;
        self.show_kids();
    }

    fn sync_hex(&mut self) {
        self.syncing = true;
        self.set_text(Child::Hex, &hex(self.color));
        self.syncing = false;
    }

    fn set_color(&mut self, c: u32) {
        self.color = c;
        self.hsv = hsv_of(c);
        self.sync_hex();
    }

    /// 在取色方块或色相条上按住、拖动
    fn pick(&mut self, c: Ctl, p: Point) {
        let Some(r) = self.ctl_rect(c) else { return };
        let fx = ((p.x - r.left) as f32 / (r.right - r.left) as f32).clamp(0.0, 1.0);
        let fy = ((p.y - r.top) as f32 / (r.bottom - r.top) as f32).clamp(0.0, 1.0);
        match c {
            Ctl::Sv => (self.hsv.1, self.hsv.2) = (fx, 1.0 - fy),
            _ => self.hsv.0 = (fx * 360.0).min(359.9),
        }
        self.color = hsv_rgb(self.hsv);
        self.sync_hex();
        self.w.invalidate_all();
    }

    /// 自绘控件被点了（或用键盘按了空格）
    fn click(&mut self, c: Ctl) {
        match c {
            Ctl::Nav(i) => self.set_page(i),
            Ctl::Toggle => self.autostart = !self.autostart,
            Ctl::Swatch(i) if i < 7 => {
                self.set_color(PRESETS[i].1);
                self.set_picker(false);
            }
            Ctl::Swatch(_) => self.set_picker(!self.picker),
            Ctl::Format(i) => self.format = if i == 0 { Format::Png } else { Format::Jpeg },
            Ctl::Fps(i) => self.fps = FPS_CHOICES[i],
            Ctl::GifFrame => self.gif_frame = !self.gif_frame,
            Ctl::Test => self.start_test(),
            Ctl::Models => self.start_models(),
            Ctl::Browse => self.browse(),
            Ctl::Target => self.menu(c),
            Ctl::Ok => return self.apply(),
            Ctl::Cancel => return self.w.close(),
            Ctl::Sv | Ctl::Hue => {}
        }
        self.w.invalidate_all();
    }

    /// Tab 顺序：导航（当前页）、本页的控件、确定、取消；色块、分段、导航各算一站，站里用方向键
    fn order(&self) -> Vec<Focus> {
        let mut v = vec![Focus::Ctl(Ctl::Nav(self.page))];
        match self.page {
            SHOT_PAGE => {
                v.extend([Focus::Child(Child::Hotkey), Focus::Ctl(Ctl::Toggle), Focus::Ctl(Ctl::Swatch(self.swatch()))]);
                if self.picker {
                    v.push(Focus::Child(Child::Hex));
                }
            }
            REC_PAGE => v.extend([Focus::Child(Child::RecHotkey), Focus::Ctl(Ctl::Fps(self.fps_index())), Focus::Ctl(Ctl::GifFrame)]),
            SAVE_PAGE => v.extend([Focus::Ctl(Ctl::Browse), Focus::Ctl(Ctl::Format(self.format_index()))]),
            OCR_PAGE => v.extend([
                Focus::Child(Child::OcrEndpoint),
                Focus::Child(Child::OcrModel),
                Focus::Ctl(Ctl::Models),
                Focus::Child(Child::OcrKey),
                Focus::Ctl(Ctl::Test),
            ]),
            _ => v.extend([
                Focus::Child(Child::Endpoint),
                Focus::Child(Child::Model),
                Focus::Ctl(Ctl::Models),
                Focus::Child(Child::Key),
                Focus::Ctl(Ctl::Target),
                Focus::Ctl(Ctl::Test),
            ]),
        }
        v.extend([Focus::Ctl(Ctl::Ok), Focus::Ctl(Ctl::Cancel)]);
        v
    }

    fn current(&self) -> Option<Focus> {
        if let Some(id) = self.w.focused_field() {
            return CHILDREN.into_iter().find(|c| *c as usize == id).map(Focus::Child);
        }
        self.w.focused().then_some(self.focus.map(Focus::Ctl)).flatten()
    }

    fn focus_to(&mut self, f: Focus) {
        self.kbd = true;
        match f {
            Focus::Child(c) => {
                self.focus = None;
                self.w.focus_field(Some(c as usize));
            }
            Focus::Ctl(c) => {
                self.focus = Some(c);
                self.w.focus_field(None);
            }
        }
        self.w.invalidate_all();
    }

    fn tab(&mut self, back: bool) {
        let order = self.order();
        let same = |a: &Focus, b: &Focus| match (a, b) {
            (Focus::Ctl(x), Focus::Ctl(y)) => std::mem::discriminant(x) == std::mem::discriminant(y),
            _ => a == b,
        };
        let n = order.len();
        let next = match self.current().and_then(|c| order.iter().position(|o| same(o, &c))) {
            Some(i) if back => (i + n - 1) % n,
            Some(i) => (i + 1) % n,
            None if back => n - 1,
            None => 0,
        };
        self.focus_to(order[next]);
    }

    /// 键盘：`own` 为 true 表示焦点在窗口本身（自绘控件上），false 是从系统控件转过来的
    fn key(&mut self, k: Key, shift: bool, own: bool) {
        let f = if own { self.focus } else { None };
        match k {
            Key::Tab => self.tab(shift),
            Key::Esc => self.w.close(),
            Key::Enter => match f {
                Some(c @ (Ctl::Browse | Ctl::Test | Ctl::Models | Ctl::Cancel | Ctl::Target)) => self.click(c),
                _ => self.apply(),
            },
            Key::Space => {
                if let Some(c) = f {
                    self.kbd = true;
                    self.click(c);
                }
            }
            Key::Left | Key::Right | Key::Up | Key::Down => {
                let d: i32 = if matches!(k, Key::Left | Key::Up) { -1 } else { 1 };
                match f {
                    Some(Ctl::Nav(i)) => {
                        let p = (i as i32 + d).rem_euclid(PAGES.len() as i32) as usize;
                        self.set_page(p);
                        self.focus_to(Focus::Ctl(Ctl::Nav(p)));
                    }
                    Some(Ctl::Swatch(i)) => {
                        let j = (i as i32 + d).clamp(0, 7) as usize;
                        if j < 7 {
                            self.set_color(PRESETS[j].1);
                            self.set_picker(false);
                        }
                        self.focus_to(Focus::Ctl(Ctl::Swatch(j)));
                    }
                    Some(Ctl::Format(_)) => {
                        let j = usize::from(d > 0);
                        self.format = if j == 0 { Format::Png } else { Format::Jpeg };
                        self.focus_to(Focus::Ctl(Ctl::Format(j)));
                    }
                    Some(Ctl::Fps(i)) => {
                        let j = (i as i32 + d).clamp(0, FPS_CHOICES.len() as i32 - 1) as usize;
                        self.fps = FPS_CHOICES[j];
                        self.focus_to(Focus::Ctl(Ctl::Fps(j)));
                    }
                    Some(c @ Ctl::Target) if k == Key::Down => self.menu(c),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// 框里当前填的服务（还没保存）：翻译页是翻译服务，文字识别页是大模型取字实际会用的服务
    fn service(&self) -> translate::Config {
        let tr = translate::Config {
            endpoint: self.text(Child::Endpoint).trim().to_string(),
            model: self.text(Child::Model).trim().to_string(),
            api_key: self.text(Child::Key).trim().to_string(),
            target: self.target,
        };
        if self.page != OCR_PAGE {
            return tr;
        }
        let f = |c| self.text(c).trim().to_string();
        tr.overlay(&f(Child::OcrEndpoint), &f(Child::OcrModel), f(Child::OcrKey))
    }

    /// 当前页的模型框
    fn model_box(&self) -> Child {
        if self.page == OCR_PAGE { Child::OcrModel } else { Child::Model }
    }

    /// 测试连接那一行开始转圈；返回这次请求的编号，结果回来时编号不对就丢掉
    fn busy(&mut self, msg: &'static str) -> u32 {
        self.test_id += 1;
        self.test = Test::Busy(msg);
        self.w.timer(T_SPIN, FRAME_MS);
        self.test_id
    }

    fn start_test(&mut self) {
        if matches!(self.test, Test::Busy(_)) {
            return;
        }
        let cfg = self.service();
        if cfg.endpoint.is_empty() || cfg.model.is_empty() {
            self.test = Test::Done(false, "请先填写服务地址和模型".into());
            return;
        }
        let ocr = self.page == OCR_PAGE;
        let (id, w) = (self.busy("正在测试…"), self.w);
        std::thread::spawn(move || {
            let t0 = Instant::now();
            let result = if ocr { translate::test_image(&cfg, translate::TIMEOUT) } else { translate::test_connection(&cfg, translate::TIMEOUT) };
            let result = result.map(|_| t0.elapsed());
            w.post(move |ui: &mut Ui, _| ui.tested(id, result));
        });
    }

    fn tested(&mut self, id: u32, result: Result<Duration, translate::Error>) {
        if id != self.test_id {
            return;
        }
        self.w.kill_timer(T_SPIN);
        self.test = match result {
            Ok(d) if self.page == OCR_PAGE => Test::Done(true, format!("连接正常，模型能看图，用时 {:.1} 秒", d.as_secs_f32())),
            Ok(d) => Test::Done(true, format!("连接正常，用时 {:.1} 秒", d.as_secs_f32())),
            Err(e) => Test::Done(false, e.to_string()),
        };
        self.w.invalidate_all();
    }

    /// 「选择…」：用框里当前的地址和 Key 拉模型列表，拿到后弹下拉列表
    fn start_models(&mut self) {
        if matches!(self.test, Test::Busy(_)) {
            return;
        }
        let cfg = self.service();
        if cfg.endpoint.is_empty() {
            self.test = Test::Done(false, "请先填写服务地址".into());
            return;
        }
        let (id, w) = (self.busy("正在获取模型…"), self.w);
        std::thread::spawn(move || {
            let result = translate::list_models(&cfg, translate::TIMEOUT);
            w.post(move |ui: &mut Ui, _| ui.listed(id, result));
        });
    }

    fn listed(&mut self, id: u32, result: Result<Vec<String>, translate::Error>) {
        // 等的时候换了页的，结果在 reset_test 里已经丢掉了
        if id != self.test_id {
            return;
        }
        self.w.kill_timer(T_SPIN);
        self.w.invalidate_all();
        match result {
            Ok(list) if list.is_empty() => self.test = Test::Done(false, "服务没有列出模型，请直接填写模型名".into()),
            Ok(list) => {
                self.test = Test::Idle;
                self.models = list;
                self.menu(Ctl::Models);
            }
            Err(e) => self.test = Test::Done(false, e.to_string()),
        }
    }

    /// 快捷键有问题：翻到它那一页，红框加原因，焦点放回那个快捷键框
    fn reject_hotkey(&mut self, c: Child, msg: String) {
        self.hk_bad = Some((c, msg));
        self.set_page(c.page());
        self.w.focus_field(Some(c as usize));
    }

    fn browse(&self) {
        let start = if self.save_dir.is_empty() { plat::pictures_dir() } else { self.save_dir.clone() };
        let w = self.w;
        w.ask_folder(start, move |d| {
            if let Some(d) = d {
                w.with(|ui: &mut Ui| {
                    ui.set_text(Child::SaveDir, &d);
                    ui.save_dir = d;
                });
            }
        });
    }

    /// 「确定」：两个快捷键先注册，有一个失败就都恢复原键、留在窗口里；成功了再统一保存，关窗口，出胶囊
    fn apply(&mut self) {
        let need = if cfg!(target_os = "macos") { "需要包含 ⌃、⌥ 或 ⌘" } else { "需要包含 Ctrl 或 Alt" };
        let mut keys = Vec::new();
        for c in [Child::Hotkey, Child::RecHotkey] {
            match self.read_hotkey(c) {
                Some(hk) => keys.push(hk),
                None => return self.reject_hotkey(c, format!("请按下一个组合键，{need}")),
            }
        }
        let (hk, rec) = (keys[0], keys[1]);
        if hk == rec {
            return self.reject_hotkey(Child::RecHotkey, "和截图快捷键重复了，请换一个组合键".into());
        }
        // 改了的（和原来没注册上的）重新注册；先都注销，两个对调时才不会撞上自己
        let changed: Vec<_> = [(hotkeys::CAPTURE, self.orig.hotkey, hk, Child::Hotkey), (hotkeys::RECORD, self.orig.record_hotkey, rec, Child::RecHotkey)]
            .into_iter()
            .filter(|(id, old, new, _)| new != old || !plat::hotkey_ok(*id))
            .collect();
        // 注册 `pick` 选的那个键（新的或原来的），返回第一个没注册上的
        let register = |pick: fn(&(usize, Hotkey, Hotkey, Child)) -> Hotkey| {
            changed.iter().for_each(|k| _ = plat::set_hotkey(k.0, 0, 0));
            changed.iter().filter(|k| !hotkeys::apply(k.0, pick(k))).map(|k| (k.3, pick(k))).collect::<Vec<_>>().first().copied()
        };
        if let Some((c, new)) = register(|k| k.2) {
            register(|k| k.1);
            return self.reject_hotkey(c, format!("{} 已被其他程序占用，请换一个组合键", new.name(" + ")));
        }
        let new = Settings {
            hotkey: hk,
            record_hotkey: rec,
            record_fps: self.fps,
            gif_frame: self.gif_frame,
            sel_color: self.color,
            save_dir: self.save_dir.clone(),
            save_format: self.format,
            ocr_endpoint: self.text(Child::OcrEndpoint).trim().to_string(),
            ocr_model: self.text(Child::OcrModel).trim().to_string(),
            endpoint: self.text(Child::Endpoint).trim().to_string(),
            model: self.text(Child::Model).trim().to_string(),
            target: self.target,
        };
        let mut errs = Vec::new();
        if (self.autostart != self.autostart0 || self.autostart)
            && let Err(e) = plat::set_autostart(self.autostart)
        {
            errs.push(format!("开机启动没有设置成功：{e}"));
        }
        for (c, old, which, name) in [(Child::Key, &self.key0, settings::TRANSLATE_KEY, "翻译"), (Child::OcrKey, &self.ocr_key0, settings::OCR_KEY, "文字识别")] {
            let key = self.text(c).trim().to_string();
            if key != *old
                && let Err(e) = plat::set_api_key(which, &key)
            {
                errs.push(format!("{name}的 API Key 没有保存：{e}"));
            }
        }
        if let Err(e) = settings::set(new) {
            errs.push(format!("设置没有写进文件：{e}"));
        }
        hotkeys::refresh_tray();
        let mon = self.w.monitor();
        self.w.close();
        if errs.is_empty() {
            toast::show(&mon, Kind::Ok, "设置已保存");
        } else {
            toast::show(&mon, Kind::Err, &errs.join("；"));
        }
    }

    /// 下拉列表：系统菜单 + 自绘菜单项，当前项前面一个圆点
    fn menu(&self, c: Ctl) {
        let Some(mut r) = self.ctl_rect(c) else { return };
        let (options, cur): (Vec<String>, Option<usize>) = match c {
            // 模型列表对齐模型框弹出；手填的模型不在列表里就不打圆点
            Ctl::Models => {
                r.left = self.field(self.model_box()).left;
                let typed = self.text(self.model_box());
                (self.models.clone(), self.models.iter().position(|m| *m == typed.trim()))
            }
            _ => (TARGETS.iter().map(|t| t.1.to_string()).collect(), Some(TARGETS.iter().position(|t| t.0 == self.target).unwrap_or(0))),
        };
        let items: Vec<MenuItem> = options
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let key = if c == Ctl::Models && t == translate::DEEPSEEK_MODEL { "推荐" } else { "" };
                MenuItem { checked: Some(i) == cur, ..MenuItem::new(i + 1, t, key) }
            })
            .collect();
        let (w, picked) = (self.w, options.clone());
        let at = Point { x: r.left, y: r.bottom + (4.0 * self.scale) as i32 };
        w.menu(&items, at, move |id| {
            if id == 0 {
                return;
            }
            w.with(|ui: &mut Ui| {
                match c {
                    Ctl::Models => ui.set_text(ui.model_box(), &picked[id - 1]),
                    _ => ui.target = TARGETS[id - 1].0,
                }
                ui.w.invalidate_all();
            });
        });
    }

    fn draw(&self, c: &Canvas) -> Option<()> {
        let s = self.scale;
        let lw = s.round().max(1.0);
        let fill = |r: RectF, col: u32| c.fill_rect(r, rgb(col));
        // 整像素描边画在像素中心才不糊
        let frame = |r: &Rect, radius: f32, bg: Option<u32>, line: u32, width: f32| {
            let h = width / 2.0;
            let rr = rectf(r.left as f32 + h, r.top as f32 + h, r.right as f32 - h, r.bottom as f32 - h);
            if let Some(bg) = bg {
                c.fill_round(rr, radius * s, rgb(bg));
            }
            c.stroke_round(rr, radius * s, rgb(line), width);
        };
        let text = |t: &str, size: f32, bold: bool| Text::new(FONT, size * s, bold, t);
        // 一行字，(x, y) 是左上角（像素）；返回宽度
        let put = |t: &str, size: f32, bold: bool, x: f32, y: f32, color: u32| -> f32 {
            let Some(l) = text(t, size, bold) else { return 0.0 };
            c.text(&l, x, y, rgb(color));
            l.width()
        };
        // 一行字在 [top, bottom) 里上下居中
        let put_mid = |t: &str, size: f32, bold: bool, x: f32, top: f32, bottom: f32, color: u32| -> f32 {
            let Some(l) = text(t, size, bold) else { return 0.0 };
            c.text(&l, x, ((top + bottom - l.height()) / 2.0).round(), rgb(color));
            l.width()
        };
        let label = |t: &str, row: f32| put_mid(t, 12.0, false, self.px(LX), self.px(row), self.px(row + 28.0), TEXT);
        let hint = |t: &str, x: f32, y: f32, color: u32| -> f32 {
            let Some(l) = Text::wrapped(FONT, 12.0 * s, false, t, self.px(RIGHT) - x, None) else { return 0.0 };
            c.text(&l, x, y, rgb(color));
            l.height()
        };
        let state = |ctl: Ctl| match (self.hot == Some(ctl), self.down == Some(ctl)) {
            (true, true) => 2,
            (true, false) => 1,
            _ => 0,
        };
        let button = |ctl: Ctl, t: &str, primary: bool| {
            let Some(r) = self.ctl_rect(ctl) else { return };
            let st = state(ctl);
            let (bg, line, color) = if primary {
                ([ACCENT, ACCENT_HOVER, ACCENT_PRESS][st], [ACCENT, ACCENT_HOVER, ACCENT_PRESS][st], 0xFFFFFF)
            } else {
                ([0xFBFBFB, 0xF2F1F4, 0xE9E7EC][st], LINE, TEXT)
            };
            frame(&r, 4.0, Some(bg), line, lw);
            let Some(l) = text(t, 12.0, primary) else { return };
            let (x, y) = ((r.left + r.right) as f32 / 2.0 - l.width() / 2.0, (r.top + r.bottom) as f32 / 2.0 - l.height() / 2.0);
            c.text(&l, x.round(), y.round(), rgb(color));
        };
        let dropdown = |ctl: Ctl, t: &str| {
            let Some(r) = self.ctl_rect(ctl) else { return };
            frame(&r, 4.0, Some(if state(ctl) > 0 { 0xF2F1F4 } else { 0xFBFBFB }), LINE, lw);
            let Some(l) = text(t, 12.0, false) else { return };
            let icon_x = r.right as f32 - self.px(6.0) - self.px(14.0);
            l.truncate(icon_x - self.px(12.0) - r.left as f32 - self.px(10.0));
            c.text(&l, r.left as f32 + self.px(10.0), ((r.top + r.bottom) as f32 - l.height()) / 2.0, rgb(TEXT));
            ui::icon(c, "chev_down", icon_x, (r.top + r.bottom) as f32 / 2.0 - 7.0 * s, 14.0 * s, rgb(HINT));
        };
        let focus = self.w.focused_field();
        // 分段按钮：`rs` 是各段的位置，选中那段浅紫底、紫字
        let segments = |rs: &[Rect], labels: &[&str], sel: usize| {
            let (Some(a), Some(b)) = (rs.first(), rs.last()) else { return };
            let whole = Rect { left: a.left, top: a.top, right: b.right, bottom: a.bottom };
            frame(&whole, 4.0, Some(0xFFFFFF), LINE, lw);
            // 选中那段的底色只填在它自己那块，外侧的圆角跟着外框
            c.push_clip(rf(&rs[sel]));
            c.fill_round(rectf(whole.left as f32 + lw, whole.top as f32 + lw, whole.right as f32 - lw, whole.bottom as f32 - lw), 3.0 * s, rgb(ACCENT_SOFT));
            c.pop_clip();
            for r in &rs[1..] {
                fill(rectf(r.left as f32, r.top as f32, r.left as f32 + lw, r.bottom as f32), LINE);
            }
            for (i, r) in rs.iter().enumerate() {
                let on = i == sel;
                let Some(l) = text(labels[i], 12.0, on) else { continue };
                let x = (r.left + r.right) as f32 / 2.0 - l.width() / 2.0;
                c.text(&l, x.round(), ((r.top + r.bottom) as f32 / 2.0 - l.height() / 2.0).round(), rgb(if on { ACCENT } else { TEXT }));
            }
        };
        // 开关：开是主题色底、白点在右，关是白底灰边、灰点在左；右边一行说明，和标签 `row` 对齐
        let switch = |ctl: Ctl, on: bool, note: &str, row: f32| {
            let Some(r) = self.ctl_rect(ctl) else { return };
            frame(&r, 10.0, Some(if on { ACCENT } else { 0xFFFFFF }), if on { ACCENT } else { 0x8A8590 }, lw);
            let cx = r.left as f32 + if on { 29.0 } else { 9.0 } * s;
            c.fill_ellipse(cx, (r.top + r.bottom) as f32 / 2.0, 5.0 * s, 5.0 * s, rgb(if on { 0xFFFFFF } else { HINT }));
            put_mid(note, 12.0, false, r.right as f32 + self.px(8.0), self.px(row), self.px(row + 28.0), HINT);
        };
        // 快捷键框下面那行字
        let hotkey_hint = |ch: Child, orig: Hotkey| {
            let changed = self.read_hotkey(ch).is_some_and(|h| h != orig);
            let (msg, color) = match &self.hk_bad {
                Some((bad, m)) if *bad == ch => (m.as_str(), ERR),
                _ if focus == Some(ch as usize) => (if cfg!(target_os = "macos") { "请按下新的组合键，需要包含 ⌃、⌥ 或 ⌘" } else { "请按下新的组合键，需要包含 Ctrl 或 Alt" }, HINT),
                _ if changed => ("点「确定」后生效", HINT),
                _ => ("点一下输入框，再按新的组合键", HINT),
            };
            hint(msg, self.px(CX), self.px(ROW0 + 34.0), color);
        };
        // 「测试连接」和右边的结果：翻译页、文字识别页共用
        let test_row = || -> Option<()> {
            button(Ctl::Test, "测试连接", false);
            let Some(r) = self.ctl_rect(Ctl::Test) else { return Some(()) };
            let x = r.right as f32 + self.px(8.0);
            let (top, bottom) = (r.top as f32, r.bottom as f32);
            match &self.test {
                Test::Idle => {}
                Test::Busy(msg) => {
                    ui::spinner(c, x, ((top + bottom) / 2.0 - 8.0 * s).round(), s, self.angle, rgb(0xEBDDFB), rgb(ACCENT));
                    put_mid(msg, 12.0, false, x + self.px(22.0), top, bottom, HINT);
                }
                Test::Done(ok, msg) => {
                    // 第一行和按钮文字对齐，长的错误原因往下折行
                    let l = Text::wrapped(FONT, 12.0 * s, false, msg, self.px(RIGHT) - x, None)?;
                    let h = l.first_line_height();
                    c.text(&l, x, ((top + bottom - h) / 2.0).round(), rgb(if *ok { OK } else { ERR }));
                }
            }
            Some(())
        };

        c.clear(rgb(0xFFFFFF));
        // 左侧导航
        let (main_h, nav_w, win_w) = (self.px(MAIN_H), self.px(NAV_W), self.px(WIN_W));
        fill(rectf(0.0, 0.0, nav_w, main_h), 0xFAF9FB);
        fill(rectf(nav_w - lw, 0.0, nav_w, main_h), 0xEEEDF0);
        for (i, name) in PAGES.iter().enumerate() {
            let Some(r) = self.ctl_rect(Ctl::Nav(i)) else { continue };
            let on = i == self.page;
            if on || state(Ctl::Nav(i)) > 0 {
                c.fill_round(rf(&r), 4.0 * s, rgb(if on { ACCENT_SOFT } else { 0xEFEDF2 }));
            }
            put_mid(name, 13.0, on, r.left as f32 + self.px(12.0), r.top as f32, r.bottom as f32, if on { ACCENT } else { TEXT });
        }
        // 页标题
        put(PAGES[self.page], 18.0, true, self.px(LX), self.px(18.0), TEXT);
        match self.page {
            SHOT_PAGE => {
                label("快捷键", ROW0);
                hotkey_hint(Child::Hotkey, self.orig.hotkey);
                label("开机启动", 120.0);
                switch(Ctl::Toggle, self.autostart, "开机时自动启动 Jie", 120.0);
                put("选区颜色", 12.0, false, self.px(LX), self.px(SW_Y + 5.0), TEXT);
                let cur = self.swatch();
                for i in 0..8 {
                    let Some(r) = self.ctl_rect(Ctl::Swatch(i)) else { continue };
                    let (cx, cy) = ((r.left + r.right) as f32 / 2.0, (r.top + r.bottom) as f32 / 2.0);
                    let custom_set = i == 7 && cur == 7;
                    let color = match PRESETS.get(i) {
                        Some(p) => p.1,
                        None if custom_set => self.color,
                        None => 0xFFFFFF,
                    };
                    c.fill_ellipse(cx, cy, 12.0 * s, 12.0 * s, rgb(color));
                    let edge = if i == 7 && !custom_set { rgb(0x8A8590) } else { rgba(0, 0.18) };
                    c.stroke_ellipse(cx, cy, 11.5 * s, 11.5 * s, edge, 1.0 * s, Stroke::Plain);
                    if i == cur {
                        // 选中：外面隔 2 的白，再一圈 1.5 的深色
                        c.stroke_ellipse(cx, cy, 14.75 * s, 14.75 * s, rgb(TEXT), 1.5 * s, Stroke::Plain);
                    }
                    if i == 7 && !custom_set {
                        let Some(l) = text("+", 15.0, true) else { continue };
                        c.text(&l, cx - l.width() / 2.0, cy - l.height() / 2.0, rgb(HINT));
                    }
                }
                let mut y = SW_Y + 24.0 + 6.0;
                if self.picker {
                    self.draw_picker(c, lw)?;
                    y = PICK_Y + 198.0 + 6.0;
                }
                hint("用在选区边框、手柄、窗口高亮、录屏边框、选中标注的虚线框和钉图描边上", self.px(CX), self.px(y), HINT);
            }
            REC_PAGE => {
                label("快捷键", ROW0);
                hotkey_hint(Child::RecHotkey, self.orig.record_hotkey);
                label("帧率", 126.0);
                let mut rs = Vec::new();
                for i in 0..FPS_CHOICES.len() {
                    rs.extend(self.ctl_rect(Ctl::Fps(i)));
                }
                let labels: Vec<String> = FPS_CHOICES.iter().map(|f| f.to_string()).collect();
                segments(&rs, &labels.iter().map(String::as_str).collect::<Vec<_>>(), self.fps_index());
                if let Some(b) = rs.last() {
                    put_mid("帧 / 秒", 12.0, false, b.right as f32 + self.px(8.0), b.top as f32, b.bottom as f32, HINT);
                }
                hint("帧率越高越流畅，GIF 文件也越大。区域大、画面变化多时，实际帧率会低一些。", self.px(CX), self.px(160.0), HINT);
                label("动图边框", 212.0);
                switch(Ctl::GifFrame, self.gif_frame, "录好的 GIF 钉在屏幕上时，外面画一圈选区颜色的框", 212.0);
                hint(
                    "按快捷键后框选要录的区域（可以吸附窗口和页面里的边界），点「开始录制」马上开始，旁边的秒表按钮可以选延时 3、5、10 秒；\
                     录制中再按一次快捷键，或点控制条上的「停止」。录好的 GIF 在原位置播放，可以复制、保存或钉在屏幕上。",
                    self.px(CX),
                    self.px(262.0),
                    HINT,
                );
            }
            SAVE_PAGE => {
                label("默认位置", ROW0);
                button(Ctl::Browse, "浏览…", false);
                hint("每次保存后会自动记成上次用的文件夹", self.px(CX), self.px(98.0), HINT);
                label("格式", 126.0);
                if let (Some(a), Some(b)) = (self.ctl_rect(Ctl::Format(0)), self.ctl_rect(Ctl::Format(1))) {
                    segments(&[a, b], &["PNG", "JPEG"], self.format_index());
                    put_mid("JPEG 质量固定 90", 12.0, false, b.right as f32 + self.px(8.0), b.top as f32, b.bottom as f32, HINT);
                }
            }
            OCR_PAGE => {
                label("服务地址", 58.0);
                label("模型", 98.0);
                button(Ctl::Models, "选择…", false);
                label("API Key", 138.0);
                hint("截图会发给这个服务识别，三项都留空就用翻译页的。", self.px(CX), self.px(172.0), HINT);
                test_row()?;
            }
            _ => {
                label("服务地址", 58.0);
                label("模型", 98.0);
                button(Ctl::Models, "选择…", false);
                label("API Key", 138.0);
                label("译成", 178.0);
                dropdown(Ctl::Target, self.target_label());
                if let Some(r) = self.ctl_rect(Ctl::Target) {
                    let t = self.target_hint();
                    if self.hint_wraps() {
                        hint(t, self.px(CX), r.bottom as f32 + self.px(6.0), HINT);
                    } else {
                        put_mid(t, 12.0, false, r.right as f32 + self.px(8.0), r.top as f32, r.bottom as f32, HINT);
                    }
                }
                test_row()?;
            }
        }
        // 系统控件的外框（画在页面内容之后，取色器的底色才不会盖住色值框）：平时 1px，聚焦时 2px 主题色，快捷键有问题时 2px 红
        for ch in CHILDREN.into_iter().filter(|ch| self.visible(*ch)) {
            let r = self.field(ch);
            let bad = self.hk_bad.as_ref().is_some_and(|(c, _)| *c == ch);
            let (line, width) = if bad {
                (ERR, 2.0 * lw)
            } else if focus == Some(ch as usize) {
                (ACCENT, 2.0 * lw)
            } else {
                (LINE, lw)
            };
            frame(&r, 4.0, Some(0xFFFFFF), line, width);
        }
        // 底栏
        fill(rectf(0.0, main_h, win_w, self.px(MAIN_H + FOOT_H)), 0xF7F6F9);
        fill(rectf(0.0, main_h, win_w, main_h + lw), 0xEEEDF0);
        put_mid(concat!("Jie ", env!("CARGO_PKG_VERSION")), 12.0, false, self.px(24.0), main_h, self.px(MAIN_H + FOOT_H), HINT);
        button(Ctl::Ok, "确定", true);
        button(Ctl::Cancel, "取消", false);
        // 键盘焦点框：2px 主题色，离控件 1
        if self.kbd
            && self.w.focused()
            && let Some(ctl) = self.focus
            && let Some(r) = self.ctl_rect(ctl)
        {
            let g = 2.0 * lw;
            let o = Rect { left: r.left - g as i32, top: r.top - g as i32, right: r.right + g as i32, bottom: r.bottom + g as i32 };
            let radius = match ctl {
                Ctl::Swatch(_) => 16.0,
                Ctl::Toggle | Ctl::GifFrame => 12.0,
                _ => 6.0,
            };
            frame(&o, radius, None, ACCENT, g);
        }
        Some(())
    }

    /// 取色器：饱和度/明度方块 + 色相条 + 预览色块（色值输入框是系统控件）
    fn draw_picker(&self, c: &Canvas, lw: f32) -> Option<()> {
        let s = self.scale;
        let boxr = self.rect(CX, PICK_Y, CX + 222.0, PICK_Y + 198.0);
        let h = lw / 2.0;
        let rr = rectf(boxr.left as f32 + h, boxr.top as f32 + h, boxr.right as f32 - h, boxr.bottom as f32 - h);
        c.fill_round(rr, 6.0 * s, rgb(0xFCFBFD));
        c.stroke_round(rr, 6.0 * s, rgb(0xE6E3EA), lw);
        let (Some(sv), Some(hue)) = (self.ctl_rect(Ctl::Sv), self.ctl_rect(Ctl::Hue)) else { return Some(()) };
        // 方块和色相条逐像素算出来，每次重画都重新生成（只有几万个像素）
        let bitmap = |r: &Rect, f: &dyn Fn(f32, f32) -> u32| {
            let (w, ht) = ((r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32);
            let mut px = Vec::with_capacity((w * ht * 4) as usize);
            for y in 0..ht {
                for x in 0..w {
                    px.extend_from_slice(&(f(x as f32 / (w - 1).max(1) as f32, y as f32 / (ht - 1).max(1) as f32) | 0xFF00_0000).to_le_bytes());
                }
            }
            c.pixels(&px, w, ht, rf(r));
        };
        let (hh, ss, vv) = self.hsv;
        bitmap(&sv, &|x, y| hsv_rgb((hh, x, 1.0 - y)));
        bitmap(&hue, &|x, _| hsv_rgb(((x * 360.0) % 360.0, 1.0, 1.0)));
        let (white, dark) = (rgb(0xFFFFFF), rgba(0, 0.6));
        c.push_clip(rf(&sv));
        let (cx, cy) = (sv.left as f32 + ss * (sv.right - sv.left) as f32, sv.top as f32 + (1.0 - vv) * (sv.bottom - sv.top) as f32);
        c.stroke_ellipse(cx, cy, 6.0 * s, 6.0 * s, white, 2.0 * s, Stroke::Plain);
        c.stroke_ellipse(cx, cy, 7.5 * s, 7.5 * s, dark, 1.0 * s, Stroke::Plain);
        c.pop_clip();
        c.push_clip(rf(&hue));
        let x = hue.left as f32 + hh / 360.0 * (hue.right - hue.left) as f32;
        let (t, b) = (hue.top as f32, hue.bottom as f32);
        c.fill_rect(rectf(x - 2.0 * s, t, x + 2.0 * s, b), white);
        c.stroke_rect(rectf(x - 2.5 * s, t + 0.5 * s, x + 2.5 * s, b - 0.5 * s), dark, 1.0 * s, Stroke::Plain);
        c.pop_clip();
        // 预览色块
        let p = self.rect(CX + 11.0, PICK_Y + 159.0, CX + 39.0, PICK_Y + 187.0);
        c.fill_round(rf(&p), 4.0 * s, rgb(self.color));
        c.stroke_round(rectf(p.left as f32 + h, p.top as f32 + h, p.right as f32 - h, p.bottom as f32 - h), 4.0 * s, rgba(0, 0.18), lw);
        Some(())
    }
}

impl Handler for Ui {
    fn event(&mut self, w: Win, e: Event) {
        match e {
            Event::Down { p, button: Button::Left, .. } => {
                self.kbd = false;
                // 点在输入框的外框上（系统控件外面那一圈）也算点输入框
                if let Some(c) = CHILDREN.into_iter().find(|c| self.visible(*c) && contains(&self.field(*c), p)) {
                    w.focus_field(Some(c as usize));
                } else {
                    w.focus_field(None);
                    self.focus = self.ctl_at(p);
                    self.down = self.focus;
                    if let Some(c @ (Ctl::Sv | Ctl::Hue)) = self.down {
                        self.drag = Some(c);
                        self.pick(c, p);
                    }
                }
                w.invalidate_all();
            }
            Event::Move { p, .. } => {
                if let Some(c) = self.drag {
                    self.pick(c, p);
                }
                let hot = self.ctl_at(p);
                if hot != self.hot {
                    self.hot = hot;
                    w.invalidate_all();
                }
            }
            Event::Leave => {
                if self.hot.take().is_some() {
                    w.invalidate_all();
                }
            }
            Event::Up { button: Button::Left, .. } => {
                let p = w.cursor_pos();
                let clicked = self.down.take().filter(|c| self.ctl_at(p) == Some(*c));
                self.drag = None;
                w.invalidate_all();
                if let Some(c) = clicked {
                    self.click(c);
                }
            }
            Event::CaptureLost => {
                self.drag = None;
                self.down = None;
            }
            Event::Key { key, mods } => self.key(key, mods.shift, true),
            Event::FieldKey { key, shift } => self.key(key, shift, false),
            Event::FieldChanged(id) => {
                match CHILDREN.into_iter().find(|c| *c as usize == id) {
                    Some(Child::Hotkey | Child::RecHotkey) => self.hk_bad = None,
                    Some(Child::Hex) if !self.syncing => {
                        if let Some(c) = parse_hex(&self.text(Child::Hex)) {
                            self.color = c;
                            self.hsv = hsv_of(c);
                        }
                    }
                    _ => {}
                }
                w.invalidate_all();
            }
            Event::Timer(T_SPIN) => {
                self.angle = (self.angle + 360.0 * FRAME_MS as f32 / 800.0) % 360.0;
                w.invalidate_all();
            }
            Event::Close => w.close(),
            // 拖到缩放比例不同的显示器上：用系统建议的窗口位置，按新比例重摆
            Event::Dpi { scale, rect } => {
                self.scale = scale;
                self.place_kids();
                w.set_rect(rect);
                w.invalidate_all();
            }
            _ => {}
        }
    }

    fn paint(&mut self, _w: Win, c: &Canvas) {
        let _ = self.draw(c);
    }

    fn cursor(&mut self, _w: Win, p: Point) -> Cursor {
        if matches!(self.ctl_at(p), Some(Ctl::Sv | Ctl::Hue)) || self.drag.is_some() { Cursor::Cross } else { Cursor::Arrow }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trip() {
        for (_, c) in PRESETS {
            assert_eq!(hsv_rgb(hsv_of(c)), c, "{c:06X}");
        }
        for c in [0x000000, 0x808080, 0x123456, 0xFEDCBA] {
            assert_eq!(hsv_rgb(hsv_of(c)), c, "{c:06X}");
        }
        assert_eq!(hsv_rgb((0.0, 1.0, 1.0)), 0xFF0000);
        assert_eq!(hsv_rgb((120.0, 1.0, 1.0)), 0x00FF00);
        assert_eq!(hsv_rgb((240.0, 0.0, 1.0)), 0xFFFFFF);
    }

    #[test]
    fn hex_input() {
        assert_eq!(parse_hex("#9C44EC"), Some(0x9C44EC));
        assert_eq!(parse_hex(" 9c44ec "), Some(0x9C44EC));
        assert_eq!(parse_hex("#9C44E"), None);
        assert_eq!(parse_hex("#9C44EG"), None);
        assert_eq!(parse_hex("+9C44EC"), None);
        assert_eq!(hex(0x0A0B0C), "#0A0B0C");
    }

    /// 处理消息直到 done 成立，最多等 20 秒
    fn wait(ui: &Ui, done: impl Fn(&Ui) -> bool) {
        let t0 = Instant::now();
        while !done(ui) {
            assert!(t0.elapsed() < Duration::from_secs(20), "等超时了");
            if !plat::pump() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// 开出真的设置窗口。设置从 target/test-tmp/<name> 读，不碰真的配置；API Key 在系统里，这里只读不写
    fn open(name: &str) -> (Win, &'static mut Ui, std::path::PathBuf) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Jie")).unwrap();
        unsafe { std::env::set_var("APPDATA", &dir) };
        settings::load();
        show(false);
        let w = *Win::all(plat::Kind::Settings).first().unwrap();
        (w, w.peek::<Ui>(), dir)
    }

    /// 截下屏幕上 `r` 这块，存成 PNG
    fn shoot(r: Rect, path: &std::path::Path) {
        let (mon, bmp) = plat::capture_cursor_monitor().unwrap();
        let o = mon.rect;
        let r = Rect { left: (r.left - o.left).max(0), top: (r.top - o.top).max(0), right: (r.right - o.left).min(bmp.w), bottom: (r.bottom - o.top).min(bmp.h) };
        std::fs::write(path, plat::encode(&bmp.crop(&r), false).unwrap()).unwrap();
    }

    /// 翻译页走一遍：没配过时预填 DeepSeek → 「选择…」对着本地 mock 拉列表 → 弹出的菜单截图 →
    /// 方向键选第二个填进模型框 → mock 回 401 时测试连接那一行写原因。
    /// 会弹出真的设置窗口和菜单，截图放 target/test-tmp/settings，平时不跑：
    /// cargo test translate_page -- --ignored --nocapture
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn translate_page() {
        use crate::translate::tests::{http, mock};

        let (w, ui, dir) = open("settings");
        ui.set_page(TR_PAGE);
        assert_eq!((ui.text(Child::Endpoint).as_str(), ui.text(Child::Model).as_str()), (translate::DEEPSEEK, translate::DEEPSEEK_MODEL));

        // 菜单弹出来以后另一个线程截屏，再按 ↓↓ Enter 选第二个
        let body = r#"{"object":"list","data":[{"id":"deepseek-flash"},{"id":"deepseek-v4-pro"}]}"#;
        let (base, _req) = mock(Some(http("200 OK", body)));
        ui.set_text(Child::Endpoint, &base);
        let picker = plat::answer_menu(w, Some(dir.join("models-menu.png")), &[Key::Down, Key::Down, Key::Enter]);
        w.with(|ui: &mut Ui| ui.click(Ctl::Models));
        wait(ui, |u| u.text(Child::Model) == "deepseek-v4-pro");
        picker.join().unwrap();
        assert!(matches!(ui.test, Test::Idle));

        // Key 错：原因写在测试连接那一行
        let (bad, _req) = mock(Some(http("401 Unauthorized", r#"{"error":{"message":"bad key"}}"#)));
        ui.set_text(Child::Endpoint, &bad);
        w.with(|ui: &mut Ui| ui.click(Ctl::Models));
        wait(ui, |u| matches!(&u.test, Test::Done(false, m) if m == "API Key 无效（401），请到设置里检查。"));
        w.close();
    }

    /// 文字识别页走一遍：三个框 → 测试连接发的是图片、用这页填的服务 →
    /// 留空时用翻译页的地址、Key → 模型不收图片时写原因。截图放 target/test-tmp/settings-ocr，平时不跑：
    /// cargo test ocr_page -- --ignored --nocapture
    #[test]
    #[ignore]
    fn ocr_page() {
        use crate::translate::tests::{http, mock};
        let (w, ui, dir) = open("settings-ocr");
        let settle = |ui: &Ui| {
            let t0 = Instant::now();
            wait(ui, |_| t0.elapsed() > Duration::from_millis(300));
        };
        w.with(|ui: &mut Ui| ui.set_page(OCR_PAGE));
        settle(ui);
        shoot(w.rect(), &dir.join("1-page.png"));
        assert!(ui.visible(Child::OcrEndpoint) && ui.visible(Child::OcrKey));

        // 单独填的服务：发图片、用这页的模型和 Key
        let ok = r#"{"choices":[{"message":{"content":""}}]}"#;
        let (base, req) = mock(Some(http("200 OK", ok)));
        ui.set_text(Child::OcrEndpoint, &base);
        ui.set_text(Child::OcrModel, "vl-1");
        ui.set_text(Child::OcrKey, "sk-ocr-test");
        w.with(|ui: &mut Ui| ui.click(Ctl::Test));
        wait(ui, |u| matches!(&u.test, Test::Done(..)));
        assert!(matches!(&ui.test, Test::Done(true, m) if m.starts_with("连接正常，模型能看图")));
        let r = req.recv().unwrap();
        assert!(r.contains("data:image/png;base64,") && r.contains("\"vl-1\"") && r.to_lowercase().contains("bearer sk-ocr-test"), "{r}");

        // 地址、Key 留空：用翻译页的
        let (base, req) = mock(Some(http("200 OK", ok)));
        ui.set_text(Child::Endpoint, &base);
        ui.set_text(Child::Key, "sk-tr-test");
        ui.set_text(Child::OcrEndpoint, "");
        ui.set_text(Child::OcrKey, "");
        w.with(|ui: &mut Ui| ui.click(Ctl::Test));
        wait(ui, |u| matches!(&u.test, Test::Done(true, _)));
        let r = req.recv().unwrap();
        assert!(r.contains("\"vl-1\"") && r.to_lowercase().contains("bearer sk-tr-test"), "{r}");

        // 模型不收图片
        let (bad, _req) = mock(Some(http("400 Bad Request", r#"{"error":{"message":"image input is not supported by this model"}}"#)));
        ui.set_text(Child::OcrEndpoint, &bad);
        ui.set_text(Child::OcrKey, "sk-ocr-test");
        w.with(|ui: &mut Ui| ui.click(Ctl::Test));
        wait(ui, |u| matches!(&u.test, Test::Done(false, m) if m.contains("image input is not supported")));
        settle(ui);
        shoot(w.rect(), &dir.join("4-no-image.png"));

        // 翻页再回来，测试结果清掉
        w.with(|ui: &mut Ui| ui.set_page(TR_PAGE));
        assert!(matches!(ui.test, Test::Idle));
        w.close();
    }

    /// 录屏页：截图存 target/test-tmp/settings-rec；录屏快捷键和截图的一样时「确定」被拦下（这时还没注册任何快捷键），
    /// 翻到录屏页、红框标在录屏快捷键上；帧率分段可以用方向键换。平时不跑：cargo test record_page -- --ignored
    #[test]
    #[ignore]
    fn record_page() {
        let (w, ui, dir) = open("settings-rec");
        w.with(|ui: &mut Ui| ui.set_page(REC_PAGE));
        let t0 = Instant::now();
        wait(ui, |_| t0.elapsed() > Duration::from_millis(300));
        shoot(w.rect(), &dir.join("1-page.png"));
        assert!(ui.visible(Child::RecHotkey) && !ui.visible(Child::Hotkey));
        assert_eq!(ui.fps, 15);
        w.with(|ui: &mut Ui| {
            ui.focus = Some(Ctl::Fps(1));
            ui.key(Key::Right, false, true);
        });
        assert_eq!(ui.fps, 20);
        let hk = ui.orig.hotkey;
        w.set_hotkey_field(Child::RecHotkey as usize, hk.mods, hk.vk);
        w.with(|ui: &mut Ui| {
            ui.set_page(SHOT_PAGE);
            ui.apply();
        });
        assert!(matches!(&ui.hk_bad, Some((Child::RecHotkey, m)) if m.contains("重复")));
        assert_eq!(ui.page, REC_PAGE);
        let t0 = Instant::now();
        wait(ui, |_| t0.elapsed() > Duration::from_millis(200));
        shoot(w.rect(), &dir.join("2-duplicate.png"));
        w.close();
    }
}
