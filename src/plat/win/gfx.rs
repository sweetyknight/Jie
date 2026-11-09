//! 绘制：Direct2D 软件渲染目标 + DirectWrite，全在 CPU 上画，不创建 D3D 设备；贴大位图用 GDI。
//! 坐标一律是物理像素（渲染目标 DPI 设成 96），尺寸由调用方按缩放比例换算。
// 画图的参数天然就多（坐标、半径、颜色、线宽、样式），拆成结构体反而难读
#![allow(clippy::too_many_arguments)]

use super::Bitmap;
use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::core::{BOOL, HSTRING};
use windows_numerics::{Matrix3x2, Vector2};

/// 颜色、浮点矩形、仿射变换：直接用 Direct2D 的，字段名和 mac 上的同名结构一样
pub type Color = D2D1_COLOR_F;
pub type RectF = D2D_RECT_F;
pub type Affine = Matrix3x2;

thread_local! {
    // 系统组件，创建失败说明系统坏了。多留一个引用永不释放：4K 整屏软件渲染后 D2D 起了工作线程，
    // 在线程退出时最后一次释放工厂会卡死，之后再建工厂也跟着等死（测试一个线程一个用例时实测）。程序里只有界面线程用，只漏这一个
    static D2D: ID2D1Factory = {
        let f: ID2D1Factory = unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }.expect("Direct2D");
        std::mem::forget(f.clone());
        f
    };
    /// 用完还回来的渲染目标：(保留透明度, 目标)；建一个要几十微秒，重画频繁，留着复用
    static TARGETS: RefCell<Vec<(bool, ID2D1DCRenderTarget)>> = const { RefCell::new(Vec::new()) };
    static ROUND: ID2D1StrokeStyle = unsafe {
        d2d().CreateStrokeStyle(
            &D2D1_STROKE_STYLE_PROPERTIES { startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, lineJoin: D2D1_LINE_JOIN_ROUND, ..Default::default() },
            None,
        )
    }
    .expect("Direct2D");
}

fn dwrite() -> &'static IDWriteFactory {
    // 共享工厂可以跨线程用
    struct F(IDWriteFactory);
    unsafe impl Send for F {}
    unsafe impl Sync for F {}
    static DW: OnceLock<F> = OnceLock::new();
    &DW.get_or_init(|| F(unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }.expect("DirectWrite"))).0
}

pub(crate) fn d2d() -> ID2D1Factory {
    D2D.with(|f| f.clone())
}

/// 启动时先加载 D2D、DirectWrite 并查好字体，第一次截图才能在 100 ms 内出遮罩
pub fn warm_up() {
    // 在一张小位图上真的画一次字，软件光栅化和字形缓存都初始化好
    let Some(bmp) = Bitmap::new(16, 16) else { return };
    if let Some(l) = Text::tabular(Face::Ui, 12.0, false, "0 × 9") {
        Canvas::new(&bmp, bmp.full(), false).text(&l, 0.0, 0.0, Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 });
    }
}

/// 软件渲染目标：D2D 在 CPU 上画进自己的缓冲区，再用 GDI 贴到绑定的 DC，不创建 D3D 设备。
/// 绑定时读回 DC 里已有的内容，所以可以叠在 GDI 画好的底图上画。`alpha` 为 true 时保留透明度（分层窗口用）。
fn dc_render_target(alpha: bool) -> Option<ID2D1DCRenderTarget> {
    if let Some(rt) = TARGETS.with_borrow_mut(|v| v.iter().position(|t| t.0 == alpha).map(|i| v.swap_remove(i).1)) {
        return Some(rt);
    }
    let props = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: if alpha { D2D1_ALPHA_MODE_PREMULTIPLIED } else { D2D1_ALPHA_MODE_IGNORE },
        },
        dpiX: 96.0,
        dpiY: 96.0,
        ..Default::default()
    };
    unsafe { d2d().CreateDCRenderTarget(&props) }.ok()
}

/// 描边的样子：普通（平头、尖角），圆头圆角，或者虚线（实、空的长度是线宽的倍数，平头）
#[derive(Clone, Copy)]
pub enum Stroke<'a> {
    Plain,
    Round,
    Dash(&'a [f32]),
}

/// 一次绘制：画到一张位图（或窗口重画时的 DC）的 `area` 这块上，区域外的不画。
/// 坐标按整张位图（窗口重画时按窗口内容区）算。图形、文字用 Direct2D；贴位图、填整像素的块用 GDI，两种交替时自动切换。
pub struct Canvas<'a> {
    dc: HDC,
    area: RECT,
    /// DC 左上角那个像素在调用方坐标里的位置（窗口重画时画进的是小块离屏位图）
    org: (i32, i32),
    rt: Option<ID2D1DCRenderTarget>,
    alpha: bool,
    /// Direct2D 正在画（BeginDraw 了还没 EndDraw）
    open: Cell<bool>,
    /// 调用方设的变换；交给 D2D 时再乘上「平移到 area 左上角」
    user: Cell<Matrix3x2>,
    _bmp: std::marker::PhantomData<&'a Bitmap>,
}

impl<'a> Canvas<'a> {
    /// 画到位图 `bmp` 的 `area` 这块；`alpha` 为 true 时保留透明度（分层窗口的画面）
    pub fn new(bmp: &'a Bitmap, area: RECT, alpha: bool) -> Canvas<'a> {
        Self::dc(bmp.dc, area, alpha)
    }

    /// 画到一个 GDI DC（菜单项自绘时系统给的）
    pub(crate) fn dc(dc: HDC, area: RECT, alpha: bool) -> Canvas<'a> {
        Self::at(dc, area, (0, 0), alpha)
    }

    /// 位图 `bmp` 的左上角当作调用方坐标里的 `org`，画 `area` 这块（窗口重画时用离屏小位图）
    pub(crate) fn offset(bmp: &'a Bitmap, area: RECT, org: (i32, i32)) -> Canvas<'a> {
        Self::at(bmp.dc, area, org, false)
    }

    fn at(dc: HDC, area: RECT, org: (i32, i32), alpha: bool) -> Canvas<'a> {
        let rt = dc_render_target(alpha);
        Canvas { dc, area, org, rt, alpha, open: Cell::new(false), user: Cell::new(Matrix3x2::identity()), _bmp: std::marker::PhantomData }
    }

    /// 调用方坐标 → DC 的像素坐标
    fn dev(&self, r: RECT) -> RECT {
        RECT { left: r.left - self.org.0, top: r.top - self.org.1, right: r.right - self.org.0, bottom: r.bottom - self.org.1 }
    }

    /// 这次画的区域（整张位图的坐标）
    pub fn area(&self) -> RECT {
        self.area
    }

    /// 打开 Direct2D：绑定（读回 DC 现有的内容）、开始画，设好变换；已经开着就直接返回
    fn d2d(&self) -> Option<&ID2D1DCRenderTarget> {
        let rt = self.rt.as_ref()?;
        if !self.open.get() {
            unsafe {
                rt.BindDC(self.dc, &self.dev(self.area)).ok()?;
                rt.BeginDraw();
                // 浮层和分层窗口上 ClearType 会出彩边，统一用灰度
                rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            }
            self.open.set(true);
            self.apply();
        }
        Some(rt)
    }

    fn apply(&self) {
        if let Some(rt) = &self.rt
            && self.open.get()
        {
            unsafe { rt.SetTransform(&(self.user.get() * Matrix3x2::translation(-self.area.left as f32, -self.area.top as f32))) };
        }
    }

    /// 关掉 Direct2D，把它画的写回 DC；GDI 画之前、画完都要调
    fn flush(&self) {
        if self.open.replace(false)
            && let Some(rt) = &self.rt
        {
            let _ = unsafe { rt.EndDraw(None, None) };
        }
    }

    pub fn transform(&self) -> Affine {
        self.user.get()
    }

    pub fn set_transform(&self, m: Affine) {
        self.user.set(m);
        self.apply();
    }

    /// 这次画的区域换算到当前变换下的坐标（外接矩形）：只画看得见的部分时用
    pub fn visible(&self) -> RectF {
        let mut m = self.user.get();
        if !unsafe { D2D1InvertMatrix(&mut m) }.as_bool() {
            return RectF::default();
        }
        let a = self.area;
        let pts = [(a.left, a.top), (a.right, a.top), (a.left, a.bottom), (a.right, a.bottom)].map(|(x, y)| (x as f32, y as f32));
        let f = |(x, y): (f32, f32)| (x * m.M11 + y * m.M21 + m.M31, x * m.M12 + y * m.M22 + m.M32);
        pts.iter().map(|&p| f(p)).fold(RectF { left: f32::MAX, top: f32::MAX, right: f32::MIN, bottom: f32::MIN }, |r, (x, y)| RectF {
            left: r.left.min(x),
            top: r.top.min(y),
            right: r.right.max(x),
            bottom: r.bottom.max(y),
        })
    }

    fn brush(&self, c: Color) -> Option<(&ID2D1DCRenderTarget, ID2D1SolidColorBrush)> {
        let rt = self.d2d()?;
        Some((rt, unsafe { rt.CreateSolidColorBrush(&c, None) }.ok()?))
    }

    fn style(&self, s: Stroke) -> Option<ID2D1StrokeStyle> {
        match s {
            Stroke::Plain => None,
            Stroke::Round => Some(ROUND.with(|r| r.clone())),
            Stroke::Dash(d) => {
                let props = D2D1_STROKE_STYLE_PROPERTIES { dashStyle: D2D1_DASH_STYLE_CUSTOM, ..Default::default() };
                unsafe { d2d().CreateStrokeStyle(&props, Some(d)) }.ok()
            }
        }
    }

    pub fn clear(&self, c: Color) {
        if let Some(rt) = self.d2d() {
            unsafe { rt.Clear(Some(&c)) };
        }
    }

    pub fn fill_rect(&self, r: RectF, c: Color) {
        if let Some((rt, b)) = self.brush(c) {
            unsafe { rt.FillRectangle(&r, &b) };
        }
    }

    pub fn stroke_rect(&self, r: RectF, c: Color, w: f32, s: Stroke) {
        if let Some((rt, b)) = self.brush(c) {
            unsafe { rt.DrawRectangle(&r, &b, w, self.style(s).as_ref()) };
        }
    }

    pub fn fill_round(&self, r: RectF, radius: f32, c: Color) {
        if let Some((rt, b)) = self.brush(c) {
            unsafe { rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }, &b) };
        }
    }

    pub fn stroke_round(&self, r: RectF, radius: f32, c: Color, w: f32) {
        if let Some((rt, b)) = self.brush(c) {
            unsafe { rt.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }, &b, w, None) };
        }
    }

    pub fn fill_ellipse(&self, cx: f32, cy: f32, rx: f32, ry: f32, c: Color) {
        if let Some((rt, b)) = self.brush(c) {
            unsafe { rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: rx, radiusY: ry }, &b) };
        }
    }

    pub fn stroke_ellipse(&self, cx: f32, cy: f32, rx: f32, ry: f32, c: Color, w: f32, s: Stroke) {
        if let Some((rt, b)) = self.brush(c) {
            unsafe { rt.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: rx, radiusY: ry }, &b, w, self.style(s).as_ref()) };
        }
    }

    pub fn line(&self, a: (f32, f32), b: (f32, f32), c: Color, w: f32, s: Stroke) {
        if let Some((rt, br)) = self.brush(c) {
            unsafe { rt.DrawLine(Vector2 { X: a.0, Y: a.1 }, Vector2 { X: b.0, Y: b.1 }, &br, w, self.style(s).as_ref()) };
        }
    }

    pub fn fill_path(&self, p: &Path, c: Color) {
        if let Some((rt, b)) = self.brush(c) {
            unsafe { rt.FillGeometry(&p.0, &b, None) };
        }
    }

    pub fn stroke_path(&self, p: &Path, c: Color, w: f32, s: Stroke) {
        if let Some((rt, b)) = self.brush(c) {
            unsafe { rt.DrawGeometry(&p.0, &b, w, self.style(s).as_ref()) };
        }
    }

    /// 沿路径用 `cols × rows` 的格子图描宽 `w` 的圆头线：格子图每格一个像素（BGRA，不透明），
    /// 放大 `cell` 倍、左上角在 `origin`（当前坐标），最近邻取样。马赛克用
    pub fn stroke_cells(&self, p: &Path, w: f32, cells: &[u8], cols: u32, rows: u32, cell: f32, origin: (f32, f32)) {
        let Some(rt) = self.d2d() else { return };
        unsafe {
            let props = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            let size = D2D_SIZE_U { width: cols, height: rows };
            let Ok(bmp) = rt.CreateBitmap(size, Some(cells.as_ptr() as _), cols * 4, &props) else { return };
            let bp = D2D1_BITMAP_BRUSH_PROPERTIES {
                extendModeX: D2D1_EXTEND_MODE_CLAMP,
                extendModeY: D2D1_EXTEND_MODE_CLAMP,
                interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
            };
            let Ok(fill) = rt.CreateBitmapBrush(&bmp, Some(&bp), None) else { return };
            // 画刷坐标跟着当前变换走
            fill.SetTransform(&Matrix3x2 { M11: cell, M12: 0.0, M21: 0.0, M22: cell, M31: origin.0, M32: origin.1 });
            rt.DrawGeometry(&p.0, &fill, w, &ROUND.with(|r| r.clone()));
        }
    }

    /// 把 `w × h` 的 BGRA 像素（不透明）最近邻放进 `dst`（当前坐标）
    pub fn pixels(&self, px: &[u8], w: u32, h: u32, dst: RectF) {
        let Some(rt) = self.d2d() else { return };
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        unsafe {
            if let Ok(b) = rt.CreateBitmap(D2D_SIZE_U { width: w, height: h }, Some(px.as_ptr() as _), w * 4, &props) {
                rt.DrawBitmap(&b, Some(&dst), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None);
            }
        }
    }

    pub fn text(&self, t: &Text, x: f32, y: f32, c: Color) {
        if let Some((rt, b)) = self.brush(c) {
            unsafe { rt.DrawTextLayout(Vector2 { X: x, Y: y }, &t.layout, &b, D2D1_DRAW_TEXT_OPTIONS_NONE) };
        }
    }

    /// 之后画的裁在 r 里（当前坐标，不抗锯齿），配一个 `pop_clip`；中间只能用 Direct2D 画的那些
    pub fn push_clip(&self, r: RectF) {
        if let Some(rt) = self.d2d() {
            unsafe { rt.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED) };
        }
    }

    pub fn pop_clip(&self) {
        if let Some(rt) = self.d2d() {
            unsafe { rt.PopAxisAlignedClip() };
        }
    }

    // ---- 整像素的块：不经过变换，坐标按整张位图算 ----

    /// 把 `src` 的 `from` 这块原样贴到 (x, y)
    pub fn blit(&self, src: &Bitmap, from: RECT, x: i32, y: i32) {
        self.flush();
        let (x, y) = (x - self.org.0, y - self.org.1);
        let _ = unsafe { BitBlt(self.dc, x, y, from.right - from.left, from.bottom - from.top, Some(src.dc), from.left, from.top, SRCCOPY) };
    }

    /// 把整张 `src` 最近邻缩放进 `dst`
    pub fn blit_scaled(&self, src: &Bitmap, dst: RECT) {
        self.flush();
        let d = self.dev(dst);
        unsafe {
            SetStretchBltMode(self.dc, COLORONCOLOR);
            let _ = StretchBlt(self.dc, d.left, d.top, d.right - d.left, d.bottom - d.top, Some(src.dc), 0, 0, src.w, src.h, SRCCOPY);
        }
    }

    /// 用 0xRRGGBB 填满整像素的 r
    pub fn fill_px(&self, r: RECT, rgb: u32) {
        self.flush();
        unsafe {
            let b = CreateSolidBrush(COLORREF(((rgb & 0xFF) << 16) | (rgb & 0xFF00) | (rgb >> 16)));
            FillRect(self.dc, &self.dev(r), b);
            let _ = DeleteObject(b.into());
        }
    }
}

impl Drop for Canvas<'_> {
    fn drop(&mut self) {
        self.flush();
        if let Some(rt) = self.rt.take() {
            TARGETS.with_borrow_mut(|v| v.push((self.alpha, rt)));
        }
    }
}

// ---- 路径 ----

/// 几何路径：折线、贝塞尔、圆弧，可以填充或描边
pub struct Path(ID2D1PathGeometry);

/// 建路径时一笔一笔加；`move_to` 开新的一段，`close` 闭合当前这段
pub struct PathBuilder {
    sink: ID2D1GeometrySink,
    filled: bool,
    open: bool,
}

fn v(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

impl PathBuilder {
    pub fn move_to(&mut self, x: f32, y: f32) {
        unsafe {
            if self.open {
                self.sink.EndFigure(D2D1_FIGURE_END_OPEN);
            }
            self.sink.BeginFigure(v(x, y), if self.filled { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
        }
        self.open = true;
    }

    pub fn line_to(&mut self, x: f32, y: f32) {
        unsafe { self.sink.AddLine(v(x, y)) };
    }

    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        unsafe { self.sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: v(cx, cy), point2: v(x, y) }) };
    }

    pub fn cubic_to(&mut self, c1: (f32, f32), c2: (f32, f32), x: f32, y: f32) {
        unsafe { self.sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: v(c1.0, c1.1), point2: v(c2.0, c2.1), point3: v(x, y) }) };
    }

    /// SVG 的圆弧（终点参数化）：半径、x 轴转角（度）、大弧、顺时针（y 向下的坐标里）、终点
    pub fn arc_to(&mut self, rx: f32, ry: f32, rot: f32, large: bool, sweep: bool, x: f32, y: f32) {
        unsafe {
            self.sink.AddArc(&D2D1_ARC_SEGMENT {
                point: v(x, y),
                size: D2D_SIZE_F { width: rx, height: ry },
                rotationAngle: rot,
                sweepDirection: if sweep { D2D1_SWEEP_DIRECTION_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE },
                arcSize: if large { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            })
        };
    }

    pub fn close(&mut self) {
        if self.open {
            unsafe { self.sink.EndFigure(D2D1_FIGURE_END_CLOSED) };
            self.open = false;
        }
    }
}

impl Path {
    /// 用 `f` 一笔一笔建路径；`filled` 为 true 的路径能填充。`f` 返回 None 表示数据不对，整条不要
    pub fn new(filled: bool, f: impl FnOnce(&mut PathBuilder) -> Option<()>) -> Option<Path> {
        unsafe {
            let g = d2d().CreatePathGeometry().ok()?;
            let mut b = PathBuilder { sink: g.Open().ok()?, filled, open: false };
            f(&mut b)?;
            if b.open {
                b.sink.EndFigure(D2D1_FIGURE_END_OPEN);
            }
            b.sink.Close().ok()?;
            Some(Path(g))
        }
    }
}

// ---- 文字 ----

/// 字体：界面字（Win11 是 Segoe UI Variable Text）、Segoe UI（设置窗口和菜单）、中文（标注的文字）、等宽
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    Ui,
    Segoe,
    Cjk,
    /// 目前只有取字的样张用
    #[cfg_attr(not(test), allow(dead_code))]
    Mono,
}

fn family(f: Face) -> &'static str {
    match f {
        Face::Ui => ui_font(),
        Face::Segoe => "Segoe UI",
        Face::Cjk => "Microsoft YaHei UI",
        Face::Mono => "Consolas",
    }
}

/// 界面字体：Win11 用 Segoe UI Variable Text，Win10 没有就用 Segoe UI
fn ui_font() -> &'static str {
    static F: OnceLock<&str> = OnceLock::new();
    F.get_or_init(|| {
        let mut exists = BOOL(0);
        unsafe {
            let mut coll = None;
            if dwrite().GetSystemFontCollection(&mut coll, false).is_ok()
                && let Some(c) = coll
            {
                let mut i = 0;
                let _ = c.FindFamilyName(&HSTRING::from("Segoe UI Variable Text"), &mut i, &mut exists);
            }
        }
        if exists.as_bool() { "Segoe UI Variable Text" } else { "Segoe UI" }
    })
}

/// 文字格式；中文字符靠系统字体回退，zh-CN 让回退选到微软雅黑
fn format(face: Face, px: f32, bold: bool) -> Option<IDWriteTextFormat> {
    let weight = if bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL };
    unsafe { dwrite().CreateTextFormat(&HSTRING::from(family(face)), None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, px, &HSTRING::from("zh-CN")) }.ok()
}

/// 排好版的一段文字。位置都是 UTF-16 下标
pub struct Text {
    layout: IDWriteTextLayout,
    fmt: IDWriteTextFormat,
}

impl Text {
    fn make(face: Face, px: f32, bold: bool, s: &str, w: f32, h: f32) -> Option<Text> {
        let fmt = format(face, px, bold)?;
        let s: Vec<u16> = s.encode_utf16().collect();
        let layout = unsafe { dwrite().CreateTextLayout(&s, &fmt, w, h) }.ok()?;
        Some(Text { layout, fmt })
    }

    /// 一行字，不折行
    pub fn new(face: Face, px: f32, bold: bool, s: &str) -> Option<Text> {
        Self::make(face, px, bold, s, 10000.0, 10000.0)
    }

    /// 一行字，数字等宽（尺寸读数用）
    pub fn tabular(face: Face, px: f32, bold: bool, s: &str) -> Option<Text> {
        let t = Self::new(face, px, bold, s)?;
        unsafe {
            let ty = dwrite().CreateTypography().ok()?;
            ty.AddFontFeature(DWRITE_FONT_FEATURE { nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, parameter: 1 }).ok()?;
            t.layout.SetTypography(&ty, DWRITE_TEXT_RANGE { startPosition: 0, length: s.encode_utf16().count() as u32 }).ok()?;
        }
        Some(t)
    }

    /// 按宽度折行；`line_h` 给定时用固定行高（基线在行高的 0.76 处，接近 CSS 的效果）
    pub fn wrapped(face: Face, px: f32, bold: bool, s: &str, width: f32, line_h: Option<f32>) -> Option<Text> {
        let t = Self::make(face, px, bold, s, width.max(1.0), 100_000.0)?;
        unsafe {
            t.layout.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP).ok()?;
            if let Some(h) = line_h {
                t.layout.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, h, h * 0.76).ok()?;
            }
        }
        Some(t)
    }

    /// 一行字在 w × h 的框里水平、竖直都居中（序号用）
    pub fn centered(face: Face, px: f32, bold: bool, s: &str, w: f32, h: f32) -> Option<Text> {
        let t = Self::new(face, px, bold, s)?;
        unsafe {
            let _ = t.layout.SetMaxWidth(w);
            let _ = t.layout.SetMaxHeight(h);
            let _ = t.layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = t.layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        }
        Some(t)
    }

    /// 超过 `width` 时截断并加省略号
    pub fn truncate(&self, width: f32) {
        unsafe {
            let _ = self.layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let _ = self.layout.SetMaxWidth(width.max(1.0));
            if let Ok(sign) = dwrite().CreateEllipsisTrimmingSign(&self.fmt) {
                let t = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                let _ = self.layout.SetTrimming(&t, &sign);
            }
        }
    }

    fn metrics(&self) -> DWRITE_TEXT_METRICS {
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = unsafe { self.layout.GetMetrics(&mut m) };
        m
    }

    /// 宽度算上末尾的空格（输入框的光标要能停在空格后面）
    #[allow(clippy::misnamed_getters)]
    pub fn width(&self) -> f32 {
        self.metrics().widthIncludingTrailingWhitespace
    }

    pub fn height(&self) -> f32 {
        self.metrics().height
    }

    /// 第一行的高
    pub fn first_line_height(&self) -> f32 {
        // 缓冲区比行数少时 GetLineMetrics 一行都不填，先问行数
        let mut n = 0;
        let _ = unsafe { self.layout.GetLineMetrics(None, &mut n) };
        let mut m = vec![DWRITE_LINE_METRICS::default(); n as usize];
        let _ = unsafe { self.layout.GetLineMetrics(Some(&mut m), &mut n) };
        m.first().map_or(0.0, |m| m.height)
    }

    /// 点 (x, y)（相对文字左上角）落在哪个字符位置；点在字外面时取最近的
    pub fn hit(&self, x: f32, y: f32) -> u32 {
        let (mut trailing, mut inside, mut m) = (BOOL(0), BOOL(0), DWRITE_HIT_TEST_METRICS::default());
        let _ = unsafe { self.layout.HitTestPoint(x, y, &mut trailing, &mut inside, &mut m) };
        m.textPosition + if trailing.as_bool() { m.length } else { 0 }
    }

    /// 字符位置 [a, b) 占的矩形（每行一个，相对文字左上角）：(左, 顶, 宽, 高)
    pub fn range_rects(&self, a: u32, b: u32) -> Vec<(f32, f32, f32, f32)> {
        let mut n = 0;
        let _ = unsafe { self.layout.HitTestTextRange(a, b - a, 0.0, 0.0, None, &mut n) };
        let mut ms = vec![DWRITE_HIT_TEST_METRICS::default(); n as usize];
        if unsafe { self.layout.HitTestTextRange(a, b - a, 0.0, 0.0, Some(&mut ms), &mut n) }.is_err() {
            return Vec::new();
        }
        ms[..n as usize].iter().map(|m| (m.left, m.top, m.width, m.height)).collect()
    }

    /// 插入光标在字符位置 pos 前面时的 (x, 顶, 高)，相对文字左上角
    pub fn caret(&self, pos: u32) -> (f32, f32, f32) {
        let (mut x, mut y, mut m) = (0.0, 0.0, DWRITE_HIT_TEST_METRICS::default());
        let _ = unsafe { self.layout.HitTestTextPosition(pos, false, &mut x, &mut y, &mut m) };
        (x, m.top, m.height)
    }

    /// 给 [start, start + len) 加下划线（输入法的组合串）
    pub fn underline(&self, start: u32, len: u32) {
        let _ = unsafe { self.layout.SetUnderline(true, DWRITE_TEXT_RANGE { startPosition: start, length: len }) };
    }
}
