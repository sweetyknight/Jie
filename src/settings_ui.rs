//! 设置窗口：系统边框 + 自绘内容（新野兽派：奶油底、左边导航、右边白色面板）。
//! 只有输入框和快捷键框用系统控件，去掉系统边框、边框自己画。「确定」时统一保存并立即生效，
//! 快捷键注册失败就恢复原键、停在窗口里提示。尺寸按设计稿（DIP）乘窗口所在显示器的缩放比例。

use crate::plat::{self, Button, Canvas, Cursor, Event, Face, FieldKind, Handler, Key, MenuItem, Point, Rect, RectF, Spec, Stroke, Text, Win};
use crate::hotkeys;
use crate::settings::{self, FPS_CHOICES, FRAME_CHOICES, Format, Hotkey, Settings};
use crate::toast::{self, Kind};
use crate::translate::{self, Lang};
use crate::ui::{self, rectf, rgb, rgba};
use std::time::Instant;

const T_SPIN: usize = 1;
const FRAME_MS: u32 = 16;

// 设计稿尺寸（DIP）
const WIN_W: f32 = 640.0;
/// 内容区高度：放得下「常规」页展开取色器、「文字与翻译」页打开识别服务的样子
const MAIN_H: f32 = 486.0;
const FOOT_H: f32 = 60.0;
/// 导航宽；右边白色面板从这里到 PANEL_R
const NAV_W: f32 = 158.0;
const PANEL_R: f32 = 626.0;
/// 标签列、控件列的左边，内容区右边
const LX: f32 = 184.0;
const CX: f32 = 290.0;
const RIGHT: f32 = 600.0;
/// 第一行的顶；输入框、按钮高
const ROW0: f32 = 66.0;
const FIELD_H: f32 = 32.0;
/// 「常规」页：第二个快捷键、开机启动、色块那一行，取色器方框的顶
const REC_Y: f32 = 110.0;
const AUTO_Y: f32 = 180.0;
const SW_Y: f32 = 224.0;
const PICK_Y: f32 = 264.0;
/// 「文字与翻译」页：识别服务开关那一行
const OCR_Y: f32 = 242.0;

/// 白底上的出错、成功字（ui::ERR、ui::OK 是填充色，做字太浅）
const ERR_TEXT: u32 = 0xC42B1C;
const OK_TEXT: u32 = 0x1D7A3A;
/// 设置窗口的字一律 Segoe UI（不用界面字的 Variable 版），和系统输入框里的字一样
const FONT: Face = Face::Segoe;

/// 页名和导航图标
const PAGES: [(&str, &str); 3] = [("常规", "sliders"), ("截图与录屏", "crop"), ("文字与翻译", "translate")];
const GENERAL_PAGE: usize = 0;
const CAPTURE_PAGE: usize = 1;
const SERVICE_PAGE: usize = 2;
const PRESETS: [(&str, u32); 7] =
    [("紫", 0x9C44EC), ("品红", 0xE0389A), ("红", 0xE5383B), ("橙", 0xF27A1A), ("黄", 0xF5B700), ("绿", 0x23A55A), ("白", 0xFFFFFF)];
/// 翻译目标只提供中文、英文。
const TARGETS: [(Lang, &str); 2] = [(Lang::Zh, "简体中文"), (Lang::En, "English")];
const FORMATS: [&str; 2] = ["PNG", "JPEG"];
/// FRAME_CHOICES 各档的名字
const FRAME_NAMES: [&str; 3] = ["细", "中", "粗"];
/// 「截图与录屏」页：两个边框开关、边框粗细那几行的顶
const SHOT_FRAME_Y: f32 = 268.0;
const GIF_FRAME_Y: f32 = 312.0;
const WIDTH_Y: f32 = 356.0;
/// 长图显示方式沿用分段控件，放在边框设置下方
const LONGSHOT_Y: f32 = 420.0;
const LONGSHOT_NAMES: [&str; 2] = ["全部显示", "窗口内滚动"];

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
    /// 「截图边框」「动图边框」开关
    ShotFrame,
    GifFrame,
    /// 边框粗细，FRAME_CHOICES 里的第几个
    FrameWidth(usize),
    /// 长图钉图：全部显示、原窗口内滚动
    Longshot(usize),
    Target,
    /// 模型框右边的「选择…」：false 是翻译服务的，true 是识别服务的
    Models(bool),
    /// 「另用一家服务识别文字」开关
    Ocr,
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
    /// 识别文字的服务，开关打开才显示；留空的项沿用上面的
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
            Child::Hotkey | Child::RecHotkey | Child::Hex => GENERAL_PAGE,
            Child::SaveDir => CAPTURE_PAGE,
            _ => SERVICE_PAGE,
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
    shot_frame: bool,
    gif_frame: bool,
    frame_width: u32,
    longshot_scroll: bool,
    target: Lang,
    /// 另用一家服务识别文字
    ocr: bool,
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
                ui.set_page(GENERAL_PAGE);
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
        shot_frame: orig.shot_frame,
        gif_frame: orig.gif_frame,
        frame_width: orig.frame_width,
        longshot_scroll: orig.longshot_scroll,
        target: orig.target,
        ocr: !orig.ocr_endpoint.is_empty() || !orig.ocr_model.is_empty(),
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

/// 文字卡片的「设置」：打开设置并翻到「文字与翻译」
pub fn show_service() {
    show(false);
    if let Some(&w) = Win::all(plat::Kind::Settings).first() {
        w.with(|ui: &mut Ui| ui.set_page(SERVICE_PAGE));
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

    /// 带字按钮宽：文字 + 左右各 14，至少 72
    fn btn_w(&self, text: &str) -> f32 {
        (self.tw(text, 13.0, true) + 28.0).max(72.0).ceil()
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
        // 左 10 + 文字 + 间距 8 + 箭头 16 + 右 8，至少 120
        (self.tw(text, 13.0, false) + 42.0).clamp(120.0, RIGHT - CX).ceil()
    }

    fn target_hint(&self) -> &'static str {
        if self.target == Lang::En { "原文已是英文时改译成中文" } else { "原文已是这种语言时改译成英文" }
    }

    fn fps_index(&self) -> usize {
        FPS_CHOICES.iter().position(|f| *f == self.fps).unwrap_or(1)
    }

    fn width_index(&self) -> usize {
        FRAME_CHOICES.iter().position(|f| *f == self.frame_width).unwrap_or(1)
    }

    /// 一行分段（顶 `y`）里各段的位置：外框内边距 3、段间距 2，每段文字 + 左右各 12
    fn segs<T: AsRef<str>>(&self, y: f32, labels: &[T]) -> Vec<Rect> {
        let mut x = CX + 3.0;
        labels
            .iter()
            .map(|l| {
                let w = (self.tw(l.as_ref(), 12.5, true) + 24.0).ceil();
                x += w + 2.0;
                self.rect(x - w - 2.0, y + 3.0, x - 2.0, y + FIELD_H - 3.0)
            })
            .collect()
    }

    /// 开关 44 × 24，在一行（顶 `y`）里上下居中
    fn switch_rect(&self, y: f32) -> Rect {
        self.rect(CX, y + 4.0, CX + 44.0, y + 28.0)
    }

    /// 输入框的外框（DIP）
    fn field_dip(&self, c: Child) -> (f32, f32, f32, f32) {
        let row = |y: f32| (CX, y, RIGHT, y + FIELD_H);
        let with_btn = |y: f32, b: &str| (CX, y, RIGHT - 10.0 - self.btn_w(b), y + FIELD_H);
        match c {
            Child::Hotkey => row(ROW0),
            Child::RecHotkey => row(REC_Y),
            Child::SaveDir => with_btn(ROW0, "浏览…"),
            Child::Hex => (CX + 47.0, PICK_Y + 159.0, CX + 143.0, PICK_Y + 187.0),
            Child::Endpoint => row(ROW0),
            Child::Model => with_btn(ROW0 + 44.0, "选择…"),
            Child::Key => row(ROW0 + 88.0),
            Child::OcrEndpoint => row(OCR_Y + 44.0),
            Child::OcrModel => with_btn(OCR_Y + 88.0, "选择…"),
            Child::OcrKey => row(OCR_Y + 132.0),
        }
    }

    fn field(&self, c: Child) -> Rect {
        let (l, t, r, b) = self.field_dip(c);
        self.rect(l, t, r, b)
    }

    fn visible(&self, c: Child) -> bool {
        c.page() == self.page
            && match c {
                Child::Hex => self.picker,
                Child::OcrEndpoint | Child::OcrModel | Child::OcrKey => self.ocr,
                _ => true,
            }
    }

    /// 测试连接那一行的顶：识别服务的三项打开时往下挪
    fn test_y(&self) -> f32 {
        if self.ocr { OCR_Y + 176.0 } else { OCR_Y + 70.0 }
    }

    /// 当前页能点的自绘控件和位置
    fn ctls(&self) -> Vec<(Ctl, Rect)> {
        let mut v: Vec<(Ctl, Rect)> =
            (0..PAGES.len()).map(|i| (Ctl::Nav(i), self.rect(10.0, 6.0 + 46.0 * i as f32, NAV_W - 10.0, 46.0 + 46.0 * i as f32))).collect();
        let btn = |b: &str, y: f32| self.rect(RIGHT - self.btn_w(b), y, RIGHT, y + FIELD_H);
        match self.page {
            GENERAL_PAGE => {
                v.push((Ctl::Toggle, self.switch_rect(AUTO_Y)));
                for i in 0..8 {
                    let x = CX + 36.0 * i as f32;
                    v.push((Ctl::Swatch(i), self.rect(x, SW_Y, x + 32.0, SW_Y + 32.0)));
                }
                if self.picker {
                    v.push((Ctl::Sv, self.rect(CX + 11.0, PICK_Y + 11.0, CX + 211.0, PICK_Y + 131.0)));
                    v.push((Ctl::Hue, self.rect(CX + 11.0, PICK_Y + 139.0, CX + 211.0, PICK_Y + 151.0)));
                }
            }
            CAPTURE_PAGE => {
                v.push((Ctl::Browse, btn("浏览…", ROW0)));
                v.extend(self.segs(136.0, &FORMATS).into_iter().enumerate().map(|(i, r)| (Ctl::Format(i), r)));
                v.extend(self.segs(180.0, &FPS_CHOICES.map(|f| f.to_string())).into_iter().enumerate().map(|(i, r)| (Ctl::Fps(i), r)));
                v.push((Ctl::ShotFrame, self.switch_rect(SHOT_FRAME_Y)));
                v.push((Ctl::GifFrame, self.switch_rect(GIF_FRAME_Y)));
                v.extend(self.segs(WIDTH_Y, &FRAME_NAMES).into_iter().enumerate().map(|(i, r)| (Ctl::FrameWidth(i), r)));
                v.extend(self.segs(LONGSHOT_Y, &LONGSHOT_NAMES).into_iter().enumerate().map(|(i, r)| (Ctl::Longshot(i), r)));
            }
            _ => {
                v.push((Ctl::Models(false), btn("选择…", ROW0 + 44.0)));
                v.push((Ctl::Target, self.rect(CX, ROW0 + 132.0, CX + self.drop_w(self.target_label()), ROW0 + 132.0 + FIELD_H)));
                v.push((Ctl::Ocr, self.switch_rect(OCR_Y)));
                if self.ocr {
                    v.push((Ctl::Models(true), btn("选择…", OCR_Y + 88.0)));
                }
                let y = self.test_y();
                v.push((Ctl::Test, self.rect(CX, y, CX + self.btn_w("测试连接"), y + FIELD_H)));
            }
        }
        // 底栏按钮右对齐面板：Windows「确定 取消」，mac「取消 确定」
        let (y, ow, cw) = (MAIN_H + 13.0, self.btn_w("确定"), self.btn_w("取消"));
        let ((first, fw), (second, sw)) = if cfg!(target_os = "macos") { ((Ctl::Cancel, cw), (Ctl::Ok, ow)) } else { ((Ctl::Ok, ow), (Ctl::Cancel, cw)) };
        v.push((first, self.rect(PANEL_R - sw - 10.0 - fw, y, PANEL_R - sw - 10.0, y + 34.0)));
        v.push((second, self.rect(PANEL_R - sw, y, PANEL_R, y + 34.0)));
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

    fn set_ocr(&mut self, on: bool) {
        self.ocr = on;
        self.reset_test();
        self.show_kids();
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
            Ctl::ShotFrame => self.shot_frame = !self.shot_frame,
            Ctl::GifFrame => self.gif_frame = !self.gif_frame,
            Ctl::FrameWidth(i) => self.frame_width = FRAME_CHOICES[i],
            Ctl::Longshot(i) => self.longshot_scroll = i == 1,
            Ctl::Ocr => self.set_ocr(!self.ocr),
            Ctl::Test => self.start_test(),
            Ctl::Models(ocr) => self.start_models(ocr),
            Ctl::Browse => self.browse(),
            Ctl::Target => self.menu(c),
            Ctl::Ok => return self.apply(),
            Ctl::Cancel => return self.w.close(),
            Ctl::Sv | Ctl::Hue => {}
        }
        self.w.invalidate_all();
    }

    /// Tab 顺序：导航（当前页）、本页的控件、底栏按钮；色块、分段、导航各算一站，站里用方向键
    fn order(&self) -> Vec<Focus> {
        let mut v = vec![Focus::Ctl(Ctl::Nav(self.page))];
        match self.page {
            GENERAL_PAGE => {
                v.extend([Focus::Child(Child::Hotkey), Focus::Child(Child::RecHotkey), Focus::Ctl(Ctl::Toggle), Focus::Ctl(Ctl::Swatch(self.swatch()))]);
                if self.picker {
                    v.push(Focus::Child(Child::Hex));
                }
            }
            CAPTURE_PAGE => v.extend([
                Focus::Ctl(Ctl::Browse),
                Focus::Ctl(Ctl::Format(self.format_index())),
                Focus::Ctl(Ctl::Fps(self.fps_index())),
                Focus::Ctl(Ctl::ShotFrame),
                Focus::Ctl(Ctl::GifFrame),
                Focus::Ctl(Ctl::FrameWidth(self.width_index())),
                Focus::Ctl(Ctl::Longshot(usize::from(self.longshot_scroll))),
            ]),
            _ => {
                v.extend([
                    Focus::Child(Child::Endpoint),
                    Focus::Child(Child::Model),
                    Focus::Ctl(Ctl::Models(false)),
                    Focus::Child(Child::Key),
                    Focus::Ctl(Ctl::Target),
                    Focus::Ctl(Ctl::Ocr),
                ]);
                if self.ocr {
                    v.extend([Focus::Child(Child::OcrEndpoint), Focus::Child(Child::OcrModel), Focus::Ctl(Ctl::Models(true)), Focus::Child(Child::OcrKey)]);
                }
                v.push(Focus::Ctl(Ctl::Test));
            }
        }
        if cfg!(target_os = "macos") {
            v.extend([Focus::Ctl(Ctl::Cancel), Focus::Ctl(Ctl::Ok)]);
        } else {
            v.extend([Focus::Ctl(Ctl::Ok), Focus::Ctl(Ctl::Cancel)]);
        }
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
        // 色块、分段、导航里换了哪一个都算同一站
        let same = |a: &Focus, b: &Focus| match (a, b) {
            (Focus::Ctl(x @ (Ctl::Nav(_) | Ctl::Swatch(_) | Ctl::Format(_) | Ctl::Fps(_) | Ctl::FrameWidth(_) | Ctl::Longshot(_))), Focus::Ctl(y)) => {
                std::mem::discriminant(x) == std::mem::discriminant(y)
            }
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
                Some(c @ (Ctl::Browse | Ctl::Test | Ctl::Models(_) | Ctl::Cancel | Ctl::Target)) => self.click(c),
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
                    Some(Ctl::FrameWidth(i)) => {
                        let j = (i as i32 + d).clamp(0, FRAME_CHOICES.len() as i32 - 1) as usize;
                        self.frame_width = FRAME_CHOICES[j];
                        self.focus_to(Focus::Ctl(Ctl::FrameWidth(j)));
                    }
                    Some(Ctl::Longshot(_)) => {
                        self.longshot_scroll = d > 0;
                        self.focus_to(Focus::Ctl(Ctl::Longshot(usize::from(self.longshot_scroll))));
                    }
                    Some(c @ Ctl::Target) if k == Key::Down => self.menu(c),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// 框里当前填的服务（还没保存）：`ocr` 为 true 时是识别文字实际会用的服务（开关关着就是翻译的）
    fn service(&self, ocr: bool) -> translate::Config {
        let f = |c| self.text(c).trim().to_string();
        let tr = translate::Config { endpoint: f(Child::Endpoint), model: f(Child::Model), api_key: f(Child::Key), target: self.target };
        if ocr && self.ocr { tr.overlay(&f(Child::OcrEndpoint), &f(Child::OcrModel), f(Child::OcrKey)) } else { tr }
    }

    fn model_box(&self, ocr: bool) -> Child {
        if ocr { Child::OcrModel } else { Child::Model }
    }

    /// 测试连接那一行开始转圈；返回这次请求的编号，结果回来时编号不对就丢掉
    fn busy(&mut self, msg: &'static str) -> u32 {
        self.test_id += 1;
        self.test = Test::Busy(msg);
        self.w.timer(T_SPIN, FRAME_MS);
        self.test_id
    }

    /// 测翻译服务；识别服务开关打开时再发一张图给识别服务，看模型收不收图片
    fn start_test(&mut self) {
        if matches!(self.test, Test::Busy(_)) {
            return;
        }
        let (tr, ocr) = (self.service(false), self.ocr.then(|| self.service(true)));
        if tr.endpoint.is_empty() || tr.model.is_empty() {
            self.test = Test::Done(false, "请先填写服务地址和模型".into());
            return;
        }
        let (id, w) = (self.busy("正在测试…"), self.w);
        std::thread::spawn(move || {
            let t0 = Instant::now();
            let result = translate::test_connection(&tr, translate::TIMEOUT).map_err(|e| e.to_string()).and_then(|_| match &ocr {
                Some(c) => translate::test_image(c, translate::TIMEOUT).map_err(|e| format!("识别服务：{e}")),
                None => Ok(()),
            });
            let msg = result.map(|_| format!("连接正常{}，用时 {:.1} 秒", if ocr.is_some() { "，识别模型能看图" } else { "" }, t0.elapsed().as_secs_f32()));
            w.post(move |ui: &mut Ui, _| ui.tested(id, msg));
        });
    }

    fn tested(&mut self, id: u32, result: Result<String, String>) {
        if id != self.test_id {
            return;
        }
        self.w.kill_timer(T_SPIN);
        self.test = match result {
            Ok(m) => Test::Done(true, m),
            Err(m) => Test::Done(false, m),
        };
        self.w.invalidate_all();
    }

    /// 「选择…」：用框里当前的地址和 Key 拉模型列表，拿到后弹下拉列表
    fn start_models(&mut self, ocr: bool) {
        if matches!(self.test, Test::Busy(_)) {
            return;
        }
        let cfg = self.service(ocr);
        if cfg.endpoint.is_empty() {
            self.test = Test::Done(false, "请先填写服务地址".into());
            return;
        }
        let (id, w) = (self.busy("正在获取模型…"), self.w);
        std::thread::spawn(move || {
            let result = translate::list_models(&cfg, translate::TIMEOUT);
            w.post(move |ui: &mut Ui, _| ui.listed(id, ocr, result));
        });
    }

    fn listed(&mut self, id: u32, ocr: bool, result: Result<Vec<String>, translate::Error>) {
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
                self.menu(Ctl::Models(ocr));
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
            shot_frame: self.shot_frame,
            gif_frame: self.gif_frame,
            frame_width: self.frame_width,
            longshot_scroll: self.longshot_scroll,
            sel_color: self.color,
            save_dir: self.save_dir.clone(),
            save_format: self.format,
            // 开关关着：识别沿用翻译服务，单独填的 Key 留在系统里不动
            ocr_endpoint: if self.ocr { self.text(Child::OcrEndpoint).trim().to_string() } else { String::new() },
            ocr_model: if self.ocr { self.text(Child::OcrModel).trim().to_string() } else { String::new() },
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
                && (c == Child::Key || self.ocr)
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
            Ctl::Models(ocr) => {
                r.left = self.field(self.model_box(ocr)).left;
                let typed = self.text(self.model_box(ocr));
                (self.models.clone(), self.models.iter().position(|m| *m == typed.trim()))
            }
            _ => (TARGETS.iter().map(|t| t.1.to_string()).collect(), Some(TARGETS.iter().position(|t| t.0 == self.target).unwrap_or(0))),
        };
        let items: Vec<MenuItem> = options
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let key = if matches!(c, Ctl::Models(_)) && t == translate::DEEPSEEK_MODEL { "推荐" } else { "" };
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
                    Ctl::Models(ocr) => ui.set_text(ui.model_box(ocr), &picked[id - 1]),
                    _ => ui.target = TARGETS[id - 1].0,
                }
                ui.w.invalidate_all();
            });
        });
    }

    fn draw(&self, c: &Canvas) -> Option<()> {
        let s = self.scale;
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
        let label = |t: &str, row: f32| put_mid(t, 13.0, true, self.px(LX), self.px(row), self.px(row + FIELD_H), ui::INK);
        // 说明字，折行到内容区右边；`mid` 有值时在这一行里上下居中
        let hint = |t: &str, x: f32, y: f32, mid: Option<f32>, color: u32| {
            let Some(l) = Text::wrapped(FONT, 12.0 * s, false, t, self.px(RIGHT) - x, None) else { return };
            let y = mid.map_or(y, |b| ((y + b - l.height()) / 2.0).round());
            c.text(&l, x, y, rgb(color));
        };
        let st = |ctl: Ctl| ui::state(true, false, self.hot == Some(ctl), self.down == Some(ctl));
        // 白底（或 `fill`）墨色描边的框：输入框、下拉框、分段、开关、色点
        let boxed = |r: RectF, radius: f32, fill: u32| ui::block(c, r, radius, 0.0, fill, s);
        // 带字按钮：主按钮紫底白字，其余白底墨字
        let button = |ctl: Ctl, t: &str, primary: bool| {
            let Some(r) = self.ctl_rect(ctl) else { return };
            let face = if primary { ui::primary(c, rf(&r), st(ctl), s) } else { ui::secondary(c, rf(&r), st(ctl), s) };
            let Some(l) = text(t, 13.0, true) else { return };
            let (x, y) = ((face.left + face.right - l.width()) / 2.0, (face.top + face.bottom - l.height()) / 2.0);
            c.text(&l, x.round(), y.round(), rgb(if primary { 0xFFFFFF } else { ui::INK }));
        };
        let dropdown = |ctl: Ctl, t: &str| {
            let Some(r) = self.ctl_rect(ctl) else { return };
            boxed(rf(&r), 7.0, if self.hot == Some(ctl) { ui::HOVER } else { ui::SURF });
            let Some(l) = text(t, 13.0, false) else { return };
            let icon_x = r.right as f32 - self.px(8.0) - self.px(16.0);
            l.truncate(icon_x - self.px(8.0) - r.left as f32 - self.px(10.0));
            c.text(&l, r.left as f32 + self.px(10.0), ((r.top + r.bottom) as f32 - l.height()) / 2.0, rgb(ui::INK));
            ui::icon(c, "chev_down", icon_x, (r.top + r.bottom) as f32 / 2.0 - 8.0 * s, 16.0 * s, rgb(ui::INK));
        };
        // 分段：白底墨色描边，选中那段墨底白字；右边跟一句说明。`ctl` 是第 i 段的控件
        let segments = |ctl: fn(usize) -> Ctl, labels: &[&str], sel: usize, note: &str| {
            let rs: Vec<Rect> = (0..labels.len()).filter_map(|i| self.ctl_rect(ctl(i))).collect();
            let (Some(a), Some(b)) = (rs.first(), rs.last()) else { return };
            let p = self.px(3.0);
            let whole = rectf(a.left as f32 - p, a.top as f32 - p, b.right as f32 + p, a.bottom as f32 + p);
            boxed(whole, 7.0, ui::SURF);
            for (i, r) in rs.iter().enumerate() {
                let on = i == sel;
                if on || self.hot == Some(ctl(i)) {
                    c.fill_round(rf(r), 5.0 * s, rgb(if on { ui::INK } else { ui::HOVER }));
                }
                let Some(l) = text(labels[i], 12.5, true) else { continue };
                let (x, y) = ((r.left + r.right) as f32 / 2.0 - l.width() / 2.0, (r.top + r.bottom) as f32 / 2.0 - l.height() / 2.0);
                c.text(&l, x.round(), y.round(), rgb(if on { 0xFFFFFF } else { ui::MUTED }));
            }
            put_mid(note, 12.0, false, whole.right + self.px(10.0), whole.top, whole.bottom, ui::MUTED);
        };
        // 开关 44 × 24：关是白底，开是紫底，白色圆点墨色描边；右边一句说明，和这一行 `row` 对齐
        let switch = |ctl: Ctl, on: bool, note: &str, row: f32| {
            let Some(r) = self.ctl_rect(ctl) else { return };
            boxed(rf(&r), 12.0, if on { ui::ACCENT } else if self.hot == Some(ctl) { ui::HOVER } else { ui::SURF });
            let (d, y) = (18.0 * s, (r.top + r.bottom) as f32 / 2.0 - 9.0 * s);
            let x = r.left as f32 + if on { 23.0 } else { 3.0 } * s;
            boxed(rectf(x, y, x + d, y + d), 9.0, ui::SURF);
            put_mid(note, 13.0, false, r.right as f32 + self.px(10.0), self.px(row), self.px(row + FIELD_H), ui::INK);
        };
        let focus = self.w.focused_field();
        // 两个快捷键框下面共用一行字：有问题的写原因（红框标在那个框上），正在按的写要求，改过的提示点确定
        let hotkey_hint = || {
            let keys = [(Child::Hotkey, self.orig.hotkey), (Child::RecHotkey, self.orig.record_hotkey)];
            let typing = keys.iter().any(|(ch, _)| focus == Some(*ch as usize));
            let changed = keys.iter().any(|(ch, orig)| self.read_hotkey(*ch).is_some_and(|h| h != *orig));
            let (msg, color) = match &self.hk_bad {
                Some((_, m)) => (m.as_str(), ERR_TEXT),
                None if typing => (if cfg!(target_os = "macos") { "请按下新的组合键，需要包含 ⌃、⌥ 或 ⌘" } else { "请按下新的组合键，需要包含 Ctrl 或 Alt" }, ui::MUTED),
                None if changed => ("点「确定」后生效", ui::MUTED),
                None => ("点一下输入框，再按新的组合键；两个不能相同", ui::MUTED),
            };
            hint(msg, self.px(CX), self.px(REC_Y + 38.0), None, color);
        };
        // 「测试连接」和右边的结果
        let test_row = || -> Option<()> {
            button(Ctl::Test, "测试连接", false);
            let r = self.ctl_rect(Ctl::Test)?;
            let x = r.right as f32 + self.px(12.0);
            let (top, bottom) = (r.top as f32, r.bottom as f32);
            match &self.test {
                Test::Idle => {}
                Test::Busy(msg) => {
                    ui::spinner(c, x, ((top + bottom) / 2.0 - 8.0 * s).round(), s, self.angle, rgb(ui::HOVER), rgb(ui::ACCENT));
                    put_mid(msg, 12.0, false, x + self.px(22.0), top, bottom, ui::MUTED);
                }
                Test::Done(ok, msg) => {
                    // 第一行和按钮文字对齐，长的错误原因往下折行
                    let l = Text::wrapped(FONT, 12.0 * s, true, msg, self.px(RIGHT) - x, None)?;
                    let h = l.first_line_height();
                    c.text(&l, x, ((top + bottom - h) / 2.0).round(), rgb(if *ok { OK_TEXT } else { ERR_TEXT }));
                }
            }
            Some(())
        };

        c.clear(rgb(ui::CREAM));
        // 左边导航：当前页柠檬黄块加 2,2 实心阴影
        for (i, (name, icon)) in PAGES.iter().enumerate() {
            let Some(r) = self.ctl_rect(Ctl::Nav(i)) else { continue };
            let on = i == self.page;
            if on {
                ui::block(c, rf(&r), 7.0, 2.0, ui::PICK, s);
            } else if self.hot == Some(Ctl::Nav(i)) {
                c.fill_round(rf(&r), 7.0 * s, rgb(if self.down == Some(Ctl::Nav(i)) { ui::PRESS } else { ui::HOVER }));
            }
            let fg = if on { ui::INK } else { ui::MUTED };
            ui::icon(c, icon, r.left as f32 + self.px(12.0), (r.top + r.bottom) as f32 / 2.0 - 10.0 * s, 20.0 * s, rgb(fg));
            put_mid(name, 13.5, true, r.left as f32 + self.px(42.0), r.top as f32, r.bottom as f32, fg);
        }
        // 右边白色面板和页标题
        ui::block(c, rectf(self.px(NAV_W), self.px(4.0), self.px(PANEL_R), self.px(MAIN_H)), 12.0, 4.0, ui::SURF, s);
        put(PAGES[self.page].0, 20.0, true, self.px(LX), self.px(24.0), ui::INK);
        match self.page {
            GENERAL_PAGE => {
                label("截图快捷键", ROW0);
                label("录屏快捷键", REC_Y);
                hotkey_hint();
                label("开机启动", AUTO_Y);
                switch(Ctl::Toggle, self.autostart, "开机时自动启动 Jie", AUTO_Y);
                label("选区颜色", SW_Y);
                let cur = self.swatch();
                for i in 0..8 {
                    let Some(r) = self.ctl_rect(Ctl::Swatch(i)) else { continue };
                    if self.hot == Some(Ctl::Swatch(i)) {
                        c.fill_round(rf(&r), 7.0 * s, rgb(ui::HOVER));
                    }
                    let (cx, cy) = ((r.left + r.right) as f32 / 2.0, (r.top + r.bottom) as f32 / 2.0);
                    // 最后一格：没选自定义颜色时是加号
                    match PRESETS.get(i).map(|p| p.1).or((cur == 7).then_some(self.color)) {
                        Some(col) => boxed(rectf(cx - 10.0 * s, cy - 10.0 * s, cx + 10.0 * s, cy + 10.0 * s), 10.0, col),
                        None => ui::icon(c, "plus", cx - 10.0 * s, cy - 10.0 * s, 20.0 * s, rgb(ui::INK)),
                    }
                    if i == cur {
                        // 选中：隔 2 的白，再一圈 2 墨色
                        c.stroke_ellipse(cx, cy, 13.0 * s, 13.0 * s, rgb(ui::INK), ui::line_w(s), Stroke::Plain);
                    }
                }
                if self.picker {
                    self.draw_picker(c)?;
                } else {
                    hint("用在选区边框、手柄、窗口高亮、录屏边框和钉图描边上", self.px(CX), self.px(SW_Y + 40.0), None, ui::MUTED);
                }
            }
            CAPTURE_PAGE => {
                label("保存位置", ROW0);
                button(Ctl::Browse, "浏览…", false);
                hint("每次保存后会自动记成上次用的文件夹", self.px(CX), self.px(ROW0 + 38.0), None, ui::MUTED);
                label("图片格式", 136.0);
                segments(Ctl::Format, &FORMATS, self.format_index(), "JPEG 质量固定 90");
                label("录屏帧率", 180.0);
                let fps = FPS_CHOICES.map(|f| f.to_string());
                segments(Ctl::Fps, &fps.each_ref().map(String::as_str), self.fps_index(), "帧 / 秒");
                hint("越高越流畅，GIF 也越大；区域大、画面变化多时实际帧率会低一些。", self.px(CX), self.px(218.0), None, ui::MUTED);
                label("截图边框", SHOT_FRAME_Y);
                switch(Ctl::ShotFrame, self.shot_frame, "截好的图外面画一圈选区颜色的框", SHOT_FRAME_Y);
                label("动图边框", GIF_FRAME_Y);
                switch(Ctl::GifFrame, self.gif_frame, "录好的 GIF 外面画一圈选区颜色的框", GIF_FRAME_Y);
                label("边框粗细", WIDTH_Y);
                segments(Ctl::FrameWidth, &FRAME_NAMES, self.width_index(), "录屏时区域外的框也一样粗");
                hint("复制、保存出去的图片和 GIF 也带这圈框", self.px(CX), self.px(WIDTH_Y + 38.0), None, ui::MUTED);
                label("长图钉图", LONGSHOT_Y);
                segments(Ctl::Longshot, &LONGSHOT_NAMES, usize::from(self.longshot_scroll), "");
                let text = if self.longshot_scroll { "原窗口大小，上下滚动查看；导出完整长图" } else { "缩小显示完整长图；不影响复制、保存" };
                hint(text, self.px(CX), self.px(LONGSHOT_Y + 38.0), None, ui::MUTED);
            }
            _ => {
                label("服务地址", ROW0);
                label("模型", ROW0 + 44.0);
                button(Ctl::Models(false), "选择…", false);
                label("API Key", ROW0 + 88.0);
                label("译成", ROW0 + 132.0);
                dropdown(Ctl::Target, self.target_label());
                if let Some(r) = self.ctl_rect(Ctl::Target) {
                    hint(self.target_hint(), r.right as f32 + self.px(10.0), r.top as f32, Some(r.bottom as f32), ui::MUTED);
                }
                label("文字识别", OCR_Y);
                switch(Ctl::Ocr, self.ocr, "另用一家服务识别文字", OCR_Y);
                if self.ocr {
                    label("识别地址", OCR_Y + 44.0);
                    label("识别模型", OCR_Y + 88.0);
                    button(Ctl::Models(true), "选择…", false);
                    label("识别 Key", OCR_Y + 132.0);
                } else {
                    hint("关着时用上面的服务识别文字，模型要能看图", self.px(CX), self.px(OCR_Y + 38.0), None, ui::MUTED);
                }
                test_row()?;
            }
        }
        // 系统控件的外框（画在页面内容之后，取色器的底色才不会盖住色值框）：白底墨色描边，聚焦时外面一圈浅紫，快捷键有问题时一圈红
        for ch in CHILDREN.into_iter().filter(|ch| self.visible(*ch)) {
            let r = rf(&self.field(ch));
            let bad = self.hk_bad.as_ref().is_some_and(|(c, _)| *c == ch);
            if bad || focus == Some(ch as usize) {
                let g = self.px(3.0);
                let o = rectf(r.left - g / 2.0, r.top - g / 2.0, r.right + g / 2.0, r.bottom + g / 2.0);
                c.stroke_round(o, 7.0 * s + g / 2.0, rgb(if bad { ui::ERR } else { ui::PRESS }), g);
            }
            boxed(r, 7.0, ui::SURF);
        }
        // 底栏
        put_mid(concat!("Jie ", env!("CARGO_PKG_VERSION")), 12.0, false, self.px(20.0), self.px(MAIN_H + 4.0), self.px(MAIN_H + FOOT_H), ui::MUTED);
        button(Ctl::Ok, "确定", true);
        button(Ctl::Cancel, "取消", false);
        // 键盘焦点框：2 主题紫，离控件 2
        if self.kbd
            && self.w.focused()
            && let Some(ctl) = self.focus
            && let Some(r) = self.ctl_rect(ctl)
        {
            let (g, lw) = (self.px(3.0), ui::line_w(s));
            let o = rectf(r.left as f32 - g, r.top as f32 - g, r.right as f32 + g, r.bottom as f32 + g);
            let radius = match ctl {
                Ctl::Swatch(_) => 16.0,
                Ctl::Toggle | Ctl::ShotFrame | Ctl::GifFrame | Ctl::Ocr => 14.0,
                _ => 9.0,
            };
            c.stroke_round(rectf(o.left + lw / 2.0, o.top + lw / 2.0, o.right - lw / 2.0, o.bottom - lw / 2.0), radius * s, rgb(ui::ACCENT), lw);
        }
        Some(())
    }

    /// 取色器：饱和度/明度方块 + 色相条 + 预览色块（色值输入框是系统控件）
    fn draw_picker(&self, c: &Canvas) -> Option<()> {
        let s = self.scale;
        ui::block(c, rf(&self.rect(CX, PICK_Y, CX + 222.0, PICK_Y + 198.0)), 7.0, 0.0, ui::SURF, s);
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
        ui::block(c, rf(&self.rect(CX + 11.0, PICK_Y + 159.0, CX + 39.0, PICK_Y + 187.0)), 5.0, 0.0, self.color, s);
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
    use std::time::Duration;

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
        ui.set_page(SERVICE_PAGE);
        assert_eq!((ui.text(Child::Endpoint).as_str(), ui.text(Child::Model).as_str()), (translate::DEEPSEEK, translate::DEEPSEEK_MODEL));

        // 菜单弹出来以后另一个线程截屏，再按 ↓↓ Enter 选第二个
        let body = r#"{"object":"list","data":[{"id":"deepseek-flash"},{"id":"deepseek-v4-pro"}]}"#;
        let (base, _req) = mock(Some(http("200 OK", body)));
        ui.set_text(Child::Endpoint, &base);
        let picker = plat::answer_menu(w, Some(dir.join("models-menu.png")), &[Key::Down, Key::Down, Key::Enter]);
        w.with(|ui: &mut Ui| ui.click(Ctl::Models(false)));
        wait(ui, |u| u.text(Child::Model) == "deepseek-v4-pro");
        picker.join().unwrap();
        assert!(matches!(ui.test, Test::Idle));

        // Key 错：原因写在测试连接那一行
        let (bad, _req) = mock(Some(http("401 Unauthorized", r#"{"error":{"message":"bad key"}}"#)));
        ui.set_text(Child::Endpoint, &bad);
        w.with(|ui: &mut Ui| ui.click(Ctl::Models(false)));
        wait(ui, |u| matches!(&u.test, Test::Done(false, m) if m == "API Key 无效（401），请到设置里检查。"));
        w.close();
    }

    /// 停一会儿让窗口画完
    fn settle(ui: &Ui) {
        let t0 = Instant::now();
        wait(ui, |_| t0.elapsed() > Duration::from_millis(300));
    }

    /// 识别服务：开关打开才多出三项 → 测试连接先测翻译服务、再发图片给识别服务（用那边的模型和 Key）→
    /// 识别地址、Key 留空时图片发给翻译服务 → 模型不收图片时写原因 → 关掉开关三项藏起来。
    /// 截图放 target/test-tmp/settings-ocr，平时不跑：cargo test ocr_service -- --ignored
    #[test]
    #[ignore]
    fn ocr_service() {
        use crate::translate::tests::{http, mock};
        let (w, ui, dir) = open("settings-ocr");
        w.with(|ui: &mut Ui| ui.set_page(SERVICE_PAGE));
        assert!(!ui.ocr && !ui.visible(Child::OcrEndpoint));
        w.with(|ui: &mut Ui| ui.click(Ctl::Ocr));
        settle(ui);
        shoot(w.rect(), &dir.join("1-page.png"));
        assert!(ui.visible(Child::OcrEndpoint) && ui.visible(Child::OcrKey));

        let ok = r#"{"choices":[{"message":{"content":""}}]}"#;
        let (tr, tr_req) = mock(Some(http("200 OK", ok)));
        ui.set_text(Child::Endpoint, &tr);
        ui.set_text(Child::Key, "sk-tr-test");
        let (base, req) = mock(Some(http("200 OK", ok)));
        ui.set_text(Child::OcrEndpoint, &base);
        ui.set_text(Child::OcrModel, "vl-1");
        ui.set_text(Child::OcrKey, "sk-ocr-test");
        w.with(|ui: &mut Ui| ui.click(Ctl::Test));
        wait(ui, |u| matches!(&u.test, Test::Done(..)));
        assert!(matches!(&ui.test, Test::Done(true, m) if m.starts_with("连接正常，识别模型能看图")));
        let r = tr_req.recv().unwrap();
        assert!(!r.contains("data:image") && r.to_lowercase().contains("bearer sk-tr-test"), "{r}");
        let r = req.recv().unwrap();
        assert!(r.contains("data:image/png;base64,") && r.contains("\"vl-1\"") && r.to_lowercase().contains("bearer sk-ocr-test"), "{r}");

        // 地址、Key 留空：图片也发给翻译服务，带翻译的 Key
        ui.set_text(Child::OcrEndpoint, "");
        ui.set_text(Child::OcrKey, "");
        w.with(|ui: &mut Ui| ui.click(Ctl::Test));
        wait(ui, |u| matches!(&u.test, Test::Done(true, _)));
        let _ping = tr_req.recv().unwrap();
        let r = tr_req.recv().unwrap();
        assert!(r.contains("data:image") && r.contains("\"vl-1\"") && r.to_lowercase().contains("bearer sk-tr-test"), "{r}");

        // 模型不收图片
        let (bad, _req) = mock(Some(http("400 Bad Request", r#"{"error":{"message":"image input is not supported by this model"}}"#)));
        ui.set_text(Child::OcrEndpoint, &bad);
        ui.set_text(Child::OcrKey, "sk-ocr-test");
        w.with(|ui: &mut Ui| ui.click(Ctl::Test));
        wait(ui, |u| matches!(&u.test, Test::Done(false, m) if m.starts_with("识别服务：") && m.contains("image input is not supported")));
        settle(ui);
        shoot(w.rect(), &dir.join("2-no-image.png"));

        // 关掉开关：三项藏起来，测试结果清掉
        w.with(|ui: &mut Ui| ui.click(Ctl::Ocr));
        assert!(!ui.visible(Child::OcrEndpoint) && matches!(ui.test, Test::Idle));
        w.close();
    }

    /// 截图与录屏页、常规页：截图存 target/test-tmp/settings-rec；帧率分段可以用方向键换；
    /// 录屏快捷键和截图的一样时「确定」被拦下（这时还没注册任何快捷键），翻到常规页、红框标在录屏快捷键上。
    /// 平时不跑：cargo test capture_page -- --ignored
    #[test]
    #[ignore]
    fn capture_page() {
        let (w, ui, dir) = open("settings-rec");
        w.with(|ui: &mut Ui| ui.set_page(CAPTURE_PAGE));
        settle(ui);
        shoot(w.rect(), &dir.join("1-page.png"));
        assert!(ui.visible(Child::SaveDir) && !ui.visible(Child::Hotkey));
        assert_eq!(ui.fps, 15);
        w.with(|ui: &mut Ui| {
            ui.focus = Some(Ctl::Fps(1));
            ui.key(Key::Right, false, true);
        });
        assert_eq!(ui.fps, 20);
        // 截图边框开关、边框粗细分段（方向键换一档）
        assert!(!ui.shot_frame && ui.gif_frame && ui.frame_width == 2);
        w.with(|ui: &mut Ui| {
            ui.click(Ctl::ShotFrame);
            ui.focus = Some(Ctl::FrameWidth(1));
            ui.key(Key::Left, false, true);
        });
        assert!(ui.shot_frame && ui.frame_width == 1 && ui.focus == Some(Ctl::FrameWidth(0)));
        // 长图显示方式复用分段控件，鼠标、方向键和 Tab 顺序都要可用。
        w.with(|ui: &mut Ui| {
            ui.click(Ctl::Longshot(1));
            ui.focus = Some(Ctl::Longshot(1));
            ui.key(Key::Left, false, true);
        });
        assert!(!ui.longshot_scroll && ui.focus == Some(Ctl::Longshot(0)));
        w.with(|ui: &mut Ui| ui.key(Key::Right, false, true));
        assert!(ui.longshot_scroll && ui.order().contains(&Focus::Ctl(Ctl::Longshot(1))));
        settle(ui);
        shoot(w.rect(), &dir.join("1-frames.png"));
        let hk = ui.orig.hotkey;
        w.set_hotkey_field(Child::RecHotkey as usize, hk.mods, hk.vk);
        w.with(|ui: &mut Ui| ui.apply());
        assert!(matches!(&ui.hk_bad, Some((Child::RecHotkey, m)) if m.contains("重复")));
        assert_eq!(ui.page, GENERAL_PAGE);
        settle(ui);
        shoot(w.rect(), &dir.join("2-duplicate.png"));
        w.with(|ui: &mut Ui| ui.click(Ctl::Swatch(7)));
        settle(ui);
        shoot(w.rect(), &dir.join("3-picker.png"));
        w.close();
    }
}
