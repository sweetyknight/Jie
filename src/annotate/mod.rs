//! 原位标注：数据模型、撤销重做、命中、绘制。截图遮罩和钉图编辑共用一个 Editor。
//! 坐标一律是底图的物理像素（遮罩是整块屏幕，钉图是原图）；粗细、字号是 DIP，乘 scale 换算。

pub mod mosaic;
pub mod text;

use crate::plat::{Affine, Bitmap, Canvas, Cursor, Face, Key, Mods, Path, Rect, RectF, Stroke, Text};
use crate::ui::{self, rectf};
use std::cell::Cell;

/// 色板：红（默认）、黄、绿、紫、黑、白
pub const COLORS: [u32; 6] = [0xE5383B, 0xF5B700, 0x23A55A, 0x9C44EC, 0x1B1B1F, 0xFFFFFF];
pub const COLOR_NAMES: [&str; 6] = ["红", "黄", "绿", "紫", "黑", "白"];
// 三档粗细（DIP）：线条、文字字号、序号直径、马赛克笔刷宽和格子
pub const STROKE: [f32; 3] = [2.0, 4.0, 6.0];
pub const FONT: [f32; 3] = [16.0, 22.0, 30.0];
pub const NUM_D: [f32; 3] = [22.0, 28.0, 36.0];
pub const BRUSH: [f32; 3] = [16.0, 28.0, 44.0];
pub const BLOCK: [f32; 3] = [8.0, 10.0, 12.0];

thread_local! {
    /// 颜色和粗细：全部工具共用，本次运行内记住，不写配置文件
    static PEN: Cell<(usize, usize)> = const { Cell::new((0, 1)) };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Rect,
    Ellipse,
    Arrow,
    Pen,
    Text,
    Num,
    Mosaic,
}

impl Tool {
    pub const ALL: [Tool; 7] = [Tool::Rect, Tool::Ellipse, Tool::Arrow, Tool::Pen, Tool::Text, Tool::Num, Tool::Mosaic];

    /// 快捷键 R E A P T N M
    pub fn key(self) -> u8 {
        match self {
            Tool::Rect => b'R',
            Tool::Ellipse => b'E',
            Tool::Arrow => b'A',
            Tool::Pen => b'P',
            Tool::Text => b'T',
            Tool::Num => b'N',
            Tool::Mosaic => b'M',
        }
    }

    fn kind(self) -> Kind {
        match self {
            Tool::Text => Kind::Text,
            Tool::Mosaic => Kind::Mosaic,
            _ => Kind::Line,
        }
    }
}

/// 选项条的样子：粗细按钮画成圆点、字母 A 还是马赛克圆；马赛克没有色板
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Line,
    Text,
    Mosaic,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    pub fn new(x: f32, y: f32) -> Pt {
        Pt { x, y }
    }

    fn dist(self, o: Pt) -> f32 {
        (self.x - o.x).hypot(self.y - o.y)
    }

    fn xy(self) -> (f32, f32) {
        (self.x, self.y)
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Shape {
    /// 起点和终点（拖动的两个角）
    Rect(Pt, Pt),
    Ellipse(Pt, Pt),
    Arrow(Pt, Pt),
    Pen(Vec<Pt>),
    /// 文字框左上角和内容（多行用 \n）
    Text(Pt, String),
    /// 圆心和数字
    Num(Pt, u32),
    Mosaic(Vec<Pt>),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Ann {
    pub shape: Shape,
    /// 色板下标、粗细档位
    pub color: usize,
    pub size: usize,
}

/// 文字标注的排版：微软雅黑，行高 1.3 倍，不自动换行
pub fn text_layout(text: &str, px: f32) -> Option<Text> {
    Text::wrapped(Face::Cjk, px, false, text, 100_000.0, Some(px * 1.3))
}

fn span(pts: impl IntoIterator<Item = Pt>) -> RectF {
    pts.into_iter().fold(rectf(f32::MAX, f32::MAX, f32::MIN, f32::MIN), |r, p| rectf(r.left.min(p.x), r.top.min(p.y), r.right.max(p.x), r.bottom.max(p.y)))
}

fn grow(r: RectF, d: f32) -> RectF {
    rectf(r.left - d, r.top - d, r.right + d, r.bottom + d)
}

/// 点到线段的距离
fn seg_dist(p: Pt, a: Pt, b: Pt) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(Pt::new(a.x + t * dx, a.y + t * dy))
}

/// 箭头头部的长和半宽：长 8 + 2.5 × 粗细（DIP），最多占全长的 0.6
fn arrow_head(len: f32, t: f32, s: f32) -> (f32, f32) {
    let hl = (8.0 * s + 2.5 * t).min(len * 0.6);
    (hl, hl * 0.5)
}

impl Ann {
    pub fn kind(&self) -> Kind {
        match self.shape {
            Shape::Text(..) => Kind::Text,
            Shape::Mosaic(_) => Kind::Mosaic,
            _ => Kind::Line,
        }
    }

    /// 线宽（像素）
    fn width(&self, s: f32) -> f32 {
        match self.shape {
            Shape::Mosaic(_) => BRUSH[self.size] * s,
            _ => STROKE[self.size] * s,
        }
    }

    /// 画出来占的范围（像素，含线宽）
    pub fn bounds(&self, s: f32) -> RectF {
        let w = self.width(s) / 2.0;
        match &self.shape {
            Shape::Rect(a, b) | Shape::Ellipse(a, b) => grow(span([*a, *b]), w),
            Shape::Arrow(a, b) => grow(span([*a, *b]), w.max(arrow_head(a.dist(*b), w * 2.0, s).1)),
            Shape::Pen(p) | Shape::Mosaic(p) => grow(span(p.iter().copied()), w),
            Shape::Text(at, t) => {
                let (tw, th) = text_layout(t, FONT[self.size] * s).map_or((0.0, 0.0), |l| (l.width(), l.height()));
                rectf(at.x, at.y, at.x + tw, at.y + th)
            }
            Shape::Num(c, _) => grow(span([*c]), NUM_D[self.size] * s / 2.0),
        }
    }

    /// p 点在不在这个标注上：线条按描边算（容差 5 个屏幕像素），文字和序号按范围算。`zoom` 是钉图的缩放
    fn hit(&self, p: Pt, s: f32, zoom: f32) -> bool {
        let tol = 5.0 * s / zoom + self.width(s) / 2.0;
        match &self.shape {
            Shape::Rect(a, b) => {
                let r = span([*a, *b]);
                let outer = p.x >= r.left - tol && p.x <= r.right + tol && p.y >= r.top - tol && p.y <= r.bottom + tol;
                let inner = p.x > r.left + tol && p.x < r.right - tol && p.y > r.top + tol && p.y < r.bottom - tol;
                outer && !inner
            }
            Shape::Ellipse(a, b) => {
                let r = span([*a, *b]);
                let (rx, ry) = ((r.right - r.left) / 2.0, (r.bottom - r.top) / 2.0);
                rx >= 1.0 && ry >= 1.0 && ((((p.x - r.left - rx) / rx).hypot((p.y - r.top - ry) / ry)) - 1.0).abs() * rx.min(ry) < tol
            }
            Shape::Arrow(a, b) => seg_dist(p, *a, *b) < tol + 3.0 * s / zoom,
            Shape::Pen(v) | Shape::Mosaic(v) => v.windows(2).any(|w| seg_dist(p, w[0], w[1]) < tol),
            Shape::Text(..) | Shape::Num(..) => {
                let r = self.bounds(s);
                p.x >= r.left && p.x <= r.right && p.y >= r.top && p.y <= r.bottom
            }
        }
    }

    pub fn offset(&mut self, dx: f32, dy: f32) {
        let mv = |p: &mut Pt| (p.x, p.y) = (p.x + dx, p.y + dy);
        match &mut self.shape {
            Shape::Rect(a, b) | Shape::Ellipse(a, b) | Shape::Arrow(a, b) => [a, b].into_iter().for_each(mv),
            Shape::Pen(v) | Shape::Mosaic(v) => v.iter_mut().for_each(mv),
            Shape::Text(p, _) | Shape::Num(p, _) => mv(p),
        }
    }

    /// 拖出来不到 3 像素的矩形、椭圆、箭头当手抖
    fn tiny(&self) -> bool {
        match &self.shape {
            Shape::Rect(a, b) | Shape::Ellipse(a, b) | Shape::Arrow(a, b) => (b.x - a.x).abs().max((b.y - a.y).abs()) < 3.0,
            _ => false,
        }
    }
}

/// 一组标注和它的撤销、重做记录（整组快照）
#[derive(Default)]
pub struct Doc {
    pub anns: Vec<Ann>,
    undo: Vec<Vec<Ann>>,
    redo: Vec<Vec<Ann>>,
}

impl Doc {
    /// 只有标注、没有历史（钉图刚建好时）
    pub fn with(anns: Vec<Ann>) -> Doc {
        Doc { anns, ..Doc::default() }
    }

    /// 改动之前调用：记下现在的样子，清掉重做
    pub fn checkpoint(&mut self) {
        self.undo.push(self.anns.clone());
        self.redo.clear();
    }

    /// 刚记下的那步没用上（画得太小的图形）：回到记下时的样子
    fn discard(&mut self) {
        if let Some(a) = self.undo.pop() {
            self.anns = a;
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(a) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.anns, a));
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(a) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.anns, a));
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn next_num(&self) -> u32 {
        self.anns.iter().filter_map(|a| if let Shape::Num(_, n) = a.shape { Some(n) } else { None }).max().unwrap_or(0) + 1
    }
}

enum Drag {
    None,
    /// 拖动选中的标注；pushed：已经记了撤销点（动过 2 像素才记）
    Move { p0: Pt, last: Pt, pushed: bool },
    /// 画新图形；矩形、椭圆、箭头拖过 2 像素才创建
    Draw { p0: Pt, made: bool },
    /// 文字工具：松手时在按下的位置开始输入
    Text(Pt),
}

/// 标注编辑：当前工具、选中的标注、鼠标拖动。宿主负责坐标换算、重画、操作栏和选项条。
pub struct Editor {
    pub doc: Doc,
    pub tool: Option<Tool>,
    /// 选中的标注
    pub sel: Option<usize>,
    /// DIP → 物理像素
    pub scale: f32,
    /// 钉图的缩放（遮罩是 1）：点击容差按屏幕上的距离算
    pub zoom: f32,
    drag: Drag,
    /// 正在输入的文字
    pub text: Option<text::Edit>,
}

impl Editor {
    pub fn new(doc: Doc, scale: f32) -> Editor {
        Editor { doc, tool: None, sel: None, scale, zoom: 1.0, drag: Drag::None, text: None }
    }

    pub fn selected(&self) -> Option<&Ann> {
        self.sel.and_then(|i| self.doc.anns.get(i))
    }

    /// 选项条显示的颜色和粗细：选中标注的，否则是当前默认值
    pub fn pen(&self) -> (usize, usize) {
        self.selected().map_or_else(|| PEN.get(), |a| (a.color, a.size))
    }

    /// 选项条：(样子, 有没有删除按钮)；没选工具、也没选中标注时不显示
    pub fn opts(&self) -> Option<(Kind, bool)> {
        match self.selected() {
            Some(a) => Some((a.kind(), true)),
            None => self.tool.map(|t| (t.kind(), false)),
        }
    }

    /// 选工具；再选一次当前工具就是放下工具
    pub fn set_tool(&mut self, t: Tool) {
        self.finish_text();
        self.tool = if self.tool == Some(t) { None } else { Some(t) };
        self.sel = None;
    }

    /// 改颜色：有选中的标注就改它，同时记成默认值
    pub fn set_color(&mut self, c: usize) {
        if let Some(i) = self.sel
            && self.doc.anns[i].color != c
        {
            self.doc.checkpoint();
            self.doc.anns[i].color = c;
        }
        PEN.set((c, PEN.get().1));
    }

    pub fn set_size(&mut self, size: usize) {
        if let Some(i) = self.sel
            && self.doc.anns[i].size != size
        {
            self.doc.checkpoint();
            self.doc.anns[i].size = size;
        }
        PEN.set((PEN.get().0, size));
    }

    pub fn delete(&mut self) -> bool {
        let Some(i) = self.sel.take() else { return false };
        self.doc.checkpoint();
        self.doc.anns.remove(i);
        true
    }

    pub fn undo(&mut self) -> bool {
        self.finish_text();
        self.sel = None;
        self.doc.undo()
    }

    pub fn redo(&mut self) -> bool {
        self.finish_text();
        self.sel = None;
        self.doc.redo()
    }

    /// 鼠标下最上面的标注；选着马赛克工具时跳过马赛克（点上去是接着涂）
    pub fn hit(&self, p: Pt) -> Option<usize> {
        let skip = |a: &Ann| self.tool == Some(Tool::Mosaic) && a.kind() == Kind::Mosaic;
        self.doc.anns.iter().rposition(|a| !skip(a) && a.hit(p, self.scale, self.zoom))
    }

    /// 按下左键。`inside`：点在能画的区域里（遮罩的选区、钉图的图片）。
    /// 返回 true 表示编辑器接手了；false 交给宿主（移动选区、拖钉图、框新选区）
    pub fn down(&mut self, p: Pt, inside: bool) -> bool {
        self.drag = Drag::None;
        // 正在输入文字时，点哪里都只是结束输入
        if self.text.is_some() {
            self.finish_text();
            return true;
        }
        if !inside {
            self.sel = None;
            return false;
        }
        // 点到已画的标注就是选中它，不管当前选没选工具
        if let Some(i) = self.hit(p) {
            self.sel = Some(i);
            // 马赛克取的是它所在位置的原图，拖走会露出原来盖住的内容，所以只能选中、不能移动
            if self.doc.anns[i].kind() != Kind::Mosaic {
                self.drag = Drag::Move { p0: p, last: p, pushed: false };
            }
            return true;
        }
        self.sel = None;
        let Some(tool) = self.tool else { return false };
        let (color, size) = PEN.get();
        match tool {
            Tool::Text => self.drag = Drag::Text(p),
            Tool::Num => {
                self.doc.checkpoint();
                let n = self.doc.next_num();
                self.doc.anns.push(Ann { shape: Shape::Num(p, n), color, size });
            }
            Tool::Pen | Tool::Mosaic => {
                self.doc.checkpoint();
                let shape = if tool == Tool::Pen { Shape::Pen(vec![p, p]) } else { Shape::Mosaic(vec![p, p]) };
                self.doc.anns.push(Ann { shape, color, size });
                self.drag = Drag::Draw { p0: p, made: true };
            }
            _ => self.drag = Drag::Draw { p0: p, made: false },
        }
        true
    }

    /// 拖动；`shift`：正方形、正圆、45° 角箭头
    pub fn drag_to(&mut self, p: Pt, shift: bool) {
        match &mut self.drag {
            Drag::Move { p0, last, pushed } => {
                if !*pushed {
                    if p.dist(*p0) < 2.0 {
                        return;
                    }
                    self.doc.checkpoint();
                    *pushed = true;
                }
                let (dx, dy) = (p.x - last.x, p.y - last.y);
                *last = p;
                if let Some(a) = self.sel.and_then(|i| self.doc.anns.get_mut(i)) {
                    a.offset(dx, dy);
                }
            }
            Drag::Draw { p0, made } => {
                if !*made {
                    if p.dist(*p0) < 2.0 {
                        return;
                    }
                    let shape = match self.tool {
                        Some(Tool::Ellipse) => Shape::Ellipse(*p0, *p0),
                        Some(Tool::Arrow) => Shape::Arrow(*p0, *p0),
                        _ => Shape::Rect(*p0, *p0),
                    };
                    let (color, size) = PEN.get();
                    self.doc.checkpoint();
                    self.doc.anns.push(Ann { shape, color, size });
                    *made = true;
                }
                let Some(a) = self.doc.anns.last_mut() else { return };
                match &mut a.shape {
                    Shape::Pen(v) | Shape::Mosaic(v) => {
                        if v.last() != Some(&p) {
                            v.push(p);
                        }
                    }
                    Shape::Arrow(a, b) => *b = if shift { snap45(*a, p) } else { p },
                    Shape::Rect(a, b) | Shape::Ellipse(a, b) => *b = if shift { square(*a, p) } else { p },
                    _ => {}
                }
            }
            _ => {}
        }
    }

    pub fn up(&mut self) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Text(p) => self.start_text(p),
            Drag::Draw { made: true, .. } if self.doc.anns.last().is_some_and(Ann::tiny) => self.doc.discard(),
            _ => {}
        }
    }

    /// 标注的快捷键：工具 R E A P T N M（再按一次放下）、Delete、Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z（mac 上是 ⌘）。返回 true 表示处理了
    pub fn key(&mut self, k: Key, m: Mods) -> bool {
        match k {
            Key::Char(b'Z') if m.cmd && m.shift => _ = self.redo(),
            Key::Char(b'Z') if m.cmd => _ = self.undo(),
            Key::Char(b'Y') if m.cmd => _ = self.redo(),
            Key::Delete | Key::Back if self.sel.is_some() => _ = self.delete(),
            _ if m.cmd => return false,
            Key::Char(c) => match Tool::ALL.into_iter().find(|t| t.key() == c) {
                Some(t) => self.set_tool(t),
                None => return false,
            },
            _ => return false,
        }
        true
    }

    /// 鼠标在 p 时的光标；None 交给宿主
    pub fn cursor(&self, p: Pt, inside: bool) -> Option<Cursor> {
        if !inside {
            return None;
        }
        Some(match (self.hit(p), self.tool) {
            (Some(i), _) if self.doc.anns[i].kind() == Kind::Mosaic => Cursor::Arrow,
            (Some(_), _) => Cursor::Move,
            (None, Some(Tool::Text)) => Cursor::IBeam,
            (None, Some(Tool::Mosaic)) => mosaic::cursor(self),
            (None, Some(_)) => Cursor::Cross,
            (None, None) => return None,
        })
    }

    /// 每个标注在画面上的样子：范围（含选中框和抗锯齿，取整到像素）、颜色、粗细。
    /// 宿主改动前后各取一次，只重画变了的地方
    pub fn looks(&self) -> Vec<(Rect, usize, usize)> {
        let s = self.scale;
        let drawing = matches!(self.drag, Drag::Draw { .. }).then(|| self.doc.anns.len().wrapping_sub(1));
        let mut v: Vec<_> = self
            .doc
            .anns
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let pad = if self.sel == Some(i) { 7.0 * s } else { 2.0 };
                // 正在画的笔迹只报最后三个点那截（平滑后加一个点只改这截）：拖动时只重画新的一段，松手再整条重画一次
                let r = match &a.shape {
                    Shape::Pen(p) | Shape::Mosaic(p) if drawing == Some(i) => grow(span(p[p.len().saturating_sub(3)..].iter().copied()), a.width(s) / 2.0),
                    _ => a.bounds(s),
                };
                (outer(grow(r, pad)), a.color, a.size)
            })
            .collect();
        if let Some(r) = self.text_bounds() {
            let (c, sz) = PEN.get();
            v.push((outer(grow(r, 7.0 * s)), c, sz));
        }
        v
    }

    /// 画全部标注；`src` 是底图原图（马赛克从它取像素）。`frame` 是选区颜色：给出时画选中框和正在输入的文字
    pub fn draw(&self, c: &Canvas, src: &Bitmap, frame: Option<u32>) {
        let s = self.scale;
        for a in &self.doc.anns {
            draw_ann(c, a, s, src);
        }
        let Some(frame) = frame else { return };
        if let Some(a) = self.selected() {
            sel_box(c, a.bounds(s), s, frame);
        }
        self.draw_text_edit(c, frame);
    }

    /// 导出：把标注画进 out（底图上从 (ox, oy) 开始的一块拷贝），不画选中框
    pub fn burn(&self, out: &Bitmap, ox: i32, oy: i32, src: &Bitmap) {
        if self.doc.anns.is_empty() {
            return;
        }
        let c = Canvas::new(out, out.full(), false);
        c.set_transform(Affine::translation(-ox as f32, -oy as f32));
        self.draw(&c, src, None);
    }
}

/// 包住浮点范围的整像素矩形
fn outer(r: RectF) -> Rect {
    Rect { left: r.left.floor() as i32, top: r.top.floor() as i32, right: r.right.ceil() as i32, bottom: r.bottom.ceil() as i32 }
}

/// 按住 Shift 画正方形、正圆：取两边里长的那条
fn square(a: Pt, p: Pt) -> Pt {
    let (dx, dy) = (p.x - a.x, p.y - a.y);
    let m = dx.abs().max(dy.abs());
    Pt::new(a.x + m.copysign(dx), a.y + m.copysign(dy))
}

/// 按住 Shift 画箭头：角度吸到 45° 的倍数
fn snap45(a: Pt, p: Pt) -> Pt {
    let (dx, dy) = (p.x - a.x, p.y - a.y);
    let step = std::f32::consts::FRAC_PI_4;
    let ang = (dy.atan2(dx) / step).round() * step;
    let len = dx.hypot(dy);
    Pt::new(a.x + ang.cos() * len, a.y + ang.sin() * len)
}

fn draw_ann(c: &Canvas, a: &Ann, s: f32, src: &Bitmap) {
    let color = ui::rgb(COLORS[a.color]);
    let w = a.width(s);
    match &a.shape {
        Shape::Rect(p, q) => c.stroke_rect(span([*p, *q]), color, w, Stroke::Round),
        Shape::Ellipse(p, q) => {
            let r = span([*p, *q]);
            let (rx, ry) = (((r.right - r.left) / 2.0).max(0.5), ((r.bottom - r.top) / 2.0).max(0.5));
            c.stroke_ellipse(r.left + rx, r.top + ry, rx, ry, color, w, Stroke::Round);
        }
        Shape::Arrow(p, q) => {
            let len = p.dist(*q);
            if len < 1.0 {
                return;
            }
            let (ux, uy) = ((q.x - p.x) / len, (q.y - p.y) / len);
            let (hl, hw) = arrow_head(len, w, s);
            // 杆子收到头部里面一点，粗线的圆头不会从箭头尖两侧露出来
            c.line(p.xy(), (q.x - ux * hl * 0.7, q.y - uy * hl * 0.7), color, w, Stroke::Round);
            let (bx, by) = (q.x - ux * hl, q.y - uy * hl);
            if let Some(g) = path(&[*q, Pt::new(bx - uy * hw, by + ux * hw), Pt::new(bx + uy * hw, by - ux * hw)], true) {
                c.fill_path(&g, color);
            }
        }
        Shape::Pen(v) => {
            if let Some(g) = smooth(v) {
                c.stroke_path(&g, color, w, Stroke::Round);
            }
        }
        Shape::Text(at, t) => {
            if let Some(l) = text_layout(t, FONT[a.size] * s) {
                c.text(&l, at.x, at.y, color);
            }
        }
        Shape::Num(o, n) => {
            let d = NUM_D[a.size] * s;
            c.fill_ellipse(o.x, o.y, d / 2.0, d / 2.0, color);
            // 白色序号加一圈 30% 黑色细边；黄、白上用黑字
            if a.color == 5 {
                let lw = s.max(1.0);
                c.stroke_ellipse(o.x, o.y, (d - lw) / 2.0, (d - lw) / 2.0, ui::rgba(0, 0.3), lw, Stroke::Plain);
            }
            let fg = if a.color == 1 || a.color == 5 { COLORS[4] } else { 0xFFFFFF };
            if let Some(l) = Text::centered(Face::Ui, (d * 0.55).round(), true, &n.to_string(), d, d) {
                c.text(&l, o.x - d / 2.0, o.y - d / 2.0, ui::rgb(fg));
            }
        }
        Shape::Mosaic(v) => mosaic::draw(c, v, BRUSH[a.size] * s, BLOCK[a.size] * s, src),
    }
}

/// 折线（closed 时闭合并可填充）
fn path(pts: &[Pt], closed: bool) -> Option<Path> {
    let (first, rest) = pts.split_first()?;
    Path::new(closed, |b| {
        b.move_to(first.x, first.y);
        rest.iter().for_each(|p| b.line_to(p.x, p.y));
        if closed {
            b.close();
        }
        Some(())
    })
}

/// 画笔的笔迹：相邻两点的中点之间用二次贝塞尔连起来，折角变圆滑
pub fn smooth(pts: &[Pt]) -> Option<Path> {
    if pts.len() < 3 {
        return path(pts, false);
    }
    Path::new(false, |b| {
        b.move_to(pts[0].x, pts[0].y);
        for w in pts[1..].windows(2) {
            b.quad_to(w[0].x, w[0].y, (w[0].x + w[1].x) / 2.0, (w[0].y + w[1].y) / 2.0);
        }
        let last = pts[pts.len() - 1];
        b.line_to(last.x, last.y);
        Some(())
    })
}

/// 选中框：标注范围外扩 4.5，1 像素白线上叠选区颜色的虚线（4 实 3 空）
pub fn sel_box(c: &Canvas, r: RectF, s: f32, frame: u32) {
    let lw = s.round().max(1.0);
    // 整像素线宽画在像素中心才不糊
    let snap = |v: f32| (v - lw / 2.0).round() + lw / 2.0;
    let d = 4.5 * s;
    let r = rectf(snap(r.left - d), snap(r.top - d), snap(r.right + d), snap(r.bottom + d));
    c.stroke_rect(r, ui::rgb(0xFFFFFF), lw, Stroke::Plain);
    c.stroke_rect(r, ui::rgb(frame), lw, Stroke::Dash(&[4.0 * s / lw, 3.0 * s / lw]));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32) -> Ann {
        Ann { shape: Shape::Rect(Pt::new(x, 0.0), Pt::new(x + 10.0, 10.0)), color: 0, size: 1 }
    }

    #[test]
    fn undo_redo_100_steps() {
        let mut d = Doc::default();
        for i in 0..100 {
            d.checkpoint();
            d.anns.push(rect(i as f32));
            if i % 10 == 9 {
                // 中间夹着改动：改颜色、移动
                d.checkpoint();
                d.anns[0].color = i / 10 % 6;
                d.checkpoint();
                d.anns[1].offset(1.0, 1.0);
            }
        }
        let end = d.anns.clone();
        let mut n = 0;
        while d.undo() {
            n += 1;
        }
        assert_eq!((n, d.anns.len()), (120, 0));
        while d.redo() {}
        assert_eq!(d.anns, end);
        // 撤销 5 步（最后的移动、改色和 3 个矩形）再画新的，重做记录清掉
        for _ in 0..5 {
            d.undo();
        }
        assert_eq!(d.anns.len(), 97);
        d.checkpoint();
        d.anns.push(rect(500.0));
        assert!(!d.can_redo() && d.anns.len() == 98);
    }

    #[test]
    fn editor_draws_selects_moves() {
        let mut e = Editor::new(Doc::default(), 1.0);
        e.set_tool(Tool::Rect);
        // 手抖：不到 3 像素不要，也不留撤销记录
        assert!(e.down(Pt::new(10.0, 10.0), true));
        e.drag_to(Pt::new(12.5, 11.0), false);
        e.up();
        assert!(e.doc.anns.is_empty() && !e.doc.can_undo());
        // Shift 画正方形
        e.down(Pt::new(10.0, 10.0), true);
        e.drag_to(Pt::new(50.0, 30.0), true);
        e.up();
        assert_eq!(e.doc.anns[0].shape, Shape::Rect(Pt::new(10.0, 10.0), Pt::new(50.0, 50.0)));
        // 点在边上就是选中，拖动带撤销
        assert!(e.down(Pt::new(50.0, 30.0), true));
        assert_eq!(e.sel, Some(0));
        e.drag_to(Pt::new(60.0, 30.0), false);
        e.up();
        assert_eq!(e.doc.anns[0].shape, Shape::Rect(Pt::new(20.0, 10.0), Pt::new(60.0, 50.0)));
        // 改颜色只改选中的，并且能撤销
        e.set_color(3);
        assert_eq!(e.doc.anns[0].color, 3);
        assert!(e.undo() && e.doc.anns[0].color != 3);
        // 方框中间是空的：点进去不选中，画新的
        e.down(Pt::new(40.0, 30.0), true);
        assert_eq!(e.sel, None);
        e.up();
        // 序号自动递增，撤销后回退
        e.set_tool(Tool::Num);
        e.down(Pt::new(100.0, 100.0), true);
        e.down(Pt::new(150.0, 100.0), true);
        assert!(matches!(e.doc.anns.last().unwrap().shape, Shape::Num(_, 2)));
        e.undo();
        e.down(Pt::new(150.0, 100.0), true);
        assert!(matches!(e.doc.anns.last().unwrap().shape, Shape::Num(_, 2)));
        // 再按一次工具键放下工具
        assert!(e.key(Key::Char(b'N'), Mods::default()) && e.tool.is_none());
    }

    #[test]
    fn mosaic_selects_but_never_moves() {
        let mut e = Editor::new(Doc::default(), 1.0);
        e.set_tool(Tool::Mosaic);
        e.down(Pt::new(10.0, 10.0), true);
        e.drag_to(Pt::new(80.0, 10.0), false);
        e.up();
        // 选着马赛克工具时点在旧马赛克上是接着涂
        e.down(Pt::new(40.0, 10.0), true);
        e.up();
        assert_eq!((e.doc.anns.len(), e.sel), (2, None));
        // 放下工具后能选中、能删除，但拖不动
        e.set_tool(Tool::Mosaic);
        assert!(e.down(Pt::new(70.0, 10.0), true));
        assert_eq!(e.sel, Some(0));
        let before = e.doc.anns[0].clone();
        e.drag_to(Pt::new(70.0, 80.0), false);
        e.up();
        assert_eq!(e.doc.anns[0], before);
        assert!(e.key(Key::Delete, Mods::default()) && e.doc.anns.len() == 1);
    }

    /// 每种标注各画一个（含选中框），存成 target/test-tmp/annotate/shapes.png 对设计稿。平时不跑：
    /// cargo test annotate_samples -- --ignored
    #[test]
    #[ignore]
    fn annotate_samples() {
        let s = 1.5;
        let mut src = Bitmap::new(900, 300).unwrap();
        src.pixels_mut().fill(0xFF);
        let p = Pt::new;
        let a = |shape, color, size| Ann { shape, color, size };
        let mut e = Editor::new(
            Doc::with(vec![
                a(Shape::Rect(p(20.0, 20.0), p(160.0, 120.0)), 0, 0),
                a(Shape::Ellipse(p(190.0, 20.0), p(330.0, 120.0)), 1, 1),
                a(Shape::Arrow(p(360.0, 120.0), p(500.0, 30.0)), 2, 2),
                a(Shape::Pen(vec![p(530.0, 100.0), p(560.0, 30.0), p(600.0, 110.0), p(640.0, 40.0), p(680.0, 90.0)]), 3, 1),
                a(Shape::Text(p(20.0, 170.0), "文字标注 Text\n第二行".into()), 0, 1),
                a(Shape::Num(p(380.0, 220.0), 1), 0, 0),
                a(Shape::Num(p(440.0, 220.0), 2), 1, 1),
                a(Shape::Num(p(510.0, 220.0), 12), 5, 2),
                a(Shape::Num(p(580.0, 220.0), 3), 4, 1),
            ]),
            s,
        );
        e.sel = Some(1);
        let out = src.sub(&src.full()).unwrap();
        e.draw(&Canvas::new(&out, out.full(), false), &src, Some(0x9C44EC));
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("annotate");
        std::fs::create_dir_all(&dir).unwrap();
        let png = crate::plat::encode(&out.crop(&out.full()), false).unwrap();
        std::fs::write(dir.join("shapes.png"), png).unwrap();
        // 序号撤销后回退、文字范围有宽度
        e.sel = None;
        assert!(e.doc.anns[4].bounds(s).right > 100.0);
    }

    #[test]
    fn shift_snaps() {
        assert_eq!(square(Pt::new(0.0, 0.0), Pt::new(-30.0, 10.0)), Pt::new(-30.0, 30.0));
        let p = snap45(Pt::new(0.0, 0.0), Pt::new(100.0, 10.0));
        assert!((p.x - 100.5).abs() < 0.1 && p.y.abs() < 0.01, "{p:?}");
        let p = snap45(Pt::new(0.0, 0.0), Pt::new(50.0, 40.0));
        assert!((p.x - p.y).abs() < 0.01, "{p:?}");
    }
}
