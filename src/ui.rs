//! 自绘界面的公用件：颜色、深色浮层（尺寸读数、工具提示）、自绘图标、加载转圈。只用 plat 的绘制接口。

use crate::plat::{Affine, Canvas, Color, Face, Path, PathBuilder, Rect, RectF, Stroke, Text};
use std::cell::RefCell;

/// 0xRRGGBB → 颜色
pub fn rgb(c: u32) -> Color {
    rgba(c, 1.0)
}

pub fn rgba(c: u32, a: f32) -> Color {
    let ch = |s: u32| ((c >> s) & 0xFF) as f32 / 255.0;
    Color { r: ch(16), g: ch(8), b: ch(0), a }
}

pub fn rectf(l: f32, t: f32, r: f32, b: f32) -> RectF {
    RectF { left: l, top: t, right: r, bottom: b }
}

/// r 里面贴边的一圈 1 像素框（整像素，不抗锯齿）
pub fn frame_px(c: &Canvas, r: Rect, rgb: u32) {
    let (l, t, rr, b) = (r.left, r.top, r.right, r.bottom);
    for e in [
        Rect { left: l, top: t, right: rr, bottom: t + 1 },
        Rect { left: l, top: b - 1, right: rr, bottom: b },
        Rect { left: l, top: t, right: l + 1, bottom: b },
        Rect { left: rr - 1, top: t, right: rr, bottom: b },
    ] {
        c.fill_px(e, rgb);
    }
}

/// 深色浮层（尺寸读数、工具提示、钉图读数）：高 22，内边距 3 × 7，圆角 4
pub struct Hud {
    pub w: i32,
    pub h: i32,
    parts: Vec<(Text, f32, u32)>,
    scale: f32,
}

pub const HUD_TEXT2: u32 = 0xB4B4B8;

impl Hud {
    /// `parts`：(文字, 颜色)，依次排开，段间距 8；12 号界面字，数字等宽
    pub fn new(parts: &[(&str, u32)], scale: f32) -> Option<Hud> {
        let mut x = 7.0 * scale;
        let mut v = Vec::new();
        for (i, (t, c)) in parts.iter().enumerate() {
            if i > 0 {
                x += 8.0 * scale;
            }
            let l = Text::tabular(Face::Ui, 12.0 * scale, false, t)?;
            let w = l.width();
            v.push((l, x, *c));
            x += w;
        }
        Some(Hud { w: (x + 7.0 * scale).ceil() as i32, h: (22.0 * scale).round() as i32, parts: v, scale })
    }

    pub fn draw(&self, c: &Canvas, x: i32, y: i32) {
        let (x, y, s) = (x as f32, y as f32, self.scale);
        c.fill_round(rectf(x, y, x + self.w as f32, y + self.h as f32), 4.0 * s, rgba(0x1C1C1E, 0.92));
        for (l, dx, col) in &self.parts {
            // 行高 16 在 22 里居中
            c.text(l, x + dx, y + 3.0 * s, rgb(*col));
        }
    }
}

// ---- 自绘图标：20 × 20 网格，1.5 描边，圆头圆角 ----

/// (名字, [(SVG 路径, 是否填充)])；矩形、圆、椭圆已经换成路径。只用 M L H V C A Z 及其小写形式。
const ICONS: &[(&str, &[(&str, bool)])] = &[
    ("rect", &[("M5 5H15A1.5 1.5 0 0 1 16.5 6.5V13.5A1.5 1.5 0 0 1 15 15H5A1.5 1.5 0 0 1 3.5 13.5V6.5A1.5 1.5 0 0 1 5 5Z", false)]),
    ("ellipse", &[("M3 10A7 5.5 0 1 0 17 10A7 5.5 0 1 0 3 10Z", false)]),
    ("arrow", &[("M4.5 15.5 15 5M8.75 4.75h6.5v6.5", false)]),
    ("pen", &[("M3.5 13.5C5 8.5 7.4 7 8.6 9.6c1.1 2.4.4 5 2.6 4.6 2-.4 3.4-4.6 5.3-8.7", false)]),
    ("text", &[("M4.5 5h11M10 5v11M7.5 16h5", false)]),
    ("num", &[("M3 10A7 7 0 1 0 17 10A7 7 0 1 0 3 10Z", false), ("M8.5 7.7 10.3 6.5v7", false)]),
    (
        "mosaic",
        &[
            ("M4.5 3.5H15.5A1 1 0 0 1 16.5 4.5V15.5A1 1 0 0 1 15.5 16.5H4.5A1 1 0 0 1 3.5 15.5V4.5A1 1 0 0 1 4.5 3.5Z", false),
            ("M3.5 3.5h4.33v4.33H3.5zM12.17 3.5h4.33v4.33h-4.33zM7.83 7.83h4.34v4.34H7.83zM3.5 12.17h4.33v4.33H3.5zM12.17 12.17h4.33v4.33h-4.33z", true),
        ],
    ),
    ("undo", &[("M7.5 5 4.5 8l3 3", false), ("M4.5 8h7.25a3.75 3.75 0 0 1 0 7.5H8.5", false)]),
    ("redo", &[("M12.5 5l3 3-3 3", false), ("M15.5 8H8.25a3.75 3.75 0 0 0 0 7.5h3.25", false)]),
    ("pin", &[("M7.5 3.5h5M8.5 3.5v4.2L6 10.5V12h8v-1.5l-2.5-2.8V3.5M10 12v4.5", false)]),
    ("ocr", &[("M3.5 7V5c0-.8.7-1.5 1.5-1.5h2M13 3.5h2c.8 0 1.5.7 1.5 1.5v2M16.5 13v2c0 .8-.7 1.5-1.5 1.5h-2M7 16.5H5c-.8 0-1.5-.7-1.5-1.5v-2M7 8h6M7 10.5h6M7 13h3.5", false)]),
    ("translate", &[("M7 3.5V5M3.5 5.5h7M5 7.5l5 5.5M9.5 7.5 4 13M11 17l3-7.5 3 7.5M12.1 14.4h3.8", false)]),
    // 录屏：圆环里一个实心圆点；停止：实心圆角方块
    ("record", &[("M3 10A7 7 0 1 0 17 10A7 7 0 1 0 3 10Z", false), ("M7 10A3 3 0 1 0 13 10A3 3 0 1 0 7 10Z", true)]),
    // 秒表：录屏选区上的「延时开始」
    ("timer", &[("M4 11A6 6 0 1 0 16 11A6 6 0 1 0 4 11Z", false), ("M8 2.5h4M10 2.5V5M10 11V7.8M14.8 5.2l1.2-1.2", false)]),
    ("stop", &[("M7 5.5h6A1.5 1.5 0 0 1 14.5 7v6A1.5 1.5 0 0 1 13 14.5H7A1.5 1.5 0 0 1 5.5 13V7A1.5 1.5 0 0 1 7 5.5Z", true)]),
    ("save", &[("M10 3.5v9M6.5 9 10 12.5 13.5 9M4 13.5V15c0 .8.7 1.5 1.5 1.5h9c.8 0 1.5-.7 1.5-1.5v-1.5", false)]),
    ("cancel", &[("M5.5 5.5l9 9M14.5 5.5l-9 9", false)]),
    (
        "copy",
        &[
            ("M8.5 7H15A1.5 1.5 0 0 1 16.5 8.5V15A1.5 1.5 0 0 1 15 16.5H8.5A1.5 1.5 0 0 1 7 15V8.5A1.5 1.5 0 0 1 8.5 7Z", false),
            ("M13 7V5c0-.8-.7-1.5-1.5-1.5H5c-.8 0-1.5.7-1.5 1.5v6.5c0 .8.7 1.5 1.5 1.5h2", false),
        ],
    ),
    ("close", &[("M6 6l8 8M14 6l-8 8", false)]),
    ("trash", &[("M3.5 5.5h13M8 5.5V4c0-.3.2-.5.5-.5h3c.3 0 .5.2.5.5v1.5M5.5 5.5l.7 10c0 .5.5.9 1 .9h5.6c.5 0 1-.4 1-.9l.7-10", false)]),
    ("check", &[("M4.5 10.5 8 14l7.5-8", false)]),
    ("warn", &[("M10 3.5 17 16H3z", false), ("M10 8.2v3.6M10 13.9v.1", false)]),
    ("chev", &[("M8 5.5 12.5 10 8 14.5", false)]),
    // chev 顺时针转 90°：下拉按钮用
    ("chev_down", &[("M14.5 8 10 12.5 5.5 8", false)]),
    // 胶囊的状态图标里的白色记号，画在 16 × 16 网格上
    ("ok_mark", &[("M4.7 8.2 7 10.5l4.3-4.8", false)]),
    ("err_mark", &[("M5.5 5.5l5 5M10.5 5.5l-5 5", false)]),
];

/// SVG 路径里的命令字母和数字，依次取
struct PathTokens<'a>(&'a [u8]);

impl PathTokens<'_> {
    fn skip_sep(&mut self) {
        while let [b' ' | b',' | b'\t' | b'\n', rest @ ..] = self.0 {
            self.0 = rest;
        }
    }

    fn command(&mut self) -> Option<u8> {
        self.skip_sep();
        match self.0 {
            [c, rest @ ..] if c.is_ascii_alphabetic() => {
                self.0 = rest;
                Some(*c)
            }
            _ => None,
        }
    }

    /// 数字：可带符号、小数点；".5.5" 是两个数
    fn number(&mut self) -> Option<f32> {
        self.skip_sep();
        let s = self.0;
        let mut i = usize::from(matches!(s.first(), Some(b'-' | b'+')));
        let mut dot = false;
        while let Some(&c) = s.get(i) {
            if c.is_ascii_digit() || (c == b'.' && !dot) {
                dot |= c == b'.';
                i += 1;
            } else {
                break;
            }
        }
        let v = std::str::from_utf8(&s[..i]).ok()?.parse().ok()?;
        self.0 = &s[i..];
        Some(v)
    }

    fn at_end(&mut self) -> bool {
        self.skip_sep();
        self.0.is_empty()
    }
}

/// SVG 路径 → 几何路径；格式不对时返回 None（只会是图标表写错，单测会抓到）
pub fn svg_path(d: &str, filled: bool) -> Option<Path> {
    Path::new(filled, |b: &mut PathBuilder| {
        let mut t = PathTokens(d.as_bytes());
        let (mut cur, mut start) = ((0.0, 0.0), (0.0, 0.0));
        let mut cmd = b'M';
        while !t.at_end() {
            if let Some(c) = t.command() {
                cmd = c;
            }
            let rel = cmd.is_ascii_lowercase();
            let base = if rel { cur } else { (0.0, 0.0) };
            let pt = |t: &mut PathTokens| Some((base.0 + t.number()?, base.1 + t.number()?));
            match cmd.to_ascii_uppercase() {
                b'M' => {
                    let p = pt(&mut t)?;
                    b.move_to(p.0, p.1);
                    (cur, start) = (p, p);
                    // M 后面接着的坐标按 L 处理
                    cmd = if rel { b'l' } else { b'L' };
                }
                b'L' => {
                    cur = pt(&mut t)?;
                    b.line_to(cur.0, cur.1);
                }
                b'H' => {
                    cur = (t.number()? + base.0, cur.1);
                    b.line_to(cur.0, cur.1);
                }
                b'V' => {
                    cur = (cur.0, t.number()? + base.1);
                    b.line_to(cur.0, cur.1);
                }
                b'C' => {
                    let (p1, p2, p3) = (pt(&mut t)?, pt(&mut t)?, pt(&mut t)?);
                    b.cubic_to(p1, p2, p3.0, p3.1);
                    cur = p3;
                }
                b'A' => {
                    let (rx, ry, rot) = (t.number()?, t.number()?, t.number()?);
                    let (large, sweep) = (t.number()? != 0.0, t.number()? != 0.0);
                    cur = pt(&mut t)?;
                    // SVG 的 sweep=1 在 y 向下的坐标里就是顺时针
                    b.arc_to(rx, ry, rot, large, sweep, cur.0, cur.1);
                }
                b'Z' => {
                    b.close();
                    cur = start;
                }
                _ => return None,
            }
        }
        Some(())
    })
}

type IconPaths = Vec<(Path, bool)>;

thread_local! {
    static ICON_CACHE: RefCell<Vec<(&'static str, IconPaths)>> = const { RefCell::new(Vec::new()) };
    /// 转圈的那四分之一圆弧（16 × 16 网格）
    static ARC: Option<Path> = svg_path("M8 1A7 7 0 0 1 15 8", false);
}

/// 在 (x, y) 画边长 size 的图标
pub fn icon(c: &Canvas, name: &str, x: f32, y: f32, size: f32, color: Color) {
    ICON_CACHE.with_borrow_mut(|cache| {
        if !cache.iter().any(|(n, _)| *n == name) {
            let Some((n, parts)) = ICONS.iter().find(|(n, _)| *n == name) else { return };
            let paths = parts.iter().filter_map(|(d, f)| Some((svg_path(d, *f)?, *f))).collect();
            cache.push((n, paths));
        }
        let Some((_, paths)) = cache.iter().find(|(n, _)| *n == name) else { return };
        let base = c.transform();
        let k = size / 20.0;
        c.set_transform(Affine { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: x, M32: y } * base);
        for (p, filled) in paths {
            if *filled {
                c.fill_path(p, color);
            } else {
                c.stroke_path(p, color, 1.5, Stroke::Round);
            }
        }
        c.set_transform(base);
    });
}

/// 加载转圈：16 的圆环，2 宽，(x, y) 是左上角；`angle` 是转过的角度，0.8 秒一圈由调用方的定时器推进
pub fn spinner(c: &Canvas, x: f32, y: f32, s: f32, angle: f32, track: Color, head: Color) {
    c.stroke_ellipse(x + 8.0 * s, y + 8.0 * s, 7.0 * s, 7.0 * s, track, 2.0 * s, Stroke::Plain);
    ARC.with(|arc| {
        let Some(arc) = arc else { return };
        let base = c.transform();
        let (sin, cos) = angle.to_radians().sin_cos();
        // 绕 (8, 8) 转，再缩放到像素、移到图标位置
        let rot = Affine { M11: cos, M12: sin, M21: -sin, M22: cos, M31: 8.0 - 8.0 * cos + 8.0 * sin, M32: 8.0 - 8.0 * sin - 8.0 * cos };
        let place = Affine { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: x, M32: y };
        c.set_transform(rot * place * base);
        c.stroke_path(arc, head, 2.0, Stroke::Plain);
        c.set_transform(base);
    });
}

#[cfg(test)]
mod tests {
    use super::PathTokens;

    #[test]
    fn path_numbers() {
        let mut t = PathTokens(b"M3.5 7V5c0-.8.7-1.5 1.5-1.5h2");
        assert_eq!(t.command(), Some(b'M'));
        assert_eq!((t.number(), t.number()), (Some(3.5), Some(7.0)));
        assert_eq!((t.command(), t.number()), (Some(b'V'), Some(5.0)));
        assert_eq!(t.command(), Some(b'c'));
        let v: Vec<f32> = std::iter::from_fn(|| t.number()).collect();
        assert_eq!(v, [0.0, -0.8, 0.7, -1.5, 1.5, -1.5]);
        assert_eq!((t.command(), t.number(), t.at_end()), (Some(b'h'), Some(2.0), true));
    }

    #[test]
    fn all_icons_parse() {
        for (name, parts) in super::ICONS {
            for (d, f) in *parts {
                assert!(super::svg_path(d, *f).is_some(), "{name}: {d}");
            }
        }
    }
}
