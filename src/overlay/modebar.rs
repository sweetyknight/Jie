//! 遮罩顶上的模式条：「截图 | 录屏」分段 + 一句操作说明 + 「Tab 切换」。只依赖 plat、ui。

use crate::plat::{Canvas, Face, Point, Rect, Text};
use crate::ui::{self, rectf, rgb};

/// 尺寸（DIP）：描边 2 + 内边距 5；分段外框内边距 3、段高 28、段间距 2；实心阴影 4
const INSET: f32 = 7.0;
const SEG_H: f32 = 28.0;
const SHADOW: f32 = 4.0;
/// 两段的图标和名字：截图、录屏
const SEGS: [(&str, &str); 2] = [("crop", "截图"), ("record", "录屏")];

pub struct ModeBar {
    /// 连右下的实心阴影
    pub rect: Rect,
    record: bool,
    /// 截图、录屏两段
    segs: [Rect; 2],
    labels: [Text; 2],
    hint: Text,
    kbd: Text,
    /// 说明字、「Tab 切换」框的左边
    hint_x: i32,
    kbd_x: i32,
    scale: f32,
}

impl ModeBar {
    /// `record`：现在是录屏模式。水平居中在 `cx`，顶在 `top`（像素）
    pub fn layout(record: bool, cx: i32, top: i32, scale: f32) -> Option<ModeBar> {
        let px = |d: f32| (d * scale).round() as i32;
        let hint = if record { "框选要录的区域 · Enter 开始 · Esc 退出" } else { "拖动框选 · 滚轮换层 · Enter 复制 · Esc 退出" };
        let labels = [Text::new(Face::Ui, 13.0 * scale, true, SEGS[0].1)?, Text::new(Face::Ui, 13.0 * scale, true, SEGS[1].1)?];
        let hint = Text::new(Face::Ui, 12.5 * scale, false, hint)?;
        let kbd = Text::new(Face::Ui, 11.5 * scale, true, "Tab 切换")?;
        // 段宽：左右各 12、图标 16、间距 6，加上字
        let w0 = px(46.0) + labels[0].width().ceil() as i32;
        let w1 = px(46.0) + labels[1].width().ceil() as i32;
        let x0 = px(INSET + 3.0);
        let hint_x = x0 + w0 + px(2.0) + w1 + px(3.0 + 10.0);
        let kbd_x = hint_x + hint.width().ceil() as i32 + px(10.0);
        let (bw, bh, d) = (kbd_x + kbd.width().ceil() as i32 + px(12.0 + 14.0), px(INSET * 2.0 + SEG_H + 6.0), px(SHADOW));
        let left = cx - bw / 2;
        let (st, sb) = (top + px(INSET + 3.0), top + px(INSET + 3.0 + SEG_H));
        Some(ModeBar {
            rect: Rect { left, top, right: left + bw + d, bottom: top + bh + d },
            record,
            segs: [
                Rect { left: left + x0, top: st, right: left + x0 + w0, bottom: sb },
                Rect { left: left + x0 + w0 + px(2.0), top: st, right: left + x0 + w0 + px(2.0) + w1, bottom: sb },
            ],
            labels,
            hint,
            kbd,
            hint_x: left + hint_x,
            kbd_x: left + kbd_x,
            scale,
        })
    }

    /// 点在哪一段：false 截图，true 录屏
    pub fn hit(&self, p: Point) -> Option<bool> {
        let inside = |r: &Rect| p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom;
        self.segs.iter().position(inside).map(|i| i == 1)
    }

    pub fn seg(&self, record: bool) -> Rect {
        self.segs[usize::from(record)]
    }

    /// 当前模式那段墨底白字；`hot`、`down`：鼠标下、按住的那段；`rec_ok` 为 false 时「录屏」置灰（有标注）
    pub fn draw(&self, c: &Canvas, hot: Option<bool>, down: Option<bool>, rec_ok: bool) {
        let s = self.scale;
        let d = (SHADOW * s).round();
        let f = |r: &Rect| rectf(r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
        let body = f(&self.rect);
        let body = rectf(body.left, body.top, body.right - d, body.bottom - d);
        ui::block(c, body, 10.0, SHADOW, ui::SURF, s);
        let p = (3.0 * s).round();
        let (a, b) = (f(&self.segs[0]), f(&self.segs[1]));
        ui::block(c, rectf(a.left - p, a.top - p, b.right + p, b.bottom + p), 7.0, 0.0, ui::SURF, s);
        let mid = (body.top + body.bottom) / 2.0;
        for (i, r) in self.segs.iter().enumerate() {
            let (rec, r) = (i == 1, f(r));
            let fg = if rec == self.record {
                c.fill_round(r, 5.0 * s, rgb(ui::INK));
                0xFFFFFF
            } else if rec && !rec_ok {
                ui::FAINT
            } else if hot == Some(rec) {
                c.fill_round(r, 5.0 * s, rgb(if down == Some(rec) { ui::PRESS } else { ui::HOVER }));
                ui::INK
            } else {
                ui::MUTED
            };
            ui::icon(c, SEGS[i].0, r.left + 12.0 * s, mid - 8.0 * s, 16.0 * s, rgb(fg));
            let l = &self.labels[i];
            c.text(l, r.left + 34.0 * s, mid - l.height() / 2.0, rgb(fg));
        }
        c.text(&self.hint, self.hint_x as f32, mid - self.hint.height() / 2.0, rgb(ui::MUTED));
        // 「Tab 切换」：1.5 灰圈，高 20
        let (x, k) = (self.kbd_x as f32, &self.kbd);
        let lw = (1.5 * s).max(1.0);
        let r = rectf(x + lw / 2.0, mid - 10.0 * s + lw / 2.0, x + k.width() + 12.0 * s - lw / 2.0, mid + 10.0 * s - lw / 2.0);
        c.stroke_round(r, 5.0 * s - lw / 2.0, rgb(ui::MUTED), lw);
        c.text(k, x + 6.0 * s, mid - k.height() / 2.0, rgb(ui::MUTED));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_and_hit() {
        let m = ModeBar::layout(false, 960, 30, 1.5).unwrap();
        // 居中、两段挨着排在框里
        // 阴影 4 × 1.5 = 6 不算在居中里
        assert!((m.rect.left + m.rect.right - 6 - 1920).abs() <= 1);
        assert!(m.segs[0].right < m.segs[1].left && m.segs[1].right < m.hint_x && m.hint_x < m.kbd_x && m.kbd_x < m.rect.right);
        let p = |r: Rect| Point { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 };
        assert_eq!((m.hit(p(m.seg(false))), m.hit(p(m.seg(true)))), (Some(false), Some(true)));
        assert_eq!(m.hit(Point { x: m.hint_x + 2, y: m.segs[0].top + 2 }), None);
    }
}
