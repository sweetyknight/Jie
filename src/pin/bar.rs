//! 钉图编辑的操作栏窗口：贴在钉图旁边的透明小窗口，画操作栏、工具选项条和工具提示，位置规则同截图。
//! 不抢焦点，键盘一直在钉图上；透明的地方鼠标穿透。钉图每次变了把编辑器的样子交过来（`place`），点了按钮交回钉图处理。

use super::Pin;
use crate::annotate::{Editor, Kind};
use crate::overlay::bar::{Act, Bar};
use crate::overlay::opts::{self, Item, Opts};
use crate::plat::{self, Affine, Button, Canvas, Event, Face, Handler, Point, Rect, Spec, Text, Win};
use crate::ui::{self, Hud};

/// 工具提示：悬停 400ms 后出现
pub(super) const TIP_TIMER: usize = 1;

/// 操作栏和工具选项条上的按钮
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Btn {
    Bar(Act),
    Opt(Item),
}

/// 编辑器的界面状态：当前工具、选项条（样子、有没有删除）、颜色和粗细、能不能撤销和重做
#[derive(Clone, Copy, PartialEq, Default)]
struct Look {
    tool: Option<Act>,
    opt: Option<(Kind, bool)>,
    pen: (usize, usize),
    undo: bool,
    redo: bool,
}

impl Look {
    fn of(ed: &Editor) -> Look {
        Look { tool: ed.tool.map(crate::overlay::act_of), opt: ed.opts(), pen: ed.pen(), undo: ed.doc.can_undo(), redo: ed.doc.can_redo() }
    }
}

pub(super) struct Chrome {
    pin: Win,
    /// 主按钮的「完成」两个字，按 scale 排的
    label: Text,
    scale: f32,
    look: Look,
    /// 钉图的图片区域（屏幕坐标）
    img: Rect,
    /// 鼠标下、按住、显示提示的按钮
    hot: Option<Btn>,
    down: Option<Btn>,
    tip: Option<Btn>,
    /// 上次摆好的操作栏、选项条、提示（相对工作区左上角），窗口左上角在同一坐标里的位置
    bar: Option<Bar>,
    opts: Option<Opts>,
    hud: Option<(Hud, i32, i32)>,
    origin: Point,
}

fn label(s: f32) -> Option<Text> {
    Text::new(Face::Ui, 13.0 * s, true, "完成")
}

/// 建操作栏窗口（先不显示，第一次 `place` 时出现）；归钉图所有：总在钉图上面，钉图关掉时跟着销毁
pub(super) fn open(pin: Win) -> Option<Win> {
    let scale = pin.scale();
    let c = Chrome { pin, label: label(scale)?, scale, look: Look::default(), img: Rect::default(), hot: None, down: None, tip: None, bar: None, opts: None, hud: None, origin: Point::default() };
    Win::open(Spec { kind: plat::Kind::PinBar, rect: Rect::default(), title: "Jie 钉图编辑", owner: Some(pin) }, c)
}

/// 按钉图的图片区域 `img`（屏幕坐标）和编辑器现在的样子摆好、重画并显示
pub(super) fn place(bar: Win, img: Rect, ed: &Editor) {
    let look = Look::of(ed);
    if bar.with(|c: &mut Chrome| c.layout(bar, img, look)).is_some() {
        bar.show();
    }
}

impl Chrome {
    fn enabled(&self, b: Btn) -> bool {
        match b {
            Btn::Bar(Act::Undo) => self.look.undo,
            Btn::Bar(Act::Redo) => self.look.redo,
            _ => true,
        }
    }

    /// 摆操作栏、选项条和提示，窗口刚好包住这三样
    fn layout(&mut self, w: Win, img: Rect, look: Look) {
        (self.img, self.look) = (img, look);
        let mon = plat::monitor_at(Point { x: (img.left + img.right) / 2, y: (img.top + img.bottom) / 2 });
        let s = mon.scale();
        // 钉图拖到了缩放比例不同的显示器上
        if s != self.scale
            && let Some(l) = label(s)
        {
            (self.label, self.scale) = (l, s);
        }
        // 在工作区里排，坐标相对工作区左上角
        let work = mon.work;
        let (sw, sh) = (work.right - work.left, work.bottom - work.top);
        let px = |dip: f32| (dip * s).round() as i32;
        let anchor = Rect { left: img.left - work.left, top: img.top - work.top, right: img.right - work.left, bottom: img.bottom - work.top };
        let opt_h = look.opt.map_or(0, |(k, t)| Opts::size(k, t, s).1 + px(4.0));
        let bar = Bar::layout(anchor, sw, sh, s, self.label.width(), opt_h, true);
        // 选项条：和操作栏左对齐，在操作栏远离钉图的那一侧（操作栏在钉图里面时放它上方），间距 4
        let opts = look.opt.map(|(kind, trash)| {
            let (ow, oh) = Opts::size(kind, trash, s);
            let x = bar.rect.left.min(sw - ow - px(4.0)).max(px(4.0));
            let y = if bar.below { bar.rect.bottom + px(4.0) } else { bar.rect.top - px(4.0) - oh };
            Opts::layout(kind, trash, x, y, s)
        });
        // 提示：按钮下方 6 处、水平居中；下面放不下就放上方
        let hud = self.tip.and_then(|b| {
            let (r, (name, key)) = match b {
                Btn::Bar(a) => (bar.button(a), bar.tip(a)),
                Btn::Opt(i) => (opts.as_ref()?.button(i), opts::tip(look.opt?.0, i)),
            };
            let parts: &[(&str, u32)] = if key.is_empty() { &[(name, 0xFFFFFF)] } else { &[(name, 0xFFFFFF), (key, ui::HUD_TEXT2)] };
            let hud = Hud::new(parts, s)?;
            let mut y = r.bottom + px(6.0);
            if y + hud.h > sh - px(4.0) {
                y = r.top - px(6.0) - hud.h;
            }
            let x = ((r.left + r.right - hud.w) / 2).min(sw - hud.w - px(4.0)).max(px(4.0));
            Some((hud, x, y))
        });
        let mut u = bar.rect;
        for r in opts.iter().map(|o| o.rect).chain(hud.iter().map(|(h, x, y)| Rect { left: *x, top: *y, right: x + h.w, bottom: y + h.h })) {
            u = Rect { left: u.left.min(r.left), top: u.top.min(r.top), right: u.right.max(r.right), bottom: u.bottom.max(r.bottom) };
        }
        self.origin = Point { x: u.left, y: u.top };
        (self.bar, self.opts, self.hud) = (Some(bar), opts, hud);
        w.set_rect(Rect { left: work.left + u.left, top: work.top + u.top, right: work.left + u.right, bottom: work.top + u.bottom });
    }

    /// 鼠标、提示变了：按上次的图片区域和样子重新摆
    fn relayout(&mut self, w: Win) {
        self.layout(w, self.img, self.look);
    }

    /// 客户区坐标 p 下的按钮
    fn button_at(&self, p: Point) -> Option<Btn> {
        let p = Point { x: p.x + self.origin.x, y: p.y + self.origin.y };
        if let Some(i) = self.opts.as_ref().and_then(|o| o.hit(p)) {
            return Some(Btn::Opt(i));
        }
        self.bar.as_ref()?.hit(p).map(Btn::Bar)
    }

    /// 按钮在客户区里的位置（测试用）
    #[cfg(test)]
    pub(super) fn button(&self, b: Btn) -> Rect {
        let r = match b {
            Btn::Bar(a) => self.bar.as_ref().unwrap().button(a),
            Btn::Opt(i) => self.opts.as_ref().unwrap().button(i),
        };
        Rect { left: r.left - self.origin.x, top: r.top - self.origin.y, right: r.right - self.origin.x, bottom: r.bottom - self.origin.y }
    }

    /// 鼠标换了按钮：提示先收起，停 400ms 再出来
    fn set_hot(&mut self, w: Win, hot: Option<Btn>) {
        if hot != self.hot {
            (self.hot, self.tip) = (hot, None);
            w.kill_timer(TIP_TIMER);
            if hot.is_some() && self.down.is_none() {
                w.timer(TIP_TIMER, 400);
            }
            self.relayout(w);
        }
    }
}

impl Handler for Chrome {
    fn event(&mut self, w: Win, e: Event) {
        match e {
            Event::Move { p, .. } => self.set_hot(w, self.button_at(p).filter(|b| self.enabled(*b))),
            Event::Leave => self.set_hot(w, None),
            // 钉图弹着保存对话框（不能操作）时不响应
            Event::Down { p, button: Button::Left, .. } if self.pin.enabled() => {
                if let Some(b) = self.button_at(p).filter(|b| self.enabled(*b)) {
                    (self.down, self.tip) = (Some(b), None);
                    w.kill_timer(TIP_TIMER);
                    self.relayout(w);
                }
            }
            // 松开时还在按下的那个按钮上才算点击；交给钉图处理（可能结束编辑、关掉这个窗口），等这边处理完再做
            Event::Up { button: Button::Left, .. } => {
                if let Some(b) = self.down.take() {
                    self.relayout(w);
                    if self.hot == Some(b) {
                        let pin = self.pin;
                        Win::later(move || _ = pin.with(|p: &mut Pin| p.act(pin, b)));
                    }
                }
            }
            Event::CaptureLost => {
                if self.down.take().is_some() {
                    self.relayout(w);
                }
            }
            Event::Timer(TIP_TIMER) => {
                w.kill_timer(TIP_TIMER);
                self.tip = self.hot;
                self.relayout(w);
            }
            _ => {}
        }
    }

    fn paint(&mut self, _w: Win, c: &Canvas) {
        let bar_btn = |b: Option<Btn>| if let Some(Btn::Bar(a)) = b { Some(a) } else { None };
        let opt_btn = |b: Option<Btn>| if let Some(Btn::Opt(i)) = b { Some(i) } else { None };
        c.set_transform(Affine::translation(-self.origin.x as f32, -self.origin.y as f32));
        if let Some(bar) = &self.bar {
            bar.draw(c, &self.label, bar_btn(self.hot), bar_btn(self.down), self.look.tool, |a| self.enabled(Btn::Bar(a)));
        }
        if let Some(o) = &self.opts {
            o.draw(c, self.look.pen, opt_btn(self.hot), opt_btn(self.down));
        }
        if let Some((hud, x, y)) = &self.hud {
            hud.draw(c, *x, *y);
        }
        c.set_transform(Affine::identity());
    }
}
