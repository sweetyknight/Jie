//! 操作栏：截图遮罩、钉图编辑、录屏结果共用。布局、绘制、命中；尺寸按设计稿（DIP），换算成物理像素后取整，按钮底色才清楚。
//! 也管标注工具按钮和标注工具的对应关系。只依赖 plat、ui、annotate，用它的模块之间互不依赖。

pub(crate) mod opts;

use crate::annotate::Tool;
use crate::plat::{Canvas, Point as POINT, Rect as RECT, Text};
use crate::ui::{self, rectf, rgb};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    Rect,
    Ellipse,
    Arrow,
    Pen,
    Text,
    Num,
    Mosaic,
    Undo,
    Redo,
    Pin,
    Record,
    /// 录屏选区上的「延时开始」：点一下换一档（不延时、3、5、10 秒）
    Delay,
    Ocr,
    Translate,
    Save,
    Cancel,
    Copy,
    Done,
}

/// 一组按钮：(动作, 图标, 提示, 快捷键)；最后一个是主按钮，文字由调用方给
pub type Items = [(Act, &'static str, &'static str, &'static str)];

/// 截图遮罩
pub const SHOT: [(Act, &str, &str, &str); 15] = [
    (Act::Rect, "rect", "矩形", "R"),
    (Act::Ellipse, "ellipse", "椭圆", "E"),
    (Act::Arrow, "arrow", "箭头", "A"),
    (Act::Pen, "pen", "画笔", "P"),
    (Act::Text, "text", "文字", "T"),
    (Act::Num, "num", "序号", "N"),
    (Act::Mosaic, "mosaic", "马赛克", "M"),
    (Act::Undo, "undo", "撤销", "Ctrl+Z"),
    (Act::Redo, "redo", "重做", "Ctrl+Y"),
    (Act::Ocr, "ocr", "取字", ""),
    (Act::Translate, "translate", "翻译", ""),
    (Act::Pin, "pin", "钉图", ""),
    (Act::Save, "save", "保存", "Ctrl+S"),
    (Act::Cancel, "cancel", "取消", "Esc"),
    (Act::Copy, "copy", "复制", "Enter"),
];
/// 录屏快捷键打开的选区：只能框选，主按钮是开始录制，旁边选要不要延时
pub const RECORD: [(Act, &str, &str, &str); 3] =
    [(Act::Cancel, "cancel", "取消", "Esc"), (Act::Delay, "timer", "延时开始", ""), (Act::Record, "record", "开始录制", "Enter")];
/// 钉图编辑：改动即时生效，没有钉图和取消；复制是普通按钮，主按钮是完成
pub const PIN: [(Act, &str, &str, &str); 14] = [
    (Act::Rect, "rect", "矩形", "R"),
    (Act::Ellipse, "ellipse", "椭圆", "E"),
    (Act::Arrow, "arrow", "箭头", "A"),
    (Act::Pen, "pen", "画笔", "P"),
    (Act::Text, "text", "文字", "T"),
    (Act::Num, "num", "序号", "N"),
    (Act::Mosaic, "mosaic", "马赛克", "M"),
    (Act::Undo, "undo", "撤销", "Ctrl+Z"),
    (Act::Redo, "redo", "重做", "Ctrl+Y"),
    (Act::Ocr, "ocr", "取字", ""),
    (Act::Translate, "translate", "翻译", ""),
    (Act::Save, "save", "保存", "Ctrl+S"),
    (Act::Copy, "copy", "复制", "Ctrl+C"),
    (Act::Done, "check", "完成编辑", "Enter"),
];
/// 录屏结果（原位播放的动图下面）：主按钮是复制，和截图一致
pub const RESULT: [(Act, &str, &str, &str); 4] =
    [(Act::Pin, "pin", "钉在屏幕上", ""), (Act::Save, "save", "保存", "Ctrl+S"), (Act::Cancel, "trash", "丢弃", "Esc"), (Act::Copy, "copy", "复制", "Enter")];
/// 这些按钮后面画分组线
const SEP_AFTER: [Act; 3] = [Act::Mosaic, Act::Redo, Act::Translate];

/// 操作栏上的标注工具按钮
const TOOL_ACTS: [(Act, Tool); 7] = [
    (Act::Rect, Tool::Rect),
    (Act::Ellipse, Tool::Ellipse),
    (Act::Arrow, Tool::Arrow),
    (Act::Pen, Tool::Pen),
    (Act::Text, Tool::Text),
    (Act::Num, Tool::Num),
    (Act::Mosaic, Tool::Mosaic),
];

pub fn tool_of(a: Act) -> Option<Tool> {
    TOOL_ACTS.iter().find(|(x, _)| *x == a).map(|(_, t)| *t)
}

pub fn act_of(t: Tool) -> Act {
    TOOL_ACTS.iter().find(|(_, x)| *x == t).map_or(Act::Rect, |(a, _)| *a)
}

const TEXT: u32 = 0x1A1A1A;
const TEXT_DISABLED: u32 = 0xA8A8A8;
const ACCENT: u32 = 0x9C44EC;
const ACCENT_SOFT: u32 = 0xF3E9FD;

pub struct Bar {
    pub rect: RECT,
    /// 在 anchor 下方（工具选项条跟着放在操作栏下面；否则放上面）
    pub below: bool,
    items: &'static Items,
    buttons: Vec<(Act, RECT)>,
    seps: Vec<i32>,
    scale: f32,
    /// 按钮右下角的小角标（比如延时的秒数）；有角标的按钮画成选中的样子
    pub badge: Option<(Act, String)>,
}

impl Bar {
    /// 右边和 anchor（选区）右边对齐；先放下方 8 处，放不下放上方，都不行放进里面的右下角；
    /// 左右推回屏内，离边至少 4。`label_w` 是主按钮文字宽度（像素），`opt_h` 是工具选项条连同间距 4 的高度（没有时 0），
    /// `all`：用哪组按钮（SHOT、RECORD、PIN、RESULT）。
    pub fn layout(anchor: RECT, screen_w: i32, screen_h: i32, scale: f32, label_w: f32, opt_h: i32, all: &'static Items) -> Bar {
        let px = |dip: f32| (dip * scale).round() as i32;
        // 先按 DIP 从左往右排：外框 1 + 内边距 3，按钮 32，间距 2，分组线左右各再留 4
        let (mut x, mut items, mut seps) = (4.0, Vec::new(), Vec::new());
        for (i, (a, ..)) in all.iter().enumerate() {
            let w = if i + 1 == all.len() {
                x += 2.0; // 主按钮多留 2
                8.0 + 20.0 + 4.0 + label_w / scale + 12.0
            } else {
                32.0
            };
            items.push((*a, x, w));
            x += w;
            if i + 1 < all.len() {
                x += 2.0;
            }
            if SEP_AFTER.contains(a) {
                seps.push(x + 4.0);
                x += 4.0 + 1.0 + 4.0 + 2.0;
            }
        }
        let (bw, bh) = (px(x + 4.0), px(40.0));
        let (m, g) = (px(4.0), px(8.0));
        let left = (anchor.right - bw).min(screen_w - bw - m).max(m);
        let (top, below) = if anchor.bottom + g + bh + opt_h <= screen_h - m {
            (anchor.bottom + g, true)
        } else if anchor.top - g - bh - opt_h >= m {
            (anchor.top - g - bh, false)
        } else {
            ((anchor.bottom - g - bh).min(screen_h - m - bh).max(m + opt_h), false)
        };
        let (bt, bb) = (top + px(4.0), top + px(36.0));
        let buttons = items.iter().map(|(a, x, w)| (*a, RECT { left: left + px(*x), top: bt, right: left + px(x + w), bottom: bb })).collect();
        Bar {
            rect: RECT { left, top, right: left + bw, bottom: top + bh },
            below,
            items: all,
            buttons,
            seps: seps.iter().map(|x| left + px(*x)).collect(),
            scale,
            badge: None,
        }
    }

    pub fn hit(&self, p: POINT) -> Option<Act> {
        let inside = |r: &RECT| p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom;
        self.buttons.iter().find(|(_, r)| inside(r)).map(|(a, _)| *a)
    }

    /// 提示文字和快捷键
    pub fn tip(&self, a: Act) -> (&'static str, &'static str) {
        let (_, _, t, k) = self.items.iter().find(|i| i.0 == a).unwrap();
        (t, k)
    }

    pub fn button(&self, a: Act) -> RECT {
        self.buttons.iter().find(|(b, _)| *b == a).map(|(_, r)| *r).unwrap_or(self.rect)
    }

    /// `hot`：鼠标下的按钮；`down`：按住的按钮；`on`：当前工具；`enabled`：不能用的按钮置灰
    pub fn draw(&self, c: &Canvas, label: &Text, hot: Option<Act>, down: Option<Act>, on: Option<Act>, enabled: impl Fn(Act) -> bool) {
        let (s, on_tool, main) = (self.scale, on, self.items[self.items.len() - 1].0);
        let f = |r: &RECT| (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
        // 1 像素以上的整像素描边画在像素中心，外框才不糊
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
        for (a, r) in &self.buttons {
            let (l, t, rr, bb) = f(r);
            let on = enabled(*a);
            let state = |normal: u32, hover: u32, press: u32| match (on && hot == Some(*a), down == Some(*a)) {
                (true, true) => Some(press),
                (true, false) => Some(hover),
                _ => (normal != 0).then_some(normal),
            };
            let icon_name = self.items.iter().find(|i| i.0 == *a).unwrap().1;
            let icon_y = t + (bb - t - 20.0 * s) / 2.0;
            if *a == main {
                let bg = state(ACCENT, 0xA95CF0, 0x8733D4).unwrap();
                c.fill_round(rectf(l, t, rr, bb), 4.0 * s, rgb(if on { bg } else { TEXT_DISABLED }));
                ui::icon(c, icon_name, l + 8.0 * s, icon_y, 20.0 * s, rgb(0xFFFFFF));
                c.text(label, l + 32.0 * s, t + (bb - t - label.height()) / 2.0, rgb(0xFFFFFF));
            } else {
                // 当前工具、带角标的：主题浅底、主题色图标，悬停也不变
                let current = on_tool == Some(*a) || self.badge.as_ref().is_some_and(|b| b.0 == *a);
                let bg = if current { Some(ACCENT_SOFT) } else { state(0, 0xEBEBEB, 0xE0E0E0) };
                if let Some(bg) = bg {
                    c.fill_round(rectf(l, t, rr, bb), 4.0 * s, rgb(bg));
                }
                let x = l + (rr - l - 20.0 * s) / 2.0;
                let fg = if current { ACCENT } else if on { TEXT } else { TEXT_DISABLED };
                ui::icon(c, icon_name, x, icon_y, 20.0 * s, rgb(fg));
            }
        }
        // 角标：主题色圆角底、白色小字，贴着按钮右下角
        if let Some((a, t)) = &self.badge
            && let Some(l) = Text::tabular(crate::plat::Face::Ui, 9.0 * s, true, t)
        {
            let r = self.button(*a);
            let (w, h) = ((l.width() + 6.0 * s).max(13.0 * s), 13.0 * s);
            let (x, y) = (r.right as f32 - w + 1.0 * s, r.bottom as f32 - h + 1.0 * s);
            c.fill_round(rectf(x, y, x + w, y + h), h / 2.0, rgb(ACCENT));
            c.text(&l, x + (w - l.width()) / 2.0, y + (h - l.height()) / 2.0, rgb(0xFFFFFF));
        }
    }
}
