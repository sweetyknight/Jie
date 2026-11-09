//! 结果胶囊：截图所在显示器工作区底部居中，离底边 48；白底墨色描边、4,4 实心阴影；透明窗口，不抢焦点、鼠标穿透。
//! 成功停 1.5 秒、失败 4 秒，然后 200ms 淡出；处理中一直转圈，直到被下一条替换。

use crate::plat::{self, Canvas, Color, Event, Face, Handler, Monitor, Rect, Spec, Text, Win};
use crate::ui::{self, rectf, rgb};
use std::cell::Cell;

const T_DISMISS: usize = 1;
const T_FADE: usize = 2;
const T_SPIN: usize = 3;
/// 淡出和转圈的帧间隔
const FRAME_MS: u32 = 16;
/// 实心阴影 DIP
const SHADOW: f32 = 4.0;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Ok,
    Err,
    Busy,
}

struct Toast {
    kind: Kind,
    text: Text,
    scale: f32,
    angle: f32,
    alpha: f32,
    action: Option<fn()>,
}

thread_local! {
    /// 正显示的那条
    static TOAST: Cell<Option<Win>> = const { Cell::new(None) };
}

/// 显示一条胶囊，替换掉正在显示的那条
pub fn show(mon: &Monitor, kind: Kind, text: &str) { show_action(mon,kind,text,None); }

pub fn show_action(mon: &Monitor, kind: Kind, text: &str, action: Option<fn()>) {
    if let Some(old) = TOAST.take() {
        old.close();
    }
    let s = mon.scale();
    let Some(text) = Text::wrapped(Face::Ui, 13.0 * s, false, text, ((mon.work.right-mon.work.left) as f32 - 100.0*s).min(600.0*s).max(100.0*s), None) else { return };
    // 高 40；左内边距 11、右 16；状态方块 18，间距 10；窗口再加上阴影
    let d = (SHADOW * s).round();
    let h = ((40.0 * s).max(text.height() + 18.0 * s) + d).ceil() as i32;
    let w = (11.0 * s + 18.0 * s + 10.0 * s + text.width() + 16.0 * s + d).ceil() as i32;
    let work = mon.work;
    let (x, y) = ((work.left + work.right - w) / 2, work.bottom - (48.0 * s).round() as i32 - h);
    let t = Toast { kind, text, scale: s, angle: 0.0, alpha: 1.0, action };
    let spec = Spec { kind: if action.is_some() {plat::Kind::PinBar} else {plat::Kind::Toast}, rect: Rect { left: x, top: y, right: x + w, bottom: y + h }, title: "Jie", owner: None };
    let Some(win) = Win::open(spec, t) else { return };
    win.show();
    match kind {
        Kind::Ok => win.timer(T_DISMISS, 1500),
        Kind::Err => win.timer(T_DISMISS, 4000),
        Kind::Busy => win.timer(T_SPIN, FRAME_MS),
    }
    TOAST.set(Some(win));
}

impl Handler for Toast {
    fn event(&mut self, w: Win, e: Event) {
        if matches!(e, Event::Down {button:plat::Button::Left,..}) && let Some(action)=self.action { w.close(); Win::later(action); return; }
        let Event::Timer(id) = e else { return };
        match id {
            T_DISMISS => {
                w.kill_timer(T_DISMISS);
                w.timer(T_FADE, FRAME_MS);
            }
            T_FADE => {
                self.alpha -= FRAME_MS as f32 / 200.0;
                if self.alpha > 0.0 {
                    w.set_alpha(self.alpha);
                } else {
                    w.close();
                    if TOAST.get() == Some(w) {
                        TOAST.set(None);
                    }
                }
            }
            T_SPIN => {
                // 0.8 秒一圈
                self.angle = (self.angle + 360.0 * FRAME_MS as f32 / 800.0) % 360.0;
                w.invalidate_all();
            }
            _ => {}
        }
    }

    fn paint(&mut self, _w: Win, c: &Canvas) {
        let s = self.scale;
        let a = c.area();
        let (w, h) = ((a.right - a.left) as f32, (a.bottom - a.top) as f32);
        let d = (SHADOW * s).round();
        let (w, h) = (w - d, h - d);
        c.clear(Color::default());
        ui::block(c, rectf(0.0, 0.0, w, h), 10.0, SHADOW, ui::SURF, s);
        let (ix, iy) = (11.0 * s, (h - 18.0 * s) / 2.0);
        match self.kind {
            Kind::Ok | Kind::Err => {
                let (bg, mark) = if self.kind == Kind::Ok { (ui::OK, "ok_mark") } else { (ui::ERR, "err_mark") };
                ui::block(c, rectf(ix, iy, ix + 18.0 * s, iy + 18.0 * s), 5.0, 0.0, bg, s);
                // 状态图标的路径画在 16 网格上，按 20 网格的接口传 20 × s 就是 1 单位 = 1 DIP
                ui::icon(c, mark, ix + 1.0 * s, iy + 1.0 * s, 20.0 * s, rgb(ui::INK));
            }
            Kind::Busy => ui::spinner(c, ix + 1.0 * s, iy + 1.0 * s, s, self.angle, rgb(ui::HOVER), rgb(ui::ACCENT)),
        }
        c.text(&self.text, ix + 28.0 * s, (h - self.text.height()) / 2.0, rgb(ui::INK));
    }
}
