//! 操作栏：截图遮罩、钉图编辑、录屏结果共用。布局、绘制、命中；尺寸按设计稿（DIP），换算成物理像素后取整，按钮底色才清楚。
//! 按钮只放图标，名字和快捷键靠悬停提示（`tip`）；主按钮比普通按钮宽，紫底。
//! 也管标注工具按钮和标注工具的对应关系。只依赖 plat、ui、annotate，用它的模块之间互不依赖。

pub(crate) mod opts;

use crate::annotate::{Kind, Tool};
use crate::plat::{Canvas, Point as POINT, Rect as RECT, Text};
use crate::ui::{self, rectf, rgb};
use opts::Opts;

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
    /// 录屏选区上延时那排分段的一档：几秒后开始（0 是不延时）
    Delay(u8),
    Ocr,
    Translate,
    /// 截图选区上的「长截图」：关掉遮罩，边滚边截
    Long,
    Save,
    Cancel,
    Copy,
    Done,
    /// 钉图悬停条：编辑标注、关闭、动图暂停和播放
    Edit,
    Close,
    Pause,
    Play,
}

/// 一组按钮：(动作, 图标, 提示, 快捷键)；最后一个是主按钮（关闭除外）。图标为空的是写字的分段（一组只有一排），字就是提示
pub type Items = [(Act, &'static str, &'static str, &'static str)];

/// 截图遮罩
pub const SHOT: [(Act, &str, &str, &str); 16] = [
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
    (Act::Long, "long", "长截图", ""),
    (Act::Pin, "pin", "钉图", ""),
    (Act::Save, "save", "保存", "Ctrl+S"),
    (Act::Cancel, "cancel", "取消", "Esc"),
    (Act::Copy, "copy", "复制", "Enter"),
];
/// 录屏模式的选区：前面一排延时分段，主按钮是开始录制
pub const RECORD: [(Act, &str, &str, &str); 6] = [
    (Act::Delay(0), "", "不延时", ""),
    (Act::Delay(3), "", "3 秒", ""),
    (Act::Delay(5), "", "5 秒", ""),
    (Act::Delay(10), "", "10 秒", ""),
    (Act::Cancel, "cancel", "取消", "Esc"),
    (Act::Record, "record", "开始录制", "Enter"),
];
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
/// 录屏、长截图的结果（原位钉着的动图、长图下面）：主按钮是复制，和截图一致
pub const RESULT: [(Act, &str, &str, &str); 4] =
    [(Act::Pin, "pin", "钉在屏幕上", ""), (Act::Save, "save", "保存", "Ctrl+S"), (Act::Cancel, "trash", "丢弃", "Esc"), (Act::Copy, "copy", "复制", "Enter")];
/// 长截图进行中（区域旁边那条）：主按钮是完成
pub const LONG: [(Act, &str, &str, &str); 2] = [(Act::Cancel, "cancel", "取消", "Esc"), (Act::Done, "check", "完成", "Enter")];
/// 钉图悬停条（鼠标停在钉图上时下面那条）：没有主按钮
pub const HOVER: [(Act, &str, &str, &str); 6] = [
    (Act::Edit, "edit", "编辑标注", ""),
    (Act::Ocr, "ocr", "取字", ""),
    (Act::Translate, "translate", "翻译", ""),
    (Act::Copy, "copy", "复制", "Ctrl+C"),
    (Act::Save, "save", "保存", "Ctrl+S"),
    (Act::Close, "close", "关闭", "Esc"),
];
/// 动图钉图的悬停条，播放中和暂停时两套：取字、翻译认当前这一帧，复制、保存的是 GIF 文件
pub const HOVER_GIF: [(Act, &str, &str, &str); 6] = [
    (Act::Pause, "pause", "暂停", ""),
    (Act::Ocr, "ocr", "取字（当前帧）", ""),
    (Act::Translate, "translate", "翻译（当前帧）", ""),
    (Act::Copy, "copy", "复制 GIF", "Ctrl+C"),
    (Act::Save, "save", "保存 GIF", "Ctrl+S"),
    (Act::Close, "close", "关闭", "Esc"),
];
pub const HOVER_GIF_PAUSED: [(Act, &str, &str, &str); 6] = [
    (Act::Play, "play", "播放", ""),
    HOVER_GIF[1],
    HOVER_GIF[2],
    HOVER_GIF[3],
    HOVER_GIF[4],
    HOVER_GIF[5],
];
/// 这些按钮后面画分组线；「关闭」前面也画
const SEP_AFTER: [Act; 7] = [Act::Mosaic, Act::Redo, Act::Translate, Act::Delay(10), Act::Edit, Act::Pause, Act::Play];

/// 主按钮：最后一个，关闭不算
fn main_of(all: &Items) -> Option<Act> {
    all.last().map(|i| i.0).filter(|a| *a != Act::Close)
}

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

// 尺寸（DIP）：描边 2 + 内边距 5，按钮 34，间距 3；分组线 2 × 22、左右各 5；主按钮 44 宽、前面多空 2；实心阴影 4
const INSET: f32 = 7.0;
const BTN: f32 = 34.0;
const GAP: f32 = 3.0;
const SEP_M: f32 = 5.0;
const MAIN_W: f32 = 44.0;
const SHADOW: f32 = 4.0;

pub struct Bar {
    pub rect: RECT,
    /// 在 anchor 下方（工具选项条跟着放在操作栏下面；否则放上面）
    pub below: bool,
    items: &'static Items,
    buttons: Vec<(Act, RECT)>,
    seps: Vec<i32>,
    scale: f32,
    /// 最左边的一段说明（录屏结果的「00:12 · 1.3 MB」）和它的 x
    info: Option<(Text, i32)>,
}

impl Bar {
    /// 右边和 anchor（选区）右边对齐；先放下方 8 处，放不下放上方，都不行放进里面的右下角；
    /// 左右推回屏内，离边至少 4。`opt_h` 是工具选项条连同间距的高度（没有时 0），
    /// `all`：用哪组按钮（SHOT、RECORD、PIN、RESULT）；`info`：按钮前面的一段说明，后面跟分组线（紧跟分段时不加）。`rect` 连右下的实心阴影一起算。
    pub fn layout(anchor: RECT, screen_w: i32, screen_h: i32, scale: f32, opt_h: i32, all: &'static Items, info: Option<&str>) -> Bar {
        let px = |dip: f32| (dip * scale).round() as i32;
        // 先按 DIP 从左往右排
        let (mut x, mut items, mut seps) = (INSET, Vec::new(), Vec::new());
        let seg = |i: usize| all.get(i).is_some_and(|it| it.1.is_empty());
        let info = info.and_then(|t| Text::tabular(crate::plat::Face::Ui, 12.5 * scale, true, t)).map(|t| {
            let at = INSET + 5.0;
            x = at + t.width() / scale + 8.0;
            if !seg(0) {
                seps.push(x);
                x += 2.0 + SEP_M;
            }
            (t, at)
        });
        for (i, (a, _, label, _)) in all.iter().enumerate() {
            let main = Some(*a) == main_of(all);
            if main {
                x += 2.0;
            }
            // 分段：外框内边距 3，段间距 2，宽是字 + 左右各 12
            if seg(i) && (i == 0 || !seg(i - 1)) {
                x += 3.0;
            }
            let w = match () {
                _ if main => MAIN_W,
                _ if seg(i) => (Text::new(crate::plat::Face::Ui, 12.5 * scale, true, label).map_or(0.0, |t| t.width()) / scale + 24.0).ceil(),
                _ => BTN,
            };
            items.push((*a, x, w));
            x += w;
            if seg(i) && !seg(i + 1) {
                x += 3.0;
            }
            if i + 1 < all.len() {
                x += if seg(i) && seg(i + 1) { 2.0 } else { GAP };
            }
            if SEP_AFTER.contains(a) || all.get(i + 1).is_some_and(|n| n.0 == Act::Close) {
                // 按钮后面已经空了 GAP，分组线离两边按钮各 SEP_M
                seps.push(x + SEP_M - GAP);
                x += SEP_M - GAP + 2.0 + SEP_M;
            }
        }
        // 本体宽高，外面再加阴影
        let (bw, bh, d) = (px(x + INSET), px(INSET + BTN + INSET), px(SHADOW));
        let (m, g) = (px(4.0), px(8.0));
        let left = (anchor.right - bw).min(screen_w - bw - d - m).max(m);
        let (top, below) = if anchor.bottom + g + bh + d + opt_h <= screen_h - m {
            (anchor.bottom + g, true)
        } else if anchor.top - g - bh - d - opt_h >= m {
            (anchor.top - g - bh - d, false)
        } else {
            ((anchor.bottom - g - bh - d).min(screen_h - m - bh - d).max(m + opt_h), false)
        };
        let (bt, bb) = (top + px(INSET), top + px(INSET + BTN));
        let buttons = items
            .iter()
            .enumerate()
            .map(|(i, (a, x, w))| {
                let inset = if seg(i) { px(3.0) } else { 0 };
                (*a, RECT { left: left + px(*x), top: bt + inset, right: left + px(x + w), bottom: bb - inset })
            })
            .collect();
        Bar {
            rect: RECT { left, top, right: left + bw + d, bottom: top + bh + d },
            below,
            items: all,
            buttons,
            seps: seps.iter().map(|x| left + px(*x)).collect(),
            scale,
            info: info.map(|(t, at)| (t, left + px(at))),
        }
    }

    /// 整条平移 (dx, dy)
    pub fn moved(mut self, dx: i32, dy: i32) -> Bar {
        let mv = |r: &mut RECT| *r = RECT { left: r.left + dx, top: r.top + dy, right: r.right + dx, bottom: r.bottom + dy };
        mv(&mut self.rect);
        self.buttons.iter_mut().for_each(|(_, r)| mv(r));
        self.seps.iter_mut().for_each(|x| *x += dx);
        if let Some((_, x)) = &mut self.info {
            *x += dx;
        }
        self
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

    /// 按钮的图标；写字的分段是空的
    pub fn icon(&self, a: Act) -> &'static str {
        self.items.iter().find(|i| i.0 == a).unwrap().1
    }

    pub fn button(&self, a: Act) -> RECT {
        self.buttons.iter().find(|(b, _)| *b == a).map(|(_, r)| *r).unwrap_or(self.rect)
    }

    /// `hot`：鼠标下的按钮；`down`：按住的按钮；`on`：当前工具（或选中的那段）；`enabled`：不能用的按钮置灰
    pub fn draw(&self, c: &Canvas, hot: Option<Act>, down: Option<Act>, on: Option<Act>, enabled: impl Fn(Act) -> bool) {
        let (s, main) = (self.scale, main_of(self.items));
        let f = |r: &RECT| rectf(r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
        let d = (SHADOW * s).round();
        let body = f(&self.rect);
        let body = rectf(body.left, body.top, body.right - d, body.bottom - d);
        ui::block(c, body, 10.0, SHADOW, ui::SURF, s);
        let lw = ui::line_w(s);
        let mid = (body.top + body.bottom) / 2.0;
        for x in &self.seps {
            c.fill_rect(rectf(*x as f32, mid - 11.0 * s, *x as f32 + lw, mid + 11.0 * s), rgb(ui::INK));
        }
        if let Some((t, x)) = &self.info {
            c.text(t, *x as f32, mid - t.height() / 2.0, rgb(ui::INK));
        }
        // 分段的外框：白底墨色描边，圈住那一排
        let segs: Vec<RECT> = self.buttons.iter().filter(|(a, _)| self.icon(*a).is_empty()).map(|(_, r)| *r).collect();
        if let (Some(a), Some(b)) = (segs.first(), segs.last()) {
            let p = (3.0 * s).round();
            ui::block(c, rectf(a.left as f32 - p, a.top as f32 - p, b.right as f32 + p, b.bottom as f32 + p), 7.0, 0.0, ui::SURF, s);
        }
        for (a, r) in &self.buttons {
            let r = f(r);
            let icon_name = self.icon(*a);
            if icon_name.is_empty() {
                // 一段：选中的墨底白字，悬停浅紫
                let fg = if on == Some(*a) {
                    c.fill_round(r, 5.0 * s, rgb(ui::INK));
                    0xFFFFFF
                } else if hot == Some(*a) {
                    c.fill_round(r, 5.0 * s, rgb(if down == Some(*a) { ui::PRESS } else { ui::HOVER }));
                    ui::INK
                } else {
                    ui::MUTED
                };
                if let Some(l) = Text::new(crate::plat::Face::Ui, 12.5 * s, true, self.tip(*a).0) {
                    c.text(&l, (r.left + r.right - l.width()) / 2.0, (r.top + r.bottom - l.height()) / 2.0, rgb(fg));
                }
                continue;
            }
            let st = ui::state(enabled(*a), on == Some(*a) && Some(*a) != main, hot == Some(*a), down == Some(*a));
            let (face, dy, fg) = if Some(*a) == main {
                (ui::primary(c, r, st, s), 0.0, rgb(0xFFFFFF))
            } else {
                let (dy, fg) = ui::button_bg(c, r, st, s);
                (r, dy, fg)
            };
            let x = face.left + (face.right - face.left - 20.0 * s) / 2.0;
            let y = face.top + (face.bottom - face.top - 20.0 * s) / 2.0 + dy;
            ui::icon(c, icon_name, x, y, 20.0 * s, fg);
        }
    }

    /// 工具选项条：跟着当前工具，第一个按钮对准工具按钮，小尖角指着它；没有当前工具（选中了已画的标注）就和操作栏左对齐、不画尖角。
    /// 放在操作栏远离 anchor 的那一侧（操作栏在 anchor 里面时放它上方）。屏幕宽 `screen_w`，左右离边至少 4
    pub fn opts(&self, kind: Kind, trash: bool, tool: Option<Tool>, screen_w: i32) -> Opts {
        let s = self.scale;
        let px = |dip: f32| (dip * s).round() as i32;
        let (ow, oh) = Opts::size(kind, trash, s);
        let at = tool.map(|t| self.button(act_of(t)));
        let x = at.map_or(self.rect.left, |r| r.left - px(INSET)).min(screen_w - ow - px(4.0)).max(px(4.0));
        let y = if self.below { self.rect.bottom + px(2.0) } else { self.rect.top - px(2.0) - oh };
        Opts::layout(kind, trash, x, y, s, self.below, at.map(|r| (r.left + r.right) / 2))
    }
}

/// 工具选项条连同和操作栏之间的间距有多高：给 `Bar::layout` 的 `opt_h`
pub fn opts_height(kind: Kind, trash: bool, scale: f32) -> i32 {
    Opts::size(kind, trash, scale).1 + (2.0 * scale).round() as i32
}
