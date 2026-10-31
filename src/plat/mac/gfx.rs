use super::Rect;
use objc2_core_foundation::{
    CFArray, CFAttributedString, CFBoolean, CFData, CFDictionary, CFNumber, CFRange, CFRetained, CFString, CFType, CGAffineTransform, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapInfo, CGColor, CGColorSpace, CGContext, CGDataProvider, CGImage, CGImageAlphaInfo, CGInterpolationQuality, CGLineCap,
    CGLineJoin, CGMutablePath, CGPathDrawingMode, kCGColorSpaceSRGB,
};
use objc2_core_text::{
    CTFont, CTFontDescriptor, CTFontSymbolicTraits, CTFontUIFontType, CTLine, CTLineTruncationType, CTTypesetter, kCTFontAttributeName,
    kCTFontFeatureSettingsAttribute, kCTFontOpenTypeFeatureTag, kCTFontOpenTypeFeatureValue, kCTForegroundColorFromContextAttributeName,
};
use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::ops::Mul;

pub type Color = ColorF;
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct ColorF {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct RectF {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
#[allow(non_snake_case)]
pub struct Affine {
    pub M11: f32,
    pub M12: f32,
    pub M21: f32,
    pub M22: f32,
    pub M31: f32,
    pub M32: f32,
}
impl Affine {
    pub const fn identity() -> Self {
        Self { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 }
    }
    pub const fn translation(x: f32, y: f32) -> Self {
        Self { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: x, M32: y }
    }
}
impl Mul for Affine {
    type Output = Self;
    fn mul(self, b: Self) -> Self {
        Self {
            M11: self.M11 * b.M11 + self.M12 * b.M21,
            M12: self.M11 * b.M12 + self.M12 * b.M22,
            M21: self.M21 * b.M11 + self.M22 * b.M21,
            M22: self.M21 * b.M12 + self.M22 * b.M22,
            M31: self.M31 * b.M11 + self.M32 * b.M21 + b.M31,
            M32: self.M31 * b.M12 + self.M32 * b.M22 + b.M32,
        }
    }
}

#[derive(Clone, Copy)]
pub enum Stroke<'a> {
    Plain,
    Round,
    Dash(&'a [f32]),
}

pub struct Canvas<'a> {
    cg: &'a CGContext,
    area: Rect,
    user: Cell<Affine>,
    clips: Cell<usize>,
    _keep: PhantomData<&'a ()>,
}

pub(crate) fn context_canvas<'a>(cg: &'a CGContext, area: Rect) -> Canvas<'a> {
    CGContext::save_g_state(Some(cg));
    CGContext::clip_to_rect(Some(cg), recti(area));
    Canvas { cg, area, user: Cell::new(Affine::identity()), clips: Cell::new(0), _keep: PhantomData }
}

impl<'a> Canvas<'a> {
    pub fn new(bmp: &'a Bitmap, area: Rect, _alpha: bool) -> Canvas<'a> {
        let mut c = context_canvas(&bmp.ctx, bmp.full());
        CGContext::translate_ctm(Some(c.cg), 0.0, bmp.h as f64);
        CGContext::scale_ctm(Some(c.cg), 1.0, -1.0);
        CGContext::clip_to_rect(Some(c.cg), recti(area));
        c.area = area;
        c
    }
    pub fn area(&self) -> Rect {
        self.area
    }
    pub fn transform(&self) -> Affine {
        self.user.get()
    }
    pub fn set_transform(&self, m: Affine) {
        self.user.set(m);
    }
    pub fn visible(&self) -> RectF {
        let m = self.user.get();
        let det = m.M11 * m.M22 - m.M12 * m.M21;
        if det.abs() < 0.0001 {
            return RectF::default();
        }
        let inv = |x: f32, y: f32| (((x - m.M31) * m.M22 - (y - m.M32) * m.M21) / det, ((y - m.M32) * m.M11 - (x - m.M31) * m.M12) / det);
        let p = [
            inv(self.area.left as f32, self.area.top as f32),
            inv(self.area.right as f32, self.area.top as f32),
            inv(self.area.left as f32, self.area.bottom as f32),
            inv(self.area.right as f32, self.area.bottom as f32),
        ];
        RectF {
            left: p.iter().map(|p| p.0).fold(f32::INFINITY, f32::min),
            top: p.iter().map(|p| p.1).fold(f32::INFINITY, f32::min),
            right: p.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max),
            bottom: p.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max),
        }
    }
    fn apply(&self) {
        let m = self.user.get();
        CGContext::concat_ctm(
            Some(self.cg),
            CGAffineTransform { a: m.M11 as f64, b: m.M12 as f64, c: m.M21 as f64, d: m.M22 as f64, tx: m.M31 as f64, ty: m.M32 as f64 },
        );
    }
    fn color(c: Color) -> CFRetained<CGColor> {
        CGColor::new_srgb(c.r as f64, c.g as f64, c.b as f64, c.a as f64)
    }
    fn path(&self, p: &Path) {
        CGContext::add_path(Some(self.cg), Some(&p.0));
    }
    pub fn clear(&self, c: Color) {
        CGContext::save_g_state(Some(self.cg));
        CGContext::set_blend_mode(Some(self.cg), objc2_core_graphics::CGBlendMode::Copy);
        CGContext::set_rgb_fill_color(Some(self.cg), c.r as f64, c.g as f64, c.b as f64, c.a as f64);
        CGContext::fill_rect(Some(self.cg), recti(self.area));
        CGContext::restore_g_state(Some(self.cg));
    }
    pub fn fill_rect(&self, r: RectF, c: Color) {
        CGContext::save_g_state(Some(self.cg));
        self.apply();
        CGContext::set_fill_color_with_color(Some(self.cg), Some(&Self::color(c)));
        CGContext::fill_rect(Some(self.cg), rect(r));
        CGContext::restore_g_state(Some(self.cg));
    }
    pub fn stroke_rect(&self, r: RectF, c: Color, w: f32, s: Stroke) {
        CGContext::save_g_state(Some(self.cg));
        self.apply();
        self.stroke_setup(c, w, s);
        CGContext::stroke_rect(Some(self.cg), rect(r));
        CGContext::restore_g_state(Some(self.cg));
    }
    pub fn fill_round(&self, r: RectF, radius: f32, c: Color) {
        let p = rounded_path(r, radius);
        CGContext::save_g_state(Some(self.cg));
        self.apply();
        CGContext::set_fill_color_with_color(Some(self.cg), Some(&Self::color(c)));
        self.path(&Path(p));
        CGContext::draw_path(Some(self.cg), CGPathDrawingMode::Fill);
        CGContext::restore_g_state(Some(self.cg));
    }
    pub fn stroke_round(&self, r: RectF, radius: f32, c: Color, w: f32) {
        let p = rounded_path(r, radius);
        CGContext::save_g_state(Some(self.cg));
        self.apply();
        self.stroke_setup(c, w, Stroke::Round);
        self.path(&Path(p));
        CGContext::draw_path(Some(self.cg), CGPathDrawingMode::Stroke);
        CGContext::restore_g_state(Some(self.cg));
    }
    pub fn fill_ellipse(&self, cx: f32, cy: f32, rx: f32, ry: f32, c: Color) {
        CGContext::save_g_state(Some(self.cg));
        self.apply();
        CGContext::set_fill_color_with_color(Some(self.cg), Some(&Self::color(c)));
        CGContext::fill_ellipse_in_rect(
            Some(self.cg),
            CGRect::new(CGPoint::new((cx - rx) as f64, (cy - ry) as f64), CGSize::new((rx * 2.0) as f64, (ry * 2.0) as f64)),
        );
        CGContext::restore_g_state(Some(self.cg));
    }
    pub fn stroke_ellipse(&self, cx: f32, cy: f32, rx: f32, ry: f32, c: Color, w: f32, s: Stroke) {
        CGContext::save_g_state(Some(self.cg));
        self.apply();
        self.stroke_setup(c, w, s);
        CGContext::stroke_ellipse_in_rect(
            Some(self.cg),
            CGRect::new(CGPoint::new((cx - rx) as f64, (cy - ry) as f64), CGSize::new((rx * 2.0) as f64, (ry * 2.0) as f64)),
        );
        CGContext::restore_g_state(Some(self.cg));
    }
    pub fn line(&self, a: (f32, f32), b: (f32, f32), c: Color, w: f32, s: Stroke) {
        CGContext::save_g_state(Some(self.cg));
        self.apply();
        self.stroke_setup(c, w, s);
        CGContext::begin_path(Some(self.cg));
        CGContext::move_to_point(Some(self.cg), a.0 as f64, a.1 as f64);
        CGContext::add_line_to_point(Some(self.cg), b.0 as f64, b.1 as f64);
        CGContext::stroke_path(Some(self.cg));
        CGContext::restore_g_state(Some(self.cg));
    }
    fn stroke_setup(&self, c: Color, w: f32, s: Stroke) {
        CGContext::set_stroke_color_with_color(Some(self.cg), Some(&Self::color(c)));
        CGContext::set_line_width(Some(self.cg), w as f64);
        CGContext::set_line_cap(Some(self.cg), CGLineCap::Butt);
        CGContext::set_line_join(Some(self.cg), CGLineJoin::Miter);
        unsafe {
            CGContext::set_line_dash(Some(self.cg), 0.0, std::ptr::null(), 0);
        }
        match s {
            Stroke::Round => {
                CGContext::set_line_cap(Some(self.cg), CGLineCap::Round);
                CGContext::set_line_join(Some(self.cg), CGLineJoin::Round);
            }
            Stroke::Dash(d) => {
                let a: Vec<f64> = d.iter().map(|x| (*x * w) as f64).collect();
                unsafe {
                    CGContext::set_line_dash(Some(self.cg), 0.0, a.as_ptr(), a.len());
                }
            }
            Stroke::Plain => {}
        }
    }
    pub fn fill_path(&self, p: &Path, c: Color) {
        CGContext::save_g_state(Some(self.cg));
        self.apply();
        CGContext::set_fill_color_with_color(Some(self.cg), Some(&Self::color(c)));
        self.path(p);
        CGContext::draw_path(Some(self.cg), CGPathDrawingMode::Fill);
        CGContext::restore_g_state(Some(self.cg));
    }
    pub fn stroke_path(&self, p: &Path, c: Color, w: f32, s: Stroke) {
        CGContext::save_g_state(Some(self.cg));
        self.apply();
        self.stroke_setup(c, w, s);
        self.path(p);
        CGContext::draw_path(Some(self.cg), CGPathDrawingMode::Stroke);
        CGContext::restore_g_state(Some(self.cg));
    }
    // Image rows are top-down, while Quartz draws images in an upward y axis.
    fn draw_image(&self, img: &CGImage, dst: CGRect) {
        CGContext::save_g_state(Some(self.cg));
        CGContext::translate_ctm(Some(self.cg), dst.origin.x, dst.origin.y + dst.size.height);
        CGContext::scale_ctm(Some(self.cg), 1.0, -1.0);
        CGContext::set_interpolation_quality(Some(self.cg), CGInterpolationQuality::None);
        CGContext::draw_image(Some(self.cg), CGRect::new(CGPoint::new(0.0, 0.0), dst.size), Some(img));
        CGContext::restore_g_state(Some(self.cg));
    }
    pub fn stroke_cells(&self, p: &Path, w: f32, cells: &[u8], cols: u32, rows: u32, cell: f32, origin: (f32, f32)) {
        let Some(img) = image_from_pixels(cells, cols, rows) else {
            return;
        };
        CGContext::save_g_state(Some(self.cg));
        self.apply();
        self.stroke_setup(Color::default(), w, Stroke::Round);
        self.path(p);
        CGContext::replace_path_with_stroked_path(Some(self.cg));
        CGContext::clip(Some(self.cg));
        self.draw_image(
            &img,
            CGRect::new(CGPoint::new(origin.0 as f64, origin.1 as f64), CGSize::new((cols as f32 * cell) as f64, (rows as f32 * cell) as f64)),
        );
        CGContext::restore_g_state(Some(self.cg));
    }
    pub fn pixels(&self, px: &[u8], w: u32, h: u32, dst: RectF) {
        let Some(img) = image_from_pixels(px, w, h) else {
            return;
        };
        CGContext::save_g_state(Some(self.cg));
        self.apply();
        self.draw_image(&img, rect(dst));
        CGContext::restore_g_state(Some(self.cg));
    }
    pub fn blit(&self, src: &Bitmap, from: Rect, x: i32, y: i32) {
        let Some(img) = src.image() else { return };
        let Some(crop) = CGImage::with_image_in_rect(Some(&img), recti(from)) else {
            return;
        };
        CGContext::save_g_state(Some(self.cg));
        self.apply();
        self.draw_image(&crop, CGRect::new(CGPoint::new(x as f64, y as f64), CGSize::new((from.right - from.left) as f64, (from.bottom - from.top) as f64)));
        CGContext::restore_g_state(Some(self.cg));
    }
    pub fn blit_scaled(&self, src: &Bitmap, dst: Rect) {
        let Some(img) = src.image() else { return };
        CGContext::save_g_state(Some(self.cg));
        self.apply();
        self.draw_image(&img, recti(dst));
        CGContext::restore_g_state(Some(self.cg));
    }
    pub fn fill_px(&self, r: Rect, rgb: u32) {
        let c = Color { r: ((rgb >> 16) & 255) as f32 / 255.0, g: ((rgb >> 8) & 255) as f32 / 255.0, b: (rgb & 255) as f32 / 255.0, a: 1.0 };
        self.fill_rect(RectF { left: r.left as f32, top: r.top as f32, right: r.right as f32, bottom: r.bottom as f32 }, c);
    }
    pub fn push_clip(&self, r: RectF) {
        CGContext::save_g_state(Some(self.cg));
        let m = self.user.get();
        let transform = CGAffineTransform { a: m.M11 as f64, b: m.M12 as f64, c: m.M21 as f64, d: m.M22 as f64, tx: m.M31 as f64, ty: m.M32 as f64 };
        let path = CGMutablePath::new();
        unsafe {
            CGMutablePath::add_rect(Some(&path), &transform, rect(r));
        }
        CGContext::add_path(Some(self.cg), Some(&path));
        CGContext::clip(Some(self.cg));
        self.clips.set(self.clips.get() + 1);
    }
    pub fn pop_clip(&self) {
        assert!(self.clips.get() > 0);
        self.clips.set(self.clips.get() - 1);
        CGContext::restore_g_state(Some(self.cg));
    }
    pub fn text(&self, t: &Text, x: f32, y: f32, c: Color) {
        CGContext::save_g_state(Some(self.cg));
        self.apply();
        CGContext::set_rgb_fill_color(Some(self.cg), c.r as f64, c.g as f64, c.b as f64, c.a as f64);
        CGContext::set_text_matrix(Some(self.cg), CGAffineTransform { a: 1.0, b: 0.0, c: 0.0, d: -1.0, tx: 0.0, ty: 0.0 });
        for l in t.lines.borrow().iter() {
            CGContext::set_text_position(Some(self.cg), (x + l.left) as f64, (y + l.top + t.baseline) as f64);
            unsafe { l.line.draw(self.cg) };
        }
        CGContext::restore_g_state(Some(self.cg));
        for &(a, b) in t.underlines.borrow().iter() {
            for (left, top, width, height) in t.range_rects(a, b) {
                self.line((x + left, y + top + height - 1.0), (x + left + width, y + top + height - 1.0), c, 1.0, Stroke::Plain);
            }
        }
    }
}

fn rect(r: RectF) -> CGRect {
    CGRect::new(CGPoint::new(r.left as f64, r.top as f64), CGSize::new((r.right - r.left) as f64, (r.bottom - r.top) as f64))
}
fn recti(r: Rect) -> CGRect {
    rect(RectF { left: r.left as f32, top: r.top as f32, right: r.right as f32, bottom: r.bottom as f32 })
}
fn rounded_path(r: RectF, rad: f32) -> CFRetained<CGMutablePath> {
    let p = CGMutablePath::new();
    let rr = rect(r);
    unsafe {
        CGMutablePath::add_rounded_rect(Some(&p), std::ptr::null(), rr, rad as f64, rad as f64);
    }
    p
}
impl Drop for Canvas<'_> {
    fn drop(&mut self) {
        for _ in 0..=self.clips.get() {
            CGContext::restore_g_state(Some(self.cg));
        }
    }
}

fn image_from_pixels(px: &[u8], w: u32, h: u32) -> Option<CFRetained<CGImage>> {
    let len = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
    if w == 0 || h == 0 || px.len() != len {
        return None;
    }
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
    let provider = CGDataProvider::with_cf_data(Some(&CFData::from_bytes(px)))?;
    // The shared pixel API ignores alpha, as does the Windows bitmap brush.
    unsafe {
        CGImage::new(
            w as usize,
            h as usize,
            8,
            32,
            w as usize * 4,
            Some(&space),
            CGBitmapInfo(CGImageAlphaInfo::NoneSkipFirst.0 | (2 << 12)),
            Some(&provider),
            std::ptr::null(),
            false,
            objc2_core_graphics::CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
}

pub struct Path(pub(crate) CFRetained<CGMutablePath>);
pub struct PathBuilder {
    path: CFRetained<CGMutablePath>,
    open: bool,
    current: Option<(f32, f32)>,
}
impl PathBuilder {
    pub fn move_to(&mut self, x: f32, y: f32) {
        unsafe { CGMutablePath::move_to_point(Some(&self.path), std::ptr::null(), x as f64, y as f64) };
        self.open = true;
        self.current = Some((x, y));
    }
    pub fn line_to(&mut self, x: f32, y: f32) {
        unsafe { CGMutablePath::add_line_to_point(Some(&self.path), std::ptr::null(), x as f64, y as f64) };
        self.current = Some((x, y));
    }
    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        unsafe { CGMutablePath::add_quad_curve_to_point(Some(&self.path), std::ptr::null(), cx as f64, cy as f64, x as f64, y as f64) };
        self.current = Some((x, y));
    }
    pub fn cubic_to(&mut self, c1: (f32, f32), c2: (f32, f32), x: f32, y: f32) {
        unsafe {
            CGMutablePath::add_curve_to_point(Some(&self.path), std::ptr::null(), c1.0 as f64, c1.1 as f64, c2.0 as f64, c2.1 as f64, x as f64, y as f64)
        };
        self.current = Some((x, y));
    }
    pub fn arc_to(&mut self, rx: f32, ry: f32, rot: f32, large: bool, sweep: bool, x: f32, y: f32) {
        let Some((x0, y0)) = self.current else {
            self.line_to(x, y);
            return;
        };
        let (mut rx, mut ry) = (rx.abs() as f64, ry.abs() as f64);
        let (x1, y1) = (x as f64, y as f64);
        let (x0, y0) = (x0 as f64, y0 as f64);
        if rx < 1e-6 || ry < 1e-6 || ((x0 - x1).abs() < 1e-8 && (y0 - y1).abs() < 1e-8) {
            self.line_to(x, y);
            return;
        }
        let phi = (rot as f64).to_radians();
        let (cp, sp) = (phi.cos(), phi.sin());
        let (dx, dy) = ((x0 - x1) / 2.0, (y0 - y1) / 2.0);
        let (xp, yp) = (cp * dx + sp * dy, -sp * dx + cp * dy);
        let lam = xp * xp / (rx * rx) + yp * yp / (ry * ry);
        if lam > 1.0 {
            let q = lam.sqrt();
            rx *= q;
            ry *= q;
        }
        let den = (rx * rx * yp * yp + ry * ry * xp * xp).max(1e-18);
        let mut k = ((rx * rx * ry * ry - den) / den).max(0.0).sqrt();
        if large == sweep {
            k = -k;
        }
        let (cxp, cyp) = (k * rx * yp / ry, -k * ry * xp / rx);
        let (cx, cy) = (cp * cxp - sp * cyp + (x0 + x1) / 2.0, sp * cxp + cp * cyp + (y0 + y1) / 2.0);
        let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
            let dot = ux * vx + uy * vy;
            let cross = ux * vy - uy * vx;
            cross.atan2(dot)
        };
        let (ux, uy) = ((xp - cxp) / rx, (yp - cyp) / ry);
        let (vx, vy) = ((-xp - cxp) / rx, (-yp - cyp) / ry);
        let mut a0 = angle(1.0, 0.0, ux, uy);
        let mut da = angle(ux, uy, vx, vy);
        if !sweep && da > 0.0 {
            da -= std::f64::consts::TAU;
        } else if sweep && da < 0.0 {
            da += std::f64::consts::TAU;
        }
        let n = (da.abs() / (std::f64::consts::FRAC_PI_2)).ceil() as usize;
        let step = da / n.max(1) as f64;
        let point = |a: f64| (cx + rx * cp * a.cos() - ry * sp * a.sin(), cy + rx * sp * a.cos() + ry * cp * a.sin());
        for _ in 0..n {
            let a1 = a0 + step;
            let a = step;
            let t = 4.0 / 3.0 * (a / 4.0).tan();
            let p0 = point(a0);
            let p3 = point(a1);
            let c1 = (p0.0 - t * (rx * cp * a0.sin() + ry * sp * a0.cos()), p0.1 - t * (rx * sp * a0.sin() - ry * cp * a0.cos()));
            let c2 = (p3.0 + t * (rx * cp * a1.sin() + ry * sp * a1.cos()), p3.1 + t * (rx * sp * a1.sin() - ry * cp * a1.cos()));
            self.cubic_to((c1.0 as f32, c1.1 as f32), (c2.0 as f32, c2.1 as f32), p3.0 as f32, p3.1 as f32);
            a0 = a1;
        }
    }
    pub fn close(&mut self) {
        if self.open {
            CGMutablePath::close_subpath(Some(&self.path));
            self.open = false;
        }
    }
}

impl Path {
    pub fn new(_filled: bool, f: impl FnOnce(&mut PathBuilder) -> Option<()>) -> Option<Path> {
        let mut b = PathBuilder { path: CGMutablePath::new(), open: false, current: None };
        f(&mut b)?;
        Some(Path(b.path))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    Ui,
    Segoe,
    Cjk,
    Mono,
}
struct Line {
    line: CFRetained<CTLine>,
    start: u32,
    len: u32,
    width: f32,
    left: f32,
    top: f32,
}
pub struct Text {
    lines: RefCell<Vec<Line>>,
    len: u32,
    line_h: f32,
    baseline: f32,
    font: CFRetained<CTFont>,
    underlines: RefCell<Vec<(u32, u32)>>,
}
fn font(face: Face, px: f32, bold: bool) -> Option<CFRetained<CTFont>> {
    let f = unsafe {
        CTFont::new_ui_font_for_language(
            if face == Face::Mono { CTFontUIFontType::UserFixedPitch } else { CTFontUIFontType::System },
            px as f64,
            Some(&CFString::from_str("zh-Hans")),
        )
    }?;
    if bold {
        unsafe { f.copy_with_symbolic_traits(0.0, std::ptr::null(), CTFontSymbolicTraits::TraitBold, CTFontSymbolicTraits::TraitBold) }.or(Some(f))
    } else {
        Some(f)
    }
}
fn attributed(s: &str, f: &CTFont) -> Option<CFRetained<CFAttributedString>> {
    let dict = CFDictionary::<CFString, CFType>::from_slices(
        &[unsafe { kCTFontAttributeName }, unsafe { kCTForegroundColorFromContextAttributeName }],
        &[f.as_ref(), CFBoolean::new(true).as_ref()],
    );
    unsafe { CFAttributedString::new(None, Some(&CFString::from_str(s)), Some(dict.cast_unchecked())) }
}
fn line_width(l: &CTLine) -> f32 {
    unsafe { l.typographic_bounds(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) as f32 }
}
fn make(face: Face, px: f32, bold: bool, s: &str, width: f32, fixed: Option<f32>, digits: bool) -> Option<Text> {
    let mut f = font(face, px, bold)?;
    if digits {
        let feature = CFDictionary::<CFString, CFType>::from_slices(
            &[unsafe { kCTFontOpenTypeFeatureTag }, unsafe { kCTFontOpenTypeFeatureValue }],
            &[CFString::from_str("tnum").as_ref(), CFNumber::new_i32(1).as_ref()],
        );
        let features = CFArray::from_objects(&[&*feature]);
        let attrs = CFDictionary::<CFString, CFType>::from_slices(&[unsafe { kCTFontFeatureSettingsAttribute }], &[features.as_ref()]);
        let desc = unsafe { CTFontDescriptor::with_attributes(attrs.cast_unchecked()) };
        f = unsafe { f.copy_with_attributes(0.0, std::ptr::null(), Some(&desc)) };
    }
    let attr = attributed(s, &f)?;
    let ts = unsafe { CTTypesetter::with_attributed_string(&attr) };
    let units: Vec<_> = s.encode_utf16().collect();
    let total = units.len();
    let natural = unsafe { f.ascent() + f.descent() + f.leading() } as f32;
    let lh = fixed.unwrap_or(natural.ceil()).max(1.0);
    let baseline = fixed.map_or(unsafe { f.ascent() } as f32, |h| h * 0.76);
    let mut pos = 0;
    let mut lines = Vec::new();
    loop {
        let n = if pos < total { (unsafe { ts.suggest_line_break(pos as isize, width.max(1.0) as f64) }.max(1) as usize).min(total - pos) } else { 0 };
        let line = unsafe { ts.line(CFRange::new(pos as isize, n as isize)) };
        let w = line_width(&line);
        lines.push(Line { line, start: pos as u32, len: n as u32, width: w, left: 0.0, top: lines.len() as f32 * lh });
        pos += n;
        if pos == total {
            if n > 0 && matches!(units.last(), Some(10 | 13)) {
                continue;
            }
            break;
        }
    }
    Some(Text { lines: RefCell::new(lines), len: total as u32, line_h: lh, baseline, font: f, underlines: RefCell::new(Vec::new()) })
}
impl Text {
    pub fn new(face: Face, px: f32, bold: bool, s: &str) -> Option<Text> {
        make(face, px, bold, s, 100000.0, None, false)
    }
    pub fn tabular(face: Face, px: f32, bold: bool, s: &str) -> Option<Text> {
        make(face, px, bold, s, 100000.0, None, true)
    }
    pub fn wrapped(face: Face, px: f32, bold: bool, s: &str, w: f32, line_h: Option<f32>) -> Option<Text> {
        make(face, px, bold, s, w, line_h, false)
    }
    pub fn centered(face: Face, px: f32, bold: bool, s: &str, w: f32, h: f32) -> Option<Text> {
        let t = Self::new(face, px, bold, s)?;
        let offset = (h - t.height()) / 2.0;
        for l in t.lines.borrow_mut().iter_mut() {
            l.left = (w - l.width) / 2.0;
            l.top += offset;
        }
        Some(t)
    }
    pub fn truncate(&self, w: f32) {
        let Some(attr) = attributed("…", &self.font) else { return };
        let token = unsafe { CTLine::with_attributed_string(&attr) };
        for l in self.lines.borrow_mut().iter_mut() {
            if l.width > w.max(1.0)
                && let Some(line) = unsafe { l.line.truncated_line(w.max(1.0) as f64, CTLineTruncationType::End, Some(&token)) }
            {
                l.width = line_width(&line);
                l.line = line;
            }
        }
    }
    pub fn width(&self) -> f32 {
        self.lines.borrow().iter().map(|l| l.width).fold(0.0, f32::max)
    }
    pub fn height(&self) -> f32 {
        self.lines.borrow().len() as f32 * self.line_h
    }
    pub fn first_line_height(&self) -> f32 {
        self.line_h
    }
    pub fn hit(&self, x: f32, y: f32) -> u32 {
        let lines = self.lines.borrow();
        let i = ((y / self.line_h).floor().max(0.0) as usize).min(lines.len() - 1);
        let l = &lines[i];
        let p = unsafe { l.line.string_index_for_position(CGPoint::new((x - l.left) as f64, 0.0)) };
        if p < 0 { if x < l.left { l.start } else { l.start + l.len } } else { (p as u32).clamp(l.start, l.start + l.len) }
    }
    pub fn range_rects(&self, a: u32, b: u32) -> Vec<(f32, f32, f32, f32)> {
        self.lines
            .borrow()
            .iter()
            .filter_map(|l| {
                let (a, b) = (a.max(l.start), b.min(l.start + l.len));
                if a >= b {
                    return None;
                }
                let x0 = unsafe { l.line.offset_for_string_index(a as isize, std::ptr::null_mut()) } as f32;
                let x1 = unsafe { l.line.offset_for_string_index(b as isize, std::ptr::null_mut()) } as f32;
                Some((l.left + x0.min(x1), l.top, (x1 - x0).abs().max(1.0), self.line_h))
            })
            .collect()
    }
    pub fn caret(&self, pos: u32) -> (f32, f32, f32) {
        let lines = self.lines.borrow();
        let pos = pos.min(self.len);
        let l = lines.iter().rev().find(|l| pos >= l.start).unwrap();
        let x = unsafe { l.line.offset_for_string_index(pos as isize, std::ptr::null_mut()) } as f32;
        (l.left + x, l.top, self.line_h)
    }
    pub fn underline(&self, start: u32, len: u32) {
        self.underlines.borrow_mut().push((start, start.saturating_add(len)));
    }
}

pub struct Bitmap {
    pub(crate) w: i32,
    pub(crate) h: i32,
    pub(crate) ctx: CFRetained<CGContext>,
}
impl Bitmap {
    pub fn new(w: i32, h: i32) -> Option<Bitmap> {
        if w <= 0 || h <= 0 {
            return None;
        }
        let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
        let info = CGBitmapInfo(CGImageAlphaInfo::PremultipliedFirst.0 | (2 << 12));
        let ctx = unsafe { CGBitmapContextCreate(std::ptr::null_mut(), w as usize, h as usize, 8, w as usize * 4, Some(&space), info.0) }?;
        CGContext::set_rgb_fill_color(Some(&ctx), 0.0, 0.0, 0.0, 1.0);
        CGContext::fill_rect(Some(&ctx), CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64)));
        Some(Bitmap { w, h, ctx })
    }
    pub fn pixels(&self) -> &[u8] {
        let p = objc2_core_graphics::CGBitmapContextGetData(Some(&self.ctx));
        unsafe { std::slice::from_raw_parts(p as *const u8, self.w as usize * self.h as usize * 4) }
    }
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        let p = objc2_core_graphics::CGBitmapContextGetData(Some(&self.ctx));
        unsafe { std::slice::from_raw_parts_mut(p as *mut u8, self.w as usize * self.h as usize * 4) }
    }
    pub(crate) fn image(&self) -> Option<CFRetained<CGImage>> {
        objc2_core_graphics::CGBitmapContextCreateImage(Some(&self.ctx))
    }
}

pub fn warm_up() {
    let Some(b) = Bitmap::new(16, 16) else { return };
    if let Some(t) = Text::tabular(Face::Ui, 12.0, false, "0 × 9") {
        Canvas::new(&b, b.full(), false).text(&t, 0.0, 0.0, Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{rectf, rgb};
    #[test]
    fn pixels_blit_and_clips_keep_top_down_coordinates() {
        let mut src = Bitmap::new(8, 8).unwrap();
        for (i, p) in src.pixels_mut().chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(&[i as u8, 20, 200, 255]);
        }
        let dst = Bitmap::new(8, 8).unwrap();
        {
            let c = Canvas::new(&dst, dst.full(), false);
            c.blit(&src, src.full(), 0, 0);
        }
        assert_eq!(src.pixels(), dst.pixels());
        let part = Rect { left: 2, top: 3, right: 6, bottom: 7 };
        {
            let c = Canvas::new(&dst, part, false);
            c.set_transform(Affine { M11: 2.0, M22: 2.0, ..Affine::default() });
            c.push_clip(rectf(1.0, 2.0, 2.0, 3.0));
            c.fill_rect(rectf(0.0, 0.0, 8.0, 8.0), rgb(0xFFFFFF));
            c.pop_clip();
        }
        for y in 0..8 {
            for x in 0..8 {
                let i = (y * 8 + x) * 4;
                let expected = if (2..4).contains(&x) && (4..6).contains(&y) { &[255, 255, 255, 255][..] } else { &src.pixels()[i..i + 4] };
                assert_eq!(&dst.pixels()[i..i + 4], expected, "{x},{y}");
            }
        }
        let copy = Bitmap::new(8, 8).unwrap();
        {
            let c = Canvas::new(&copy, copy.full(), false);
            c.pixels(src.pixels(), 8, 8, rectf(0.0, 0.0, 8.0, 8.0));
        }
        assert_eq!(src.pixels(), copy.pixels());
    }
    #[test]
    fn arc_reaches_endpoint_across_multiple_segments() {
        let p = Path::new(false, |p| {
            p.move_to(10.0, 0.0);
            p.arc_to(10.0, 10.0, 0.0, true, true, 0.0, -10.0);
            Some(())
        })
        .unwrap();
        let end = objc2_core_graphics::CGPath::current_point(Some(&p.0));
        assert!(end.x.abs() < 0.001 && (end.y + 10.0).abs() < 0.001, "{end:?}");
        let bounds = objc2_core_graphics::CGPath::path_bounding_box(Some(&p.0));
        assert!((bounds.size.width - 20.0).abs() < 0.01 && (bounds.size.height - 20.0).abs() < 0.01, "{bounds:?}");
    }
    #[test]
    fn text_preserves_blank_lines_and_utf16_positions() {
        let t = Text::wrapped(Face::Ui, 18.0, false, "one\n\n中😀A", 200.0, Some(24.0)).unwrap();
        assert_eq!(t.height(), 72.0);
        assert_eq!(t.caret(5).1, 48.0);
        assert_eq!(t.hit(0.0, 49.0), 5);
        assert_eq!(t.hit(1000.0, 49.0), 9);
        let newline = Text::new(Face::Ui, 18.0, false, "a\n").unwrap();
        assert_eq!(newline.height(), newline.first_line_height() * 2.0);
        let long = Text::new(Face::Ui, 18.0, false, "long text with a real ellipsis").unwrap();
        long.truncate(80.0);
        assert!(long.width() <= 80.01);
        let image = Bitmap::new(240, 96).unwrap();
        {
            let c = Canvas::new(&image, image.full(), false);
            c.text(&t, 0.0, 0.0, rgb(0xFFFFFF));
        }
        let row = |y: usize| image.pixels()[y * 240 * 4..(y + 1) * 240 * 4].chunks_exact(4).any(|p| p[0] > 128);
        assert!((0..24).any(row));
        assert!(!(24..48).any(row));
        assert!((48..72).any(row));
    }
}
