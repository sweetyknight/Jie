//! 工具选项条：三档粗细、六色色板（马赛克没有），选中已画的标注时多一个删除。
//! 样式和操作栏一样（白底、墨色描边、实心阴影），按钮 30 × 30；跟着当前工具，小尖角指着它。位置由操作栏定（`Bar::opts`）。

use crate::annotate::{COLOR_NAMES, COLORS, Kind};
use crate::plat::{Canvas, Color, Face, Path, Point as POINT, Rect as RECT, Stroke, Text};
use crate::ui::{self, rectf, rgb};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    Size(usize),
    Color(usize),
    Delete,
}

// 尺寸（DIP）：描边 2 + 内边距 5，按钮 30，间距 3；分组线 2、左右各 5；本体高 44；上面留 8 给小尖角，右下实心阴影 4
const INSET: f32 = 7.0;
const BTN: f32 = 30.0;
const GAP: f32 = 3.0;
const SEP_M: f32 = 5.0;
const BODY_H: f32 = 44.0;
const NOTCH: f32 = 8.0;
const SHADOW: f32 = 4.0;

pub struct Opts {
    /// 连尖角、阴影一起占的地方
    pub rect: RECT,
    body: RECT,
    items: Vec<(Item, RECT)>,
    seps: Vec<i32>,
    kind: Kind,
    scale: f32,
    /// 小尖角朝上指着的 x
    notch: Option<i32>,
}

/// 按 DIP 从左往右排。返回 (按钮和 x, 分组线 x, 本体宽)
fn arrange(kind: Kind, trash: bool) -> (Vec<(Item, f32)>, Vec<f32>, f32) {
    let mut groups: Vec<Vec<Item>> = vec![(0..3).map(Item::Size).collect()];
    if kind != Kind::Mosaic {
        groups.push((0..COLORS.len()).map(Item::Color).collect());
    }
    if trash {
        groups.push(vec![Item::Delete]);
    }
    let (mut x, mut items, mut seps) = (INSET, Vec::new(), Vec::new());
    for (g, group) in groups.iter().enumerate() {
        if g > 0 {
            seps.push(x + SEP_M - GAP);
            x += SEP_M - GAP + 2.0 + SEP_M;
        }
        for it in group {
            items.push((*it, x));
            x += BTN + GAP;
        }
    }
    (items, seps, x - GAP + INSET)
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
    /// 占的地方（连尖角、阴影），像素
    pub fn size(kind: Kind, trash: bool, scale: f32) -> (i32, i32) {
        let px = |dip: f32| (dip * scale).round() as i32;
        (px(arrange(kind, trash).2) + px(SHADOW), px(NOTCH + BODY_H) + px(SHADOW))
    }

    /// (left, top) 是占的地方的左上角。`up`：在操作栏下面，尖角那 8 留在上边；否则留在下边（不画尖角）。
    /// `notch`：尖角指着的 x（当前工具按钮的中线），没有当前工具时不画
    pub fn layout(kind: Kind, trash: bool, left: i32, top: i32, scale: f32, up: bool, notch: Option<i32>) -> Opts {
        let px = |dip: f32| (dip * scale).round() as i32;
        let (items, seps, w) = arrange(kind, trash);
        let bt = if up { top + px(NOTCH) } else { top };
        let body = RECT { left, top: bt, right: left + px(w), bottom: bt + px(BODY_H) };
        let (it, ib) = (bt + px(INSET), bt + px(INSET + BTN));
        let (fw, fh) = Opts::size(kind, trash, scale);
        // 尖角不能伸出圆角：夹在本体左右各留 16 的范围里
        let notch = notch.filter(|_| up).map(|x| x.clamp(body.left + px(16.0), body.right - px(16.0)));
        Opts {
            rect: RECT { left, top, right: left + fw, bottom: top + fh },
            body,
            items: items.iter().map(|(i, x)| (*i, RECT { left: left + px(*x), top: it, right: left + px(x + BTN), bottom: ib })).collect(),
            seps: seps.iter().map(|x| left + px(*x)).collect(),
            kind,
            scale,
            notch,
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
        let f = |r: &RECT| rectf(r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
        let body = f(&self.body);
        ui::block(c, body, 10.0, SHADOW, ui::SURF, s);
        let lw = ui::line_w(s);
        if let Some(x) = self.notch {
            // 白色三角盖住本体上边框那一段，再描两条斜边
            let (x, hw, tip) = (x as f32, 8.0 * s, body.top - 7.0 * s);
            let fill = Path::new(true, |b| {
                b.move_to(x - hw, body.top + lw);
                b.line_to(x, tip);
                b.line_to(x + hw, body.top + lw);
                b.close();
                Some(())
            });
            let edge = Path::new(false, |b| {
                b.move_to(x - hw - lw / 2.0, body.top + lw / 2.0);
                b.line_to(x, tip);
                b.line_to(x + hw + lw / 2.0, body.top + lw / 2.0);
                Some(())
            });
            if let (Some(fill), Some(edge)) = (fill, edge) {
                c.fill_path(&fill, rgb(ui::SURF));
                c.stroke_path(&edge, rgb(ui::INK), lw, Stroke::Plain);
            }
        }
        let mid = (body.top + body.bottom) / 2.0;
        for x in &self.seps {
            c.fill_rect(rectf(*x as f32, mid - 11.0 * s, *x as f32 + lw, mid + 11.0 * s), rgb(ui::INK));
        }
        for (it, r) in &self.items {
            let r = f(r);
            let on = matches!(*it, Item::Size(i) if i == pen.1);
            let st = ui::state(true, on, hot == Some(*it), down == Some(*it));
            let (dy, fg) = ui::button_bg(c, r, st, s);
            let (cx, cy) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0 + dy);
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
                    // 18 的色块 + 2 的墨色内圈；选中时隔 2 的空隙再套一圈 2 的墨色
                    c.fill_ellipse(cx, cy, 9.0 * s, 9.0 * s, rgb(COLORS[i]));
                    c.stroke_ellipse(cx, cy, 9.0 * s - lw / 2.0, 9.0 * s - lw / 2.0, rgb(ui::INK), lw, Stroke::Plain);
                    if pen.0 == i {
                        let rr = 11.0 * s + lw / 2.0;
                        c.stroke_ellipse(cx, cy, rr, rr, rgb(ui::INK), lw, Stroke::Plain);
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
        // 内边 7 + 3 档 × 33 - 3 … 分组线占 2 + 2 + 5，6 色 × 33 - 3 + 内边 7
        assert_eq!(arrange(Kind::Line, false).2, 317.0);
        // 马赛克：只有 3 档；选中时再加分组线和删除
        assert_eq!(arrange(Kind::Mosaic, false).2, 110.0);
        assert_eq!(arrange(Kind::Mosaic, true).2, 152.0);
        let o = Opts::layout(Kind::Line, true, 100, 50, 1.5, true, Some(105));
        let b = o.button(Item::Size(0));
        assert_eq!((b.left, b.right - b.left), (100 + 11, 45));
        assert_eq!(o.hit(POINT { x: b.left + 1, y: b.top + 1 }), Some(Item::Size(0)));
        assert_eq!(o.rect.bottom - o.rect.top, 12 + 66 + 6);
        // 尖角夹在本体里面，不伸出左边的圆角
        assert_eq!(o.notch, Some(100 + 24));
        assert_eq!(Opts::layout(Kind::Line, true, 100, 50, 1.5, false, Some(130)).notch, None);
    }

    /// 操作栏（当前工具矩形、钉图悬停、撤销可用、重做置灰）和三种选项条，存成 target/test-tmp/annotate/bars.png。平时不跑：
    /// cargo test bar_samples -- --ignored
    #[test]
    #[ignore]
    fn bar_samples() {
        use crate::annotate::Tool;
        use crate::plat::Bitmap;
        use crate::toolbar::{Act, Bar, SHOT};
        let s = 1.5;
        let bmp = Bitmap::new(1200, 520).unwrap();
        let bar = Bar::layout(RECT { left: 0, top: 0, right: 1180, bottom: 0 }, 1200, 520, s, 0, &SHOT, None);
        {
            let c = Canvas::new(&bmp, bmp.full(), false);
            c.clear(rgb(0x707070));
            bar.draw(&c, Some(Act::Pin), None, Some(Act::Rect), |a| a != Act::Redo);
            let mut y = bar.rect.bottom + 4;
            for (kind, trash, pen, tool) in [(Kind::Line, false, (0, 1), Some(Tool::Rect)), (Kind::Text, true, (3, 2), Some(Tool::Text)), (Kind::Mosaic, true, (0, 0), None)] {
                let at = tool.map(|t| bar.button(crate::toolbar::act_of(t)));
                let x = at.map_or(bar.rect.left, |r| r.left - 11);
                let o = Opts::layout(kind, trash, x, y, s, true, at.map(|r| (r.left + r.right) / 2));
                o.draw(&c, pen, Some(Item::Color(2)), None);
                y = o.rect.bottom + 4;
            }
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("annotate");
        std::fs::create_dir_all(&dir).unwrap();
        let png = crate::plat::encode(&bmp.crop(&bmp.full()), false).unwrap();
        std::fs::write(dir.join("bars.png"), png).unwrap();
    }
}
