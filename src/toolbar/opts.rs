//! 工具选项条：三档粗细、六色色板（马赛克没有），选中已画的标注时多一个删除。
//! 高 36，按钮 28 × 28，样式和操作栏一样；位置由宿主按操作栏定。

use crate::annotate::{COLOR_NAMES, COLORS, Kind};
use crate::plat::{Canvas, Color, Face, Point as POINT, Rect as RECT, Stroke, Text};
use crate::ui::{self, rectf, rgb, rgba};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    Size(usize),
    Color(usize),
    Delete,
}

const TEXT: u32 = 0x1A1A1A;
const ACCENT: u32 = 0x9C44EC;
const ACCENT_SOFT: u32 = 0xF3E9FD;

pub struct Opts {
    pub rect: RECT,
    items: Vec<(Item, RECT)>,
    seps: Vec<i32>,
    kind: Kind,
    scale: f32,
}

/// 按 DIP 从左往右排：外框 1 + 内边距 3，按钮 28，间距 2，分组线左右各再留 4。返回 (按钮和 x, 分组线 x, 总宽)
fn arrange(kind: Kind, trash: bool) -> (Vec<(Item, f32)>, Vec<f32>, f32) {
    let mut groups: Vec<Vec<Item>> = vec![(0..3).map(Item::Size).collect()];
    if kind != Kind::Mosaic {
        groups.push((0..COLORS.len()).map(Item::Color).collect());
    }
    if trash {
        groups.push(vec![Item::Delete]);
    }
    let (mut x, mut items, mut seps) = (4.0, Vec::new(), Vec::new());
    for (g, group) in groups.iter().enumerate() {
        if g > 0 {
            seps.push(x + 4.0);
            x += 4.0 + 1.0 + 4.0 + 2.0;
        }
        for it in group {
            items.push((*it, x));
            x += 28.0 + 2.0;
        }
    }
    (items, seps, x - 2.0 + 4.0)
}

/// 提示文字和快捷键
pub fn tip(kind: Kind, it: Item) -> (&'static str, &'static str) {
    match it {
        Item::Size(i) => match kind {
            Kind::Text => (["小", "中", "大"][i], ""),
            Kind::Mosaic => (["细笔刷", "中笔刷", "粗笔刷"][i], ""),
            Kind::Line => (["细", "中", "粗"][i], ""),
        },
        Item::Color(i) => (COLOR_NAMES[i], ""),
        Item::Delete => ("删除", "Delete"),
    }
}

impl Opts {
    pub fn size(kind: Kind, trash: bool, scale: f32) -> (i32, i32) {
        ((arrange(kind, trash).2 * scale).round() as i32, (36.0 * scale).round() as i32)
    }

    pub fn layout(kind: Kind, trash: bool, left: i32, top: i32, scale: f32) -> Opts {
        let px = |dip: f32| (dip * scale).round() as i32;
        let (items, seps, w) = arrange(kind, trash);
        let (bt, bb) = (top + px(4.0), top + px(32.0));
        Opts {
            rect: RECT { left, top, right: left + px(w), bottom: top + px(36.0) },
            items: items.iter().map(|(it, x)| (*it, RECT { left: left + px(*x), top: bt, right: left + px(x + 28.0), bottom: bb })).collect(),
            seps: seps.iter().map(|x| left + px(*x)).collect(),
            kind,
            scale,
        }
    }

    pub fn hit(&self, p: POINT) -> Option<Item> {
        let inside = |r: &RECT| p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom;
        self.items.iter().find(|(_, r)| inside(r)).map(|(it, _)| *it)
    }

    pub fn button(&self, it: Item) -> RECT {
        self.items.iter().find(|(i, _)| *i == it).map_or(self.rect, |(_, r)| *r)
    }

    /// `pen`：当前（或选中标注的）颜色和粗细；`hot`、`down`：鼠标下、按住的按钮
    pub fn draw(&self, c: &Canvas, pen: (usize, usize), hot: Option<Item>, down: Option<Item>) {
        let s = self.scale;
        let f = |r: &RECT| (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
        let lw = s.round().max(1.0);
        let (l, t, r, b) = f(&self.rect);
        let h = lw / 2.0;
        let frame = rectf(l + h, t + h, r - h, b - h);
        c.fill_round(frame, 8.0 * s, rgb(0xFAFAFA));
        c.stroke_round(frame, 8.0 * s, rgb(0xD4D4D4), lw);
        for x in &self.seps {
            let mid = (t + b) / 2.0;
            c.fill_rect(rectf(*x as f32, mid - 10.0 * s, *x as f32 + lw, mid + 10.0 * s), rgb(0xD4D4D4));
        }
        for (it, r) in &self.items {
            let (l, t, rr, bb) = f(r);
            let (cx, cy) = ((l + rr) / 2.0, (t + bb) / 2.0);
            let on = matches!(*it, Item::Size(i) if i == pen.1);
            let bg = if on {
                Some(ACCENT_SOFT)
            } else {
                match (hot == Some(*it), down == Some(*it)) {
                    (true, true) => Some(0xE0E0E0),
                    (true, false) => Some(0xEBEBEB),
                    _ => None,
                }
            };
            if let Some(bg) = bg {
                c.fill_round(rectf(l, t, rr, bb), 4.0 * s, rgb(bg));
            }
            let fg = rgb(if on { ACCENT } else { TEXT });
            match *it {
                Item::Size(i) => match self.kind {
                    Kind::Line => {
                        let r = [4.0, 7.0, 10.0][i] * s / 2.0;
                        c.fill_ellipse(cx, cy, r, r, fg);
                    }
                    Kind::Text => {
                        if let Some(a) = Text::new(Face::Ui, [11.0, 14.0, 17.0][i] * s, true, "A") {
                            c.text(&a, cx - a.width() / 2.0, cy - a.height() / 2.0, fg);
                        }
                    }
                    Kind::Mosaic => mosaic_dot(c, cx, cy, [9.0, 13.0, 17.0][i] * s / 2.0, s, fg),
                },
                Item::Color(i) => {
                    // 18 的色块 + 18% 黑的内圈细边；选中时隔 2 的空隙再套 1.5 的主题色圆环
                    c.fill_ellipse(cx, cy, 9.0 * s, 9.0 * s, rgb(COLORS[i]));
                    c.stroke_ellipse(cx, cy, 9.0 * s - 0.5 * lw, 9.0 * s - 0.5 * lw, rgba(0, 0.18), lw, Stroke::Plain);
                    if pen.0 == i {
                        c.stroke_ellipse(cx, cy, 11.75 * s, 11.75 * s, rgb(ACCENT), 1.5 * s, Stroke::Plain);
                    }
                }
                Item::Delete => ui::icon(c, "trash", cx - 10.0 * s, cy - 10.0 * s, 20.0 * s, fg),
            }
        }
    }
}

/// 马赛克笔刷的图标：1.5 描边的圆，里面是 2 × 2 的棋盘格
fn mosaic_dot(c: &Canvas, cx: f32, cy: f32, r: f32, s: f32, color: Color) {
    let cell = 2.0 * s;
    let n = (r / cell).ceil() as i32;
    for j in -n..n {
        for i in -n..n {
            let (x, y) = (cx + i as f32 * cell, cy + j as f32 * cell);
            // 格子中心在圆里才画
            if (i + j).rem_euclid(2) == 0 && (x + cell / 2.0 - cx).hypot(y + cell / 2.0 - cy) < r - 0.75 * s {
                c.fill_rect(rectf(x, y, x + cell, y + cell), color);
            }
        }
    }
    c.stroke_ellipse(cx, cy, r - 0.75 * s, r - 0.75 * s, color, 1.5 * s, Stroke::Plain);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths() {
        // 3 档 + 分组线 + 6 色：4 + 3×30 + 11 + 6×30 - 2 + 4
        assert_eq!(arrange(Kind::Line, false).2, 287.0);
        // 马赛克：只有 3 档；选中时再加分组线和删除
        assert_eq!(arrange(Kind::Mosaic, false).2, 96.0);
        assert_eq!(arrange(Kind::Mosaic, true).2, 96.0 + 11.0 + 30.0);
        let o = Opts::layout(Kind::Line, true, 100, 50, 1.5);
        assert_eq!(o.button(Item::Size(0)), RECT { left: 106, top: 56, right: 148, bottom: 98 });
        assert_eq!(o.hit(POINT { x: 107, y: 60 }), Some(Item::Size(0)));
        assert_eq!(o.rect.bottom - o.rect.top, 54);
    }

    /// 操作栏（当前工具矩形、撤销可用、重做置灰）和三种选项条，存成 target/test-tmp/annotate/bars.png。平时不跑：
    /// cargo test bar_samples -- --ignored
    #[test]
    #[ignore]
    fn bar_samples() {
        use crate::toolbar::{Act, Bar, SHOT};
        use crate::plat::Bitmap;
        let s = 1.5;
        let bmp = Bitmap::new(1100, 330).unwrap();
        let label = Text::new(Face::Ui, 13.0 * s, true, "复制").unwrap();
        let bar = Bar::layout(RECT { left: 0, top: 0, right: 1090, bottom: 0 }, 1100, 330, s, label.width(), 0, &SHOT);
        {
            let c = Canvas::new(&bmp, bmp.full(), false);
            c.clear(rgb(0x707070));
            bar.draw(&c, &label, Some(Act::Pin), None, Some(Act::Rect), |a| a != Act::Redo);
            let mut y = bar.rect.bottom + 10;
            for (kind, trash, pen) in [(Kind::Line, false, (0, 1)), (Kind::Text, true, (3, 2)), (Kind::Mosaic, true, (0, 0))] {
                let o = Opts::layout(kind, trash, 10, y, s);
                o.draw(&c, pen, Some(Item::Color(2)), None);
                y = o.rect.bottom + 10;
            }
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("annotate");
        std::fs::create_dir_all(&dir).unwrap();
        let png = crate::plat::encode(&bmp.crop(&bmp.full()), false).unwrap();
        std::fs::write(dir.join("bars.png"), png).unwrap();
    }
}
