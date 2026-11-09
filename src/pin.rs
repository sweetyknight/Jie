//! 钉图：置顶、无边框，出现在选区原来的位置，100% 大小。
//! 保存原图和标注对象（不合成死图），标注在绘制时叠上去；缩小时另存一张显示用的图，内存不超过原图的 2 倍。
//! 鼠标停在钉图上时下面挂出悬停条（编辑、取字、翻译、复制、保存、关闭），离开钉图和悬停条 300ms 后收起；右键菜单也留着。
//! 「编辑」在钉图上接着标注：鼠标坐标按缩放换成原图像素交给标注编辑器，操作栏是贴在旁边的单独小窗口（bar.rs）。
//!
//! 动图钉图（录屏结果）：从 GIF 文件逐帧解码，叠到 `img` 上循环播放，内存里只有当前这一帧；不能编辑标注，复制、保存的是 GIF 文件，
//! 悬停条第一个按钮是暂停/播放。
//! 刚录完时是「结果」状态：下面挂结果条（复制、保存、钉住、丢弃），复制或保存以后就关掉，钉住以后是普通的动图钉图。
//! 钉图不知道录屏：录屏只管把文件交过来（`create_gif`）。

mod bar;

use crate::anim::Player;
use crate::annotate::{Doc, Editor, Pt};
use crate::image_io;
use crate::plat::{self, Affine, Bitmap, Button, Canvas, Cursor, Event, Handler, Image, Key, MenuItem, Mods, Point, Rect, Spec, Win};
use crate::toast::{self, Kind};
use crate::toolbar::{self, Act, Items, opts::Item, tool_of};
use crate::ui::{self, Hud, rectf};
use std::path::PathBuf;

/// 读数在最后一次滚轮 1 秒后消失
const HUD_TIMER: usize = 1;
/// 动图换下一帧
const ANIM_TIMER: usize = 2;
/// 鼠标离开钉图后隔一会儿看还在不在钉图或悬停条上
const HOVER_TIMER: usize = 3;

const CMD_EDIT: usize = 1;
const CMD_OCR: usize = 2;
const CMD_TRANSLATE: usize = 3;
const CMD_COPY: usize = 4;
const CMD_SAVE: usize = 5;
const CMD_ORIGINAL: usize = 6;
const CMD_CLOSE: usize = 7;
const CMD_PLAY: usize = 8;

/// 拖动：移动钉图（按下时的鼠标位置和窗口位置，屏幕坐标），或者交给标注编辑器（画图形、移动标注）
#[derive(Clone, Copy)]
enum Drag {
    None,
    Move(Point, Point),
    Edit,
}

struct Pin {
    /// 原图，物理像素
    img: Bitmap,
    /// 标注，坐标是原图像素
    ed: Editor,
    /// 缩小时显示用的图：(对应的缩放百分比, 图)
    view: Option<(i32, Bitmap)>,
    /// 缩放百分比 10–500，不透明度百分比 20–100
    zoom: i32,
    opacity: i32,
    hud: Option<String>,
    /// 滚轮累计量：触控板一次只给一点，攒够 120 才走一格
    wheel_acc: i32,
    drag: Drag,
    /// 编辑中、录屏结果、鼠标停在上面：操作栏窗口
    chrome: Option<Win>,
    /// 操作栏是悬停条
    hovering: bool,
    /// 动图钉图
    anim: Option<Anim>,
    /// 录屏结果：下面挂着结果条
    preview: bool,
    /// 动图钉图外面那圈框的粗细（像素，设置里关了是 0）；建好就定下，改设置不影响已经开着的
    frame: i32,
}

/// 动图：播放器（帧画在 `img` 上）、GIF 文件、是不是在播
struct Anim {
    player: Player,
    path: PathBuf,
    playing: bool,
}

fn open(img: Bitmap, at: Point, ed: Editor, anim: Option<Anim>, preview: bool) -> Option<Win> {
    let (w, h) = (img.w, img.h);
    // 和录制时的边框一样粗（3 DIP），录完框还在原来的位置
    let frame = if anim.is_some() && crate::settings::get().gif_frame { (3.0 * plat::monitor_at(at).scale()).round().max(2.0) as i32 } else { 0 };
    let pin = Pin { img, ed, view: None, zoom: 100, opacity: 100, hud: None, wheel_acc: 0, drag: Drag::None, chrome: None, hovering: false, anim, preview, frame };
    // 图片左上角在 `at`，框画在外面
    let rect = Rect { left: at.x - frame, top: at.y - frame, right: at.x + w + frame, bottom: at.y + h + frame };
    let win = Win::open(Spec { kind: plat::Kind::Pin, rect, title: "Jie 钉图", owner: None }, pin)?;
    win.show();
    Some(win)
}

/// 新建钉图；`at` 是图片左上角的屏幕坐标，`ed` 带着截图时画的标注
pub fn create(img: Bitmap, at: Point, ed: Editor) {
    open(img, at, ed, None, false);
}

/// 动图钉图：GIF 文件 `path` 钉在 `at`（屏幕坐标）开始循环播放；`result` 有值时是录屏结果，下面挂结果条，
/// 结果条最左边写这段说明（时长和大小）。钉图开着期间文件要一直在（逐帧从文件解码，复制到剪贴板的也是这个文件）
pub fn create_gif(path: PathBuf, at: Point, result: Option<String>) -> Result<(), String> {
    let preview = result.is_some();
    let mut player = Player::open(&path)?;
    let mut img = Bitmap::new(player.w, player.h).ok_or("内存不够，打不开动图")?;
    let delay = player.next(img.pixels_mut())?;
    let ed = Editor::new(Doc::default(), plat::monitor_at(at).scale());
    let win = open(img, at, ed, Some(Anim { player, path, playing: true }), preview).ok_or("打不开动图窗口")?;
    win.timer(ANIM_TIMER, delay.as_millis().max(10) as u32);
    if let Some(info) = result {
        win.with(|p: &mut Pin| p.show_result(win, info));
    }
    Ok(())
}

/// 托盘菜单：当前钉图数量
pub fn count() -> usize {
    Win::all(plat::Kind::Pin).len()
}

/// 托盘菜单「关闭所有钉图」
pub fn close_all() {
    Win::all(plat::Kind::Pin).into_iter().for_each(Win::close);
}

/// 操作栏显示的状态：工具、选项条、颜色粗细、能不能撤销重做；变了就重画操作栏
type Ui = (Option<crate::annotate::Tool>, Option<(crate::annotate::Kind, bool)>, (usize, usize), bool, bool);

fn ui(e: &Editor) -> Ui {
    (e.tool, e.opts(), e.pen(), e.doc.can_undo(), e.doc.can_redo())
}

/// 客户区坐标 → 原图像素：去掉描边 b，按缩放百分比除回去
fn to_img(p: Point, b: i32, zoom: i32) -> Pt {
    let f = |v: i32| (v - b) as f32 * 100.0 / zoom as f32;
    Pt::new(f(p.x), f(p.y))
}

/// 原图像素范围 → 客户区：按缩放乘上去、加上描边，向外取整再多留 1 像素抗锯齿
fn to_client(r: Rect, b: i32, zoom: i32) -> Rect {
    let k = zoom as f32 / 100.0;
    let (lo, hi) = (|v: i32| (v as f32 * k).floor() as i32 + b - 1, |v: i32| (v as f32 * k).ceil() as i32 + b + 1);
    Rect { left: lo(r.left), top: lo(r.top), right: hi(r.right), bottom: hi(r.bottom) }
}

/// 文件大小「1.3 MB」「820 KB」
fn size_text(path: &std::path::Path) -> String {
    let n = std::fs::metadata(path).map_or(0, |m| m.len());
    if n >= 1 << 20 { format!("{:.1} MB", n as f64 / (1u64 << 20) as f64) } else { format!("{} KB", n.div_ceil(1024)) }
}

impl Pin {
    /// 当前缩放下图片的显示尺寸
    fn shown(&self) -> (i32, i32) {
        let f = |v: i32| ((v * self.zoom + 50) / 100).max(1);
        (f(self.img.w), f(self.img.h))
    }

    /// 在编辑标注（录屏结果、悬停条不算）
    fn editing(&self) -> bool {
        self.chrome.is_some() && !self.preview && !self.hovering
    }

    /// 图片外的选区色描边：编辑时 2px；动图按设置一直有框（见 `frame`）；平时没有
    fn border(&self) -> i32 {
        if self.editing() { 2 } else { self.frame }
    }

    fn to_img(&self, p: Point) -> Pt {
        to_img(p, self.border(), self.zoom)
    }

    /// 原图像素 → 客户区（放候选窗、重画输入框用）
    fn client_of(&self) -> impl Fn(Pt) -> Pt + use<> {
        let (k, b) = (self.zoom as f32 / 100.0, self.border() as f32);
        move |p| Pt::new(p.x * k + b, p.y * k + b)
    }

    /// 客户区的点在不在图片上
    fn inside(&self, p: Point) -> bool {
        let ((iw, ih), b) = (self.shown(), self.border());
        p.x >= b && p.x < b + iw && p.y >= b && p.y < b + ih
    }

    /// 图片区域的屏幕坐标
    fn image_rect(&self, w: Win) -> Rect {
        let (r, b) = (w.rect(), self.border());
        Rect { left: r.left + b, top: r.top + b, right: r.right - b, bottom: r.bottom - b }
    }

    /// 读数：图片右下角往里 8
    fn hud(&self, w: Win) -> Option<(Hud, Rect)> {
        let s = w.scale();
        let hud = Hud::readout(&[(self.hud.as_deref()?, ui::INK)], s)?;
        let ((iw, ih), b) = (self.shown(), self.border());
        let g = (8.0 * s).round() as i32;
        let (x, y) = (b + iw - g - hud.w, b + ih - g - hud.h);
        let r = Rect { left: x, top: y, right: x + hud.w, bottom: y + hud.h };
        Some((hud, r))
    }

    /// 画描边和图片，原点挪到 (ox, oy)
    fn draw_body(&self, c: &Canvas, ox: i32, oy: i32) {
        let ((iw, ih), b) = (self.shown(), self.border());
        let editing = self.editing();
        let color = crate::settings::sel_color();
        // 描边一圈一圈从外往里画
        for i in 0..b {
            ui::frame_px(c, Rect { left: i - ox, top: i - oy, right: iw + 2 * b - i - ox, bottom: ih + 2 * b - i - oy }, color);
        }
        let (x, y) = (b - ox, b - oy);
        match &self.view {
            _ if self.zoom == 100 => c.blit(&self.img, self.img.full(), x, y),
            Some((z, v)) if *z == self.zoom => c.blit(v, v.full(), x, y),
            // 放大：最近邻，像素清楚；缩小图还没做出来时也先这样顶着
            _ => c.blit_scaled(&self.img, Rect { left: x, top: y, right: x + iw, bottom: y + ih }),
        }
        // 标注按缩放叠上去，裁在图片里；只画看得见的那块，放大到 500% 也不会分配整张大图。编辑时带选中框和正在输入的文字
        let a = c.area();
        let r = Rect { left: a.left.max(x), top: a.top.max(y), right: a.right.min(x + iw), bottom: a.bottom.min(y + ih) };
        if (!self.ed.doc.anns.is_empty() || self.ed.text.is_some()) && r.right > r.left && r.bottom > r.top {
            let k = self.zoom as f32 / 100.0;
            c.push_clip(rectf(r.left as f32, r.top as f32, r.right as f32, r.bottom as f32));
            c.set_transform(Affine { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: x as f32, M32: y as f32 });
            self.ed.draw(c, &self.img, editing.then(crate::settings::sel_color));
            c.set_transform(Affine::identity());
            c.pop_clip();
        }
    }

    /// 导出用：原图连同标注
    fn export(&self) -> Option<Image> {
        let out = self.img.sub(&self.img.full())?;
        self.ed.burn(&out, 0, 0, &self.img);
        Some(out.crop(&out.full()))
    }

    fn show_hud(&mut self, w: Win, text: String) {
        self.hud = Some(text);
        w.timer(HUD_TIMER, 1000);
        w.invalidate_all();
    }

    /// 缩放到 zoom%，`anchor`（屏幕坐标）下的那个像素保持不动
    fn set_zoom(&mut self, w: Win, zoom: i32, anchor: Point) {
        let zoom = zoom.clamp(10, 500);
        let (wr, b) = (w.rect(), self.border());
        let (ix, iy) = (wr.left + b, wr.top + b);
        let k = zoom as f32 / self.zoom as f32;
        let nx = anchor.x - ((anchor.x - ix) as f32 * k).round() as i32;
        let ny = anchor.y - ((anchor.y - iy) as f32 * k).round() as i32;
        self.zoom = zoom;
        let (iw, ih) = self.shown();
        // 缩小时换一张高质量缩小的显示图；放大和 100% 直接用原图
        self.view = if zoom < 100 { plat::scale_down(&self.img, iw, ih).map(|v| (zoom, v)) } else { None };
        w.set_rect(Rect { left: nx - b, top: ny - b, right: nx - b + iw + 2 * b, bottom: ny - b + ih + 2 * b });
        self.show_hud(w, format!("{zoom}%"));
        self.ed.zoom = zoom as f32 / 100.0;
        self.place_bar(w);
    }

    fn wheel(&mut self, w: Win, delta: i32, p: Point, mods: Mods) {
        self.wheel_acc += delta;
        let notches = self.wheel_acc / 120;
        if notches == 0 {
            return;
        }
        self.wheel_acc %= 120;
        let step = notches * 10;
        // 编辑中先提交正在输入的文字
        self.update(w, |s| s.ed.finish_text());
        if mods.cmd {
            self.opacity = (self.opacity + step).clamp(20, 100);
            w.set_alpha((self.opacity * 255 / 100) as f32 / 255.0);
            self.show_hud(w, format!("不透明度 {}%", self.opacity));
        } else {
            self.set_zoom(w, self.zoom + step, p);
        }
    }

    fn copy(&mut self, w: Win) {
        // 动图复制的是文件；录屏结果复制完就关掉，和截图一样
        if let Some(a) = &self.anim {
            match plat::copy_file(&a.path) {
                Ok(()) => toast::show(&w.monitor(), Kind::Ok, &format!("已复制 GIF（{}）", size_text(&a.path))),
                Err(e) => toast::show(&w.monitor(), Kind::Err, &format!("复制失败：{e}")),
            }
            if self.preview {
                w.close();
            }
            return;
        }
        // 正在输入的文字先提交
        self.update(w, |s| s.ed.finish_text());
        let Some(img) = self.export() else { return };
        match plat::copy_image(&img) {
            Ok(()) => toast::show(&w.monitor(), Kind::Ok, &format!("已复制 {} × {}", img.w, img.h)),
            Err(e) => toast::show(&w.monitor(), Kind::Err, &format!("复制失败：{e}")),
        }
    }

    /// 编辑时改标注都走这里：变了的标注新旧范围都重画；工具、选项条、颜色粗细、撤销重做变了就重画操作栏
    fn update(&mut self, w: Win, f: impl FnOnce(&mut Self)) {
        let before = (self.ed.looks(), ui(&self.ed));
        f(self);
        self.updated(w, before);
    }

    /// `update` 改完以后：和改之前比，标重画。不跟着闭包泛型，免得每个调用点复制一份
    #[inline(never)]
    fn updated(&mut self, w: Win, (l0, u0): (Vec<(Rect, usize, usize)>, Ui)) {
        let l1 = self.ed.looks();
        let (b, z) = (self.border(), self.zoom);
        for i in 0..l0.len().max(l1.len()) {
            if l0.get(i) != l1.get(i) {
                for (r, ..) in [l0.get(i), l1.get(i)].into_iter().flatten() {
                    w.invalidate(to_client(*r, b, z));
                }
            }
        }
        if ui(&self.ed) != u0 {
            self.place_bar(w);
        }
        w.ime(self.ed.caret(self.client_of()));
    }

    /// 建操作栏窗口，按钮点了交回这个钉图
    fn open_bar(&mut self, w: Win, items: &'static Items, info: Option<String>) {
        self.chrome = bar::open(w, items, info, |pin, b| _ = pin.with(|p: &mut Pin| p.act(pin, b)));
        self.place_bar(w);
    }

    /// 悬停条的按钮：动图播放中、暂停时不一样
    fn hover_items(&self) -> &'static Items {
        match &self.anim {
            Some(a) if a.playing => &toolbar::HOVER_GIF,
            Some(_) => &toolbar::HOVER_GIF_PAUSED,
            None => &toolbar::HOVER,
        }
    }

    /// 鼠标在钉图上：挂出悬停条（已经有操作栏、正在拖时不挂）
    fn hover(&mut self, w: Win) {
        w.kill_timer(HOVER_TIMER);
        if self.chrome.is_none() && matches!(self.drag, Drag::None) {
            self.hovering = true;
            self.open_bar(w, self.hover_items(), None);
            self.hovering = self.chrome.is_some();
        }
    }

    /// 离开后隔 300ms 看一次：鼠标不在钉图、也不在悬停条上就收起
    fn hover_check(&mut self, w: Win) {
        w.kill_timer(HOVER_TIMER);
        if !self.hovering {
            return;
        }
        let c = plat::cursor_pos();
        let over = |r: Rect| c.x >= r.left && c.x < r.right && c.y >= r.top && c.y < r.bottom;
        if over(w.rect()) || self.chrome.is_some_and(|b| over(b.rect())) {
            w.timer(HOVER_TIMER, 300);
        } else {
            self.unhover();
        }
    }

    fn unhover(&mut self) {
        if std::mem::take(&mut self.hovering)
            && let Some(c) = self.chrome.take()
        {
            c.close();
        }
    }

    /// 编辑中：操作栏按钉图现在的位置重新摆好、重画
    fn place_bar(&mut self, w: Win) {
        if let Some(c) = self.chrome {
            bar::place(c, self.image_rect(w), &self.ed);
        }
    }

    /// 进入编辑：悬停条换成编辑的操作栏；窗口往外扩 2px 画描边、图片不动
    fn enter_edit(&mut self, w: Win) {
        self.unhover();
        if self.chrome.is_some() || self.anim.is_some() {
            return;
        }
        let r = w.rect();
        w.set_rect(Rect { left: r.left - 2, top: r.top - 2, right: r.right + 2, bottom: r.bottom + 2 });
        w.invalidate_all();
        self.open_bar(w, &toolbar::PIN, None);
    }

    /// 录屏结果：下面挂结果条，最左边写 `info`
    fn show_result(&mut self, w: Win, info: String) {
        self.open_bar(w, &toolbar::RESULT, Some(info));
    }

    /// 结果条上的「钉住」：收起结果条，留下普通的动图钉图
    fn keep(&mut self, w: Win) {
        self.preview = false;
        if let Some(c) = self.chrome.take() {
            c.close();
        }
        w.invalidate_all();
    }

    /// 结果条上的「丢弃」：关掉，删掉临时文件
    fn discard(&mut self, w: Win) {
        if let Some(a) = &self.anim {
            let _ = std::fs::remove_file(&a.path);
        }
        w.close();
    }

    /// 动图换下一帧，按这一帧的延迟定下一次；解码出错就停在这一帧
    fn tick(&mut self, w: Win) {
        let (iw, ih) = self.shown();
        let Some(a) = &mut self.anim else { return };
        if !a.playing {
            return w.kill_timer(ANIM_TIMER);
        }
        match a.player.next(self.img.pixels_mut()) {
            Ok(d) => {
                if self.zoom < 100 {
                    self.view = plat::scale_down(&self.img, iw, ih).map(|v| (self.zoom, v));
                }
                w.invalidate_all();
                w.timer(ANIM_TIMER, d.as_millis().max(10) as u32);
            }
            Err(e) => {
                a.playing = false;
                w.kill_timer(ANIM_TIMER);
                toast::show(&w.monitor(), Kind::Err, &format!("动图播放出错：{e}"));
            }
        }
    }

    /// 暂停或接着播；悬停条上的按钮跟着换
    fn toggle_play(&mut self, w: Win) {
        let Some(a) = &mut self.anim else { return };
        a.playing = !a.playing;
        if a.playing {
            w.timer(ANIM_TIMER, 10);
        } else {
            w.kill_timer(ANIM_TIMER);
        }
        if self.hovering
            && let Some(c) = self.chrome
        {
            bar::set_items(c, self.hover_items());
        }
    }

    /// 结束编辑，改动保留（撤销记录留在 `ed.doc` 里，下次进来接着撤销）；正在拖的、正在输入的先收尾
    fn exit_edit(&mut self, w: Win) {
        if !self.editing() {
            return;
        }
        let Some(c) = self.chrome.take() else { return };
        self.ed.up();
        self.ed.finish_text();
        (self.ed.tool, self.ed.sel) = (None, None);
        if matches!(self.drag, Drag::Edit) {
            self.drag = Drag::None;
        }
        w.ime(None);
        c.close();
        let r = w.rect();
        w.set_rect(Rect { left: r.left + 2, top: r.top + 2, right: r.right - 2, bottom: r.bottom - 2 });
        w.invalidate_all();
    }

    /// 开始拖动钉图
    fn start_move(&mut self, w: Win) {
        let r = w.rect();
        self.drag = Drag::Move(plat::cursor_pos(), Point { x: r.left, y: r.top });
    }

    /// 编辑中按下左键：先给标注编辑器（画图形、选中和拖动标注、结束文字输入），它不要就是拖钉图
    fn edit_down(&mut self, w: Win, p: Point) {
        let (q, inside) = (self.to_img(p), self.inside(p));
        let mut taken = false;
        self.update(w, |s| taken = s.ed.down(q, inside));
        if taken {
            self.drag = Drag::Edit;
        } else {
            self.start_move(w);
        }
    }

    /// 松手或丢了鼠标：结束拖动；拖完钉图，操作栏回到新位置
    fn end_drag(&mut self, w: Win) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Edit => self.update(w, |s| s.ed.up()),
            Drag::Move(..) => self.place_bar(w),
            Drag::None => {}
        }
    }

    fn menu(&self, w: Win, at: Point) {
        let (original, editing) = (self.zoom == 100, self.editing());
        // 动图：不能编辑，能暂停；取字、翻译认当前这一帧
        if let Some(a) = &self.anim {
            let items = [
                MenuItem { default: true, ..MenuItem::new(CMD_PLAY, if a.playing { "暂停" } else { "播放" }, "") },
                MenuItem::sep(),
                MenuItem::new(CMD_OCR, "取字", ""),
                MenuItem::new(CMD_TRANSLATE, "翻译", ""),
                MenuItem::sep(),
                MenuItem::new(CMD_COPY, "复制 GIF", "Ctrl+C"),
                MenuItem::new(CMD_SAVE, "保存 GIF…", "Ctrl+S"),
                MenuItem { enabled: !original, ..MenuItem::new(CMD_ORIGINAL, "原始大小", "") },
                MenuItem::sep(),
                MenuItem::new(CMD_CLOSE, "关闭", "Esc"),
            ];
            return w.menu(&items, at, move |cmd| _ = w.with(|p: &mut Pin| p.command(w, cmd)));
        }
        let items = [
            MenuItem { default: true, ..MenuItem::new(CMD_EDIT, if editing { "完成编辑" } else { "编辑" }, if editing { "Enter" } else { "" }) },
            MenuItem::new(CMD_OCR, "取字", ""),
            MenuItem::new(CMD_TRANSLATE, "翻译", ""),
            MenuItem::sep(),
            MenuItem::new(CMD_COPY, "复制", "Ctrl+C"),
            MenuItem::new(CMD_SAVE, "保存…", "Ctrl+S"),
            MenuItem { enabled: !original, ..MenuItem::new(CMD_ORIGINAL, "原始大小", "") },
            MenuItem::sep(),
            MenuItem::new(CMD_CLOSE, "关闭", "Esc"),
        ];
        // 菜单期间钉图可能已经被关掉（比如托盘「关闭所有钉图」），那就什么都不做
        w.menu(&items, at, move |cmd| _ = w.with(|p: &mut Pin| p.command(w, cmd)));
    }

    /// 右键菜单的命令
    fn command(&mut self, w: Win, cmd: usize) {
        match cmd {
            CMD_EDIT if self.editing() => self.exit_edit(w),
            CMD_EDIT => self.enter_edit(w),
            CMD_OCR | CMD_TRANSLATE => self.recognize(w, cmd == CMD_OCR),
            CMD_COPY => self.copy(w),
            CMD_SAVE => self.save(w),
            CMD_ORIGINAL => {
                // 以图片中心为准恢复 100%
                let r = w.rect();
                self.set_zoom(w, 100, Point { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 });
            }
            CMD_PLAY => self.toggle_play(w),
            CMD_CLOSE => w.close(),
            _ => {}
        }
    }

    /// 取字、翻译：先结束编辑；带不带标注都按原图识别，卡片挨着钉图的图片区域弹出
    fn recognize(&mut self, w: Win, ocr: bool) {
        self.exit_edit(w);
        let img = self.img.crop(&self.img.full());
        let open = if ocr { crate::card::ocr } else { crate::card::translate };
        open(img, self.image_rect(w), w.monitor());
    }

    /// 编辑时操作栏、选项条上的按钮
    fn act(&mut self, w: Win, b: bar::Btn) {
        let a = match b {
            bar::Btn::Bar(a) => a,
            bar::Btn::Opt(i) => {
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
            Act::Pin => self.keep(w),
            Act::Cancel => self.discard(w),
            Act::Undo => self.update(w, |s| _ = s.ed.undo()),
            Act::Redo => self.update(w, |s| _ = s.ed.redo()),
            Act::Ocr | Act::Translate => self.recognize(w, a == Act::Ocr),
            Act::Save => self.save(w),
            Act::Copy => self.copy(w),
            Act::Done => self.exit_edit(w),
            Act::Edit => self.enter_edit(w),
            Act::Pause | Act::Play => self.toggle_play(w),
            Act::Close => w.close(),
            _ => {}
        }
    }

    fn save(&mut self, w: Win) {
        // 动图把 GIF 文件拷过去；录屏结果选好位置就关掉，和截图一样
        if let Some(a) = &self.anim {
            let (src, preview) = (a.path.clone(), self.preview);
            return image_io::ask_save_gif(w, move |dst| {
                if preview {
                    w.close();
                }
                let mon = w.monitor();
                toast::show(&mon, Kind::Busy, "正在保存…");
                image_io::copy_async(src, dst, mon);
            });
        }
        // 正在输入的文字先提交，对话框后面看到的就是存下来的样子
        self.update(w, |s| s.ed.finish_text());
        let Some(img) = self.export() else { return };
        image_io::ask_save(w, move |path| {
            let mon = w.monitor();
            toast::show(&mon, Kind::Busy, "正在保存…");
            image_io::save_async(img, path, mon);
        });
    }
}

impl Handler for Pin {
    fn event(&mut self, w: Win, e: Event) {
        // 编辑中正在输入文字：键盘和输入法先给文字框
        if self.ed.text.is_some() {
            let (mut rects, to_client) = (None, self.client_of());
            self.update(w, |s| rects = s.ed.text_event(&e, to_client));
            if let Some(rects) = rects {
                rects.into_iter().for_each(|r| w.invalidate(r));
                return;
            }
        }
        let editing = self.editing();
        match e {
            Event::Focus(true) => w.invalidate_all(),
            Event::Timer(ANIM_TIMER) => self.tick(w),
            Event::Timer(HOVER_TIMER) => self.hover_check(w),
            Event::Leave if self.hovering => w.timer(HOVER_TIMER, 300),
            // 录屏结果：双击复制（和截图一样），按键都是结果条上的快捷键
            Event::Down { button: Button::Left, double: true, .. } if self.preview => self.copy(w),
            Event::Key { key, mods } if self.preview => match key {
                Key::Enter => self.copy(w),
                Key::Esc => self.discard(w),
                Key::Char(b'C') if mods.cmd => self.copy(w),
                Key::Char(b'S') if mods.cmd => self.save(w),
                _ => {}
            },
            // 点到钉图和操作栏以外的地方（别的窗口拿到前台）就结束编辑，改动保留；自己弹的保存对话框不算
            Event::Deactivate => self.exit_edit(w),
            // 编辑中双击也只是又按了一下（连续放序号）
            Event::Down { p, button: Button::Left, .. } if editing => self.edit_down(w, p),
            Event::Down { button: Button::Left, double: true, .. } => w.close(),
            Event::Down { button: Button::Left, .. } => self.start_move(w),
            Event::Move { p, mods } => match self.drag {
                Drag::Move(p0, w0) => {
                    let c = plat::cursor_pos();
                    w.move_to(w0.x + c.x - p0.x, w0.y + c.y - p0.y);
                    // 拖动期间操作栏先藏起来，松手后出现在新位置
                    if let Some(bar) = self.chrome
                        && c != p0
                    {
                        bar.hide();
                    }
                }
                Drag::Edit => {
                    let q = self.to_img(p);
                    self.update(w, |s| s.ed.drag_to(q, mods.shift));
                }
                Drag::None => self.hover(w),
            },
            Event::Up { button: Button::Left, .. } | Event::CaptureLost => self.end_drag(w),
            Event::Up { button: Button::Right, .. } => self.menu(w, w.cursor_pos()),
            Event::Wheel { delta, p, mods } => self.wheel(w, delta, p, mods),
            Event::Timer(HUD_TIMER) => {
                w.kill_timer(HUD_TIMER);
                self.hud = None;
                w.invalidate_all();
            }
            Event::Key { key, mods } => {
                // 编辑中的标注快捷键：工具、撤销重做、删除
                let mut used = false;
                if editing {
                    self.update(w, |s| used = s.ed.key(key, mods));
                }
                match key {
                    _ if used => {}
                    Key::Esc | Key::Enter if editing => self.exit_edit(w),
                    Key::Esc => w.close(),
                    Key::Char(b'C') if mods.cmd => self.copy(w),
                    Key::Char(b'S') if mods.cmd => self.save(w),
                    _ => {}
                }
            }
            // 钉图按物理像素 1:1 显示，拖到别的缩放比例的屏幕上也不变大小
            _ => {}
        }
    }

    fn paint(&mut self, w: Win, c: &Canvas) {
        self.draw_body(c, 0, 0);
        if let Some((hud, r)) = self.hud(w) {
            hud.draw(c, r.left, r.top);
        }
    }

    fn cursor(&mut self, _w: Win, p: Point) -> Cursor {
        // 编辑器不管的地方（没选工具时的空白处）是拖动钉图
        if self.editing() { self.ed.cursor(self.to_img(p), self.inside(p)).unwrap_or(Cursor::Move) } else { Cursor::Move }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::{Ann, Shape, Tool};
    use bar::Btn;

    #[test]
    fn coords_at_zoom() {
        // 150%、编辑时描边 2：客户区 (152, 77) 是原图 (100, 50)，原图 (0, 0) 画在 (2, 2)
        assert_eq!(to_img(Point { x: 152, y: 77 }, 2, 150), Pt::new(100.0, 50.0));
        assert_eq!(to_img(Point { x: 2, y: 2 }, 2, 150), Pt::new(0.0, 0.0));
        // 50%：一个屏幕像素是两个原图像素
        assert_eq!(to_img(Point { x: 51, y: 26 }, 1, 50), Pt::new(100.0, 50.0));
        // 原图范围 → 客户区：乘缩放加描边，向外取整再多留 1
        assert_eq!(to_client(Rect { left: 100, top: 50, right: 111, bottom: 60 }, 2, 150), Rect { left: 151, top: 76, right: 170, bottom: 93 });
        assert_eq!(to_client(Rect { left: 100, top: 50, right: 110, bottom: 60 }, 1, 50), Rect { left: 50, top: 25, right: 57, bottom: 32 });
    }

    fn mouse(w: Win, e: fn(Point) -> Event, x: i32, y: i32) {
        w.send(e(Point { x, y }));
    }

    fn down(p: Point) -> Event {
        Event::Down { p, button: Button::Left, double: false, mods: Mods::default() }
    }

    fn mv(p: Point) -> Event {
        Event::Move { p, mods: Mods::default() }
    }

    fn up(_: Point) -> Event {
        Event::Up { button: Button::Left }
    }

    /// 按下、经过中间点、在终点松开（客户区坐标）
    fn drag(w: Win, from: (i32, i32), to: (i32, i32)) {
        mouse(w, down, from.0, from.1);
        mouse(w, mv, (from.0 + to.0) / 2, (from.1 + to.1) / 2);
        mouse(w, mv, to.0, to.1);
        mouse(w, up, to.0, to.1);
    }

    /// 点操作栏窗口上的按钮
    fn click(p: &Pin, b: Btn) {
        let bar = p.chrome.unwrap();
        let r = bar.peek::<bar::Chrome>().button(b);
        let (x, y) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
        for e in [mv, down, up] {
            mouse(bar, e, x, y);
        }
    }

    fn key(w: Win, key: Key) {
        w.send(Event::Key { key, mods: Mods::default() });
    }

    /// 钉图连同操作栏按屏幕上的相对位置画到灰底上，存成 target/test-tmp/pin/<name>
    fn shot(w: Win, name: &str) {
        let s = w.peek::<Pin>();
        let pr = w.rect();
        let chrome = s.chrome.map(|c| (c.rect(), c));
        let mut u = Rect { left: pr.left - 20, top: pr.top - 20, right: pr.right + 20, bottom: pr.bottom + 20 };
        if let Some((br, _)) = &chrome {
            u = Rect { left: u.left.min(br.left - 20), top: u.top.min(br.top - 20), right: u.right.max(br.right + 20), bottom: u.bottom.max(br.bottom + 20) };
        }
        let mut out = Bitmap::new(u.right - u.left, u.bottom - u.top).unwrap();
        out.pixels_mut().fill(0x70);
        s.draw_body(&Canvas::new(&out, out.full(), false), u.left - pr.left, u.top - pr.top);
        // 操作栏画成预乘 alpha 的位图再叠上去：src + dst × (1 - a)
        if let Some((br, c)) = chrome {
            let b = Bitmap::new(br.right - br.left, br.bottom - br.top).unwrap();
            c.peek::<bar::Chrome>().paint(c, &Canvas::new(&b, b.full(), true));
            let (ow, src, dst) = (out.w, b.pixels(), out.pixels_mut());
            for y in 0..b.h {
                for x in 0..b.w {
                    let si = ((y * b.w + x) * 4) as usize;
                    let di = (((y + br.top - u.top) * ow + x + br.left - u.left) * 4) as usize;
                    let a = src[si + 3] as u32;
                    for k in 0..3 {
                        dst[di + k] = (src[si + k] as u32 + dst[di + k] as u32 * (255 - a) / 255) as u8;
                    }
                }
            }
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("pin");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), plat::encode(&out.crop(&out.full()), false).unwrap()).unwrap();
    }

    /// 建一个带标注的真钉图（图片是白底，不截屏幕），用事件进入编辑、放大到 150%、画新矩形、选中旧矩形移动改色、
    /// 结束，再进入用操作栏一路撤销到最初；画面存成 target/test-tmp/pin/*.png。会在屏幕上出现几百毫秒，平时不跑：
    /// cargo test pin_edit_flow -- --ignored
    #[test]
    #[ignore]
    fn pin_edit_flow() {
        let mon = plat::monitor_at(Point::default());
        let sc = mon.scale();
        let mut img = Bitmap::new(400, 300).unwrap();
        img.pixels_mut().fill(0xF0);
        let old = Ann { shape: Shape::Rect(Pt::new(20.0, 20.0), Pt::new(120.0, 80.0)), color: 0, size: 1 };
        let at = Point { x: mon.work.left + 500, y: mon.work.top + 150 };
        create(img, at, Editor::new(Doc::with(vec![old.clone()]), sc));
        let w = *Win::all(plat::Kind::Pin).last().expect("钉图没出来");
        let s = w.peek::<Pin>();

        assert_eq!(w.rect(), Rect { left: at.x, top: at.y, right: at.x + 400, bottom: at.y + 300 });
        // 右键菜单「编辑」：窗口往外扩 2、图片不动；操作栏右边和钉图对齐，在下方 8 处
        w.with(|p: &mut Pin| p.command(w, CMD_EDIT));
        assert!(s.chrome.is_some());
        assert_eq!(w.rect(), Rect { left: at.x - 2, top: at.y - 2, right: at.x + 402, bottom: at.y + 302 });
        // 滚轮 5 格放大到 150%，以图片左上角为准
        w.send(Event::Wheel { delta: 600, p: at, mods: Mods::default() });
        assert_eq!((s.zoom, s.ed.zoom), (150, 1.5));
        assert_eq!(w.rect(), Rect { left: at.x - 2, top: at.y - 2, right: at.x + 602, bottom: at.y + 452 });
        // 操作栏窗口连右下 4 的实心阴影
        let br = s.chrome.unwrap().rect();
        assert_eq!((br.right - (4.0 * sc).round() as i32, br.top), (at.x + 600, at.y + 450 + (8.0 * sc).round() as i32));

        // 点操作栏的矩形工具，在钉图上画一个：客户区 (302, 152) → 原图 (200, 100)
        click(s, Btn::Bar(Act::Rect));
        assert_eq!(s.ed.tool, Some(Tool::Rect));
        drag(w, (302, 152), (452, 302));
        assert_eq!(s.ed.doc.anns[1].shape, Shape::Rect(Pt::new(200.0, 100.0), Pt::new(300.0, 200.0)));
        // 点旧矩形的左边（原图 (20, 50)）选中它，拖 (15, 30) 个屏幕像素 = 原图 (10, 20)
        drag(w, (32, 77), (47, 107));
        assert_eq!(s.ed.sel, Some(0));
        assert_eq!(s.ed.doc.anns[0].shape, Shape::Rect(Pt::new(30.0, 40.0), Pt::new(130.0, 100.0)));
        // 选项条点紫色改掉它
        click(s, Btn::Opt(Item::Color(3)));
        assert_eq!(s.ed.doc.anns[0].color, 3);
        // 鼠标停在「完成」上，提示出来
        let bar = s.chrome.unwrap();
        let r = bar.peek::<bar::Chrome>().button(Btn::Bar(Act::Done));
        mouse(bar, mv, (r.left + r.right) / 2, (r.top + r.bottom) / 2);
        bar.send(Event::Timer(bar::TIP_TIMER));
        shot(w, "edit.png");

        // Enter 结束编辑：改动保留，去掉描边和操作栏
        key(w, Key::Enter);
        assert!(s.chrome.is_none() && s.ed.tool.is_none() && s.ed.sel.is_none());
        assert_eq!(w.rect(), Rect { left: at.x, top: at.y, right: at.x + 600, bottom: at.y + 450 });
        assert_eq!(s.ed.doc.anns.len(), 2);
        assert!(!bar.alive());
        shot(w, "done.png");

        // 再进编辑，点操作栏的撤销一路撤到最初：改色、移动、画矩形三步
        w.with(|p: &mut Pin| p.command(w, CMD_EDIT));
        let mut n = 0;
        while s.ed.doc.can_undo() {
            click(s, Btn::Bar(Act::Undo));
            n += 1;
            assert!(n <= 3);
        }
        assert_eq!(s.ed.doc.anns, vec![old]);
        assert!(s.ed.doc.can_redo());
        // 别的窗口拿到前台也结束编辑
        w.send(Event::Deactivate);
        assert!(s.chrome.is_none());
        w.close();
    }

    /// 悬停条：鼠标挪到钉图上挂出悬停条（不算编辑），停在按钮上出提示；点「编辑标注」换成编辑的操作栏。
    /// 画面存成 target/test-tmp/pin/hover.png。会在屏幕上出现一下，平时不跑：cargo test pin_hover_flow -- --ignored
    #[test]
    #[ignore]
    fn pin_hover_flow() {
        let mon = plat::monitor_at(Point::default());
        let mut img = Bitmap::new(300, 200).unwrap();
        img.pixels_mut().fill(0xF0);
        create(img, Point { x: mon.work.left + 500, y: mon.work.top + 150 }, Editor::new(Doc::default(), mon.scale()));
        let w = *Win::all(plat::Kind::Pin).last().expect("钉图没出来");
        let s = w.peek::<Pin>();
        mouse(w, mv, 100, 100);
        assert!(s.hovering && s.chrome.is_some() && !s.editing() && s.border() == 0);
        let bar = s.chrome.unwrap();
        let r = bar.peek::<bar::Chrome>().button(Btn::Bar(Act::Translate));
        mouse(bar, mv, (r.left + r.right) / 2, (r.top + r.bottom) / 2);
        bar.send(Event::Timer(bar::TIP_TIMER));
        shot(w, "hover.png");
        click(s, Btn::Bar(Act::Edit));
        assert!(s.editing() && !s.hovering && !bar.alive());
        w.close();
    }

    /// 录屏结果：合成一段三帧的 GIF（不截屏幕），原位置开出结果钉图：下面是结果条、定时器换帧、画面跟着变；
    /// 「钉住」收起结果条变成普通动图钉图，悬停条能暂停、播放；再开一个点「丢弃」会删掉文件。会在屏幕上出现一下，平时不跑：
    /// cargo test gif_result_flow -- --ignored
    #[test]
    #[ignore]
    fn gif_result_flow() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("pin-gif");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let make = |name: &str| {
            let path = dir.join(name);
            let (w, h) = (120usize, 80usize);
            let mut wr = crate::anim::Writer::create(&path, w as i32, h as i32).unwrap();
            for (i, c) in [0x30u8, 0x90, 0xF0].into_iter().enumerate() {
                wr.push(&vec![c; w * h * 4], w * 4, std::time::Duration::from_millis(i as u64 * 100)).unwrap();
            }
            wr.finish().unwrap();
            path
        };
        let mon = plat::monitor_at(Point::default());
        let at = Point { x: mon.work.left + 300, y: mon.work.top + 200 };
        create_gif(make("a.gif"), at, Some("00:03 · 12 KB".into())).unwrap();
        let w = *Win::all(plat::Kind::Pin).last().expect("动图钉图没出来");
        let s = w.peek::<Pin>();
        assert!(s.preview && s.chrome.is_some() && s.anim.is_some());
        // 图片左上角在 at，外面一圈框（默认开，和录制时的边框一样粗）
        let f = s.frame;
        assert_eq!(f, (3.0 * mon.scale()).round().max(2.0) as i32);
        assert_eq!(w.rect(), Rect { left: at.x - f, top: at.y - f, right: at.x + 120 + f, bottom: at.y + 80 + f });
        assert_eq!(s.image_rect(w), Rect { left: at.x, top: at.y, right: at.x + 120, bottom: at.y + 80 });
        assert_eq!(s.img.pixels()[0], 0x30);
        // 定时器到了：换成第二帧
        w.send(Event::Timer(ANIM_TIMER));
        assert_eq!(s.img.pixels()[0], 0x90);
        shot(w, "gif-result.png");
        // 「钉住」：结果条收起，还是动图
        click(s, Btn::Bar(Act::Pin));
        assert!(!s.preview && s.chrome.is_none() && s.anim.is_some());
        // 鼠标挪上去挂出悬停条：点暂停，按钮换成播放，再点接着播
        mouse(w, mv, 10, 10);
        assert!(s.hovering);
        click(s, Btn::Bar(Act::Pause));
        assert!(!s.anim.as_ref().unwrap().playing);
        shot(w, "gif-hover-paused.png");
        click(s, Btn::Bar(Act::Play));
        assert!(s.anim.as_ref().unwrap().playing);
        w.close();
        // 再开一个，「丢弃」：窗口关掉、文件删掉
        let b = make("b.gif");
        create_gif(b.clone(), at, Some("00:03 · 12 KB".into())).unwrap();
        let w = *Win::all(plat::Kind::Pin).last().unwrap();
        click(w.peek::<Pin>(), Btn::Bar(Act::Cancel));
        let t0 = std::time::Instant::now();
        while w.alive() && t0.elapsed() < std::time::Duration::from_secs(2) {
            plat::pump();
        }
        assert!(!w.alive() && !b.exists());
    }
}
