//! 托盘面板：右键托盘图标（mac 上右键或按住 Control 点菜单栏图标）弹出的自绘面板，两端一样。
//! 截图、录屏两个大块（录制中是「停止录屏 00:12」），屏幕上的钉图、文字卡片各一行带「全部关闭」，设置、退出。
//! 点外面、Esc 关掉；方向键、Tab 换项，Enter、空格执行。只依赖 plat、ui：显示什么、点了做什么由外壳给。

use crate::plat::{self, Button, Canvas, Color, Event, Face, Handler, Key, Point, Rect, RectF, Spec, Text, Win};
use crate::ui::{self, rectf, rgb};
use std::time::Instant;

/// 面板上能点的
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cmd {
    Capture,
    Record,
    ClosePins,
    CloseCards,
    Settings,
    Quit,
}

/// 面板上显示的东西
pub struct Model {
    /// 两个快捷键的写法；没注册上的是空
    pub shot_key: String,
    pub rec_key: String,
    /// 正在录（含倒计时），开始截的时刻
    pub recording: bool,
    pub rec_since: Option<Instant>,
    pub pins: usize,
    pub cards: usize,
}

// 尺寸（DIP）：面板 300 宽，描边 2 + 内边距 12；两个大块 94 高、间距 10；分隔线 2；一行 36；实心阴影 6
const W: f32 = 300.0;
const H: f32 = 342.0;
const SHADOW: f32 = 6.0;
const IN: f32 = 14.0;
const TILE_B: f32 = 108.0;
const HR: [f32; 2] = [120.0, 244.0];
const GROUP_Y: f32 = 134.0;
const ROWS: [(Cmd, f32); 4] = [(Cmd::ClosePins, 154.0), (Cmd::CloseCards, 196.0), (Cmd::Settings, 256.0), (Cmd::Quit, 292.0)];
/// 录制中每秒刷新一次时长
const T_CLOCK: usize = 1;

struct Panel {
    m: Model,
    on: fn(Cmd),
    scale: f32,
    /// 鼠标下（或方向键选中）的、按住的
    hot: Option<Cmd>,
    down: Option<Cmd>,
}

thread_local! {
    static OPEN: std::cell::Cell<Option<Win>> = const { std::cell::Cell::new(None) };
}

/// 在 `at`（托盘图标附近，屏幕坐标）弹出面板：屏幕下半边往上弹，上半边往下弹，夹在工作区里。已经开着的先关掉。点了按钮先关面板再调 `on`
pub fn show(at: Point, m: Model, on: fn(Cmd)) {
    if let Some(old) = OPEN.take() {
        old.close();
    }
    let mon = plat::monitor_at(at);
    let (s, work) = (mon.scale(), mon.work);
    let (w, h, g) = ((W * s).round() as i32, (H * s).round() as i32, (8.0 * s).round() as i32);
    let d = (SHADOW * s).round() as i32;
    let x = (at.x + g - w).min(work.right - w - d - g).max(work.left + g);
    let y = if at.y > (work.top + work.bottom) / 2 { at.y.min(work.bottom) - g - h - d } else { at.y.max(work.top) + g };
    let y = y.min(work.bottom - h - d - g).max(work.top + g);
    let recording = m.rec_since.is_some();
    let p = Panel { m, on, scale: s, hot: None, down: None };
    let Some(win) = Win::open(Spec { kind: plat::Kind::Tray, rect: Rect { left: x, top: y, right: x + w + d, bottom: y + h + d }, title: "Jie 菜单", owner: None }, p) else { return };
    win.show();
    if recording {
        win.timer(T_CLOCK, 1000);
    }
    OPEN.set(Some(win));
}

/// 「12」→「00:12」
fn clock(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

impl Panel {
    fn px(&self, d: f32) -> f32 {
        (d * self.scale).round()
    }

    fn enabled(&self, c: Cmd) -> bool {
        match c {
            Cmd::ClosePins => self.m.pins > 0,
            Cmd::CloseCards => self.m.cards > 0,
            _ => true,
        }
    }

    /// 「全部关闭」按钮：文字 + 左右各 12，高 30，贴行的右边
    fn close_btn(&self, row: f32) -> RectF {
        let w = Text::new(Face::Ui, 12.5 * self.scale, true, "全部关闭").map_or(0.0, |l| l.width()) + self.px(24.0);
        rectf(self.px(W - IN) - w, self.px(row + 3.0), self.px(W - IN), self.px(row + 33.0))
    }

    /// 能点的地方（像素，窗口坐标）
    fn rect(&self, c: Cmd) -> RectF {
        let mid = W / 2.0;
        match c {
            Cmd::Capture => rectf(self.px(IN), self.px(IN), self.px(mid - 5.0), self.px(TILE_B)),
            Cmd::Record => rectf(self.px(mid + 5.0), self.px(IN), self.px(W - IN), self.px(TILE_B)),
            Cmd::ClosePins | Cmd::CloseCards => self.close_btn(ROWS.iter().find(|r| r.0 == c).map_or(0.0, |r| r.1)),
            _ => {
                let y = ROWS.iter().find(|r| r.0 == c).map_or(0.0, |r| r.1);
                rectf(self.px(IN), self.px(y), self.px(W - IN), self.px(y + 36.0))
            }
        }
    }

    fn at(&self, p: Point) -> Option<Cmd> {
        let (x, y) = (p.x as f32, p.y as f32);
        [Cmd::Capture, Cmd::Record, Cmd::ClosePins, Cmd::CloseCards, Cmd::Settings, Cmd::Quit].into_iter().find(|c| {
            let r = self.rect(*c);
            self.enabled(*c) && x >= r.left && x < r.right && y >= r.top && y < r.bottom
        })
    }

    /// 方向键、Tab：在能点的里面挪一格
    fn step(&mut self, w: Win, d: isize) {
        let order: Vec<Cmd> = [Cmd::Capture, Cmd::Record, Cmd::ClosePins, Cmd::CloseCards, Cmd::Settings, Cmd::Quit].into_iter().filter(|c| self.enabled(*c)).collect();
        let n = order.len() as isize;
        let i = match self.hot.and_then(|h| order.iter().position(|c| *c == h)) {
            Some(i) => (i as isize + d).rem_euclid(n),
            None if d > 0 => 0,
            None => n - 1,
        };
        self.hot = Some(order[i as usize]);
        w.invalidate_all();
    }

    /// 先关面板，再做
    fn run(&mut self, w: Win, c: Cmd) {
        w.close();
        if OPEN.get() == Some(w) {
            OPEN.set(None);
        }
        let on = self.on;
        Win::later(move || on(c));
    }

    fn draw(&self, c: &Canvas) {
        let s = self.scale;
        let (lw, px) = (ui::line_w(s), |d: f32| self.px(d));
        c.clear(Color::default());
        ui::block(c, rectf(0.0, 0.0, px(W), px(H)), 12.0, SHADOW, ui::SURF, s);
        let text = |t: &str, size: f32, bold: bool| Text::new(Face::Ui, size * s, bold, t);
        let mid_text = |l: &Text, x: f32, r: &RectF, col: u32| c.text(l, x, ((r.top + r.bottom - l.height()) / 2.0).round(), rgb(col));
        // 快捷键框：1.5 圈、高 20
        let kbd = |t: &str, x: f32, y: f32, col: u32| {
            let Some(l) = text(t, 11.5, true) else { return };
            let k = (1.5 * s).max(1.0);
            c.stroke_round(rectf(x + k / 2.0, y + k / 2.0, x + l.width() + 12.0 * s - k / 2.0, y + 20.0 * s - k / 2.0), 5.0 * s - k / 2.0, rgb(col), k);
            c.text(&l, x + 6.0 * s, y + (20.0 * s - l.height()) / 2.0, rgb(col));
        };
        // 两个大块：截图紫底白字，录屏黄底墨字；悬停往左上抬 1、阴影 4，按下沉 2、没阴影
        let rec_name = match (self.m.recording, self.m.rec_since) {
            (true, Some(t)) => format!("停止录屏 {}", clock(t.elapsed().as_secs())),
            (true, None) => "停止录屏".into(),
            _ => "录屏".into(),
        };
        let tiles = [
            (Cmd::Capture, ui::ACCENT, 0xFFFFFF, "crop", "截图", &self.m.shot_key),
            (Cmd::Record, ui::PICK, ui::INK, if self.m.recording { "stop" } else { "record" }, rec_name.as_str(), &self.m.rec_key),
        ];
        for (cmd, fill, fg, icon, name, key) in tiles {
            let r = self.rect(cmd);
            let (dy, shadow) = match (self.hot == Some(cmd), self.down == Some(cmd)) {
                (true, true) => (px(2.0), 0.0),
                (true, false) => (-s.round(), 4.0),
                _ => (0.0, 3.0),
            };
            let r = rectf(r.left + dy, r.top + dy, r.right + dy, r.bottom + dy);
            ui::block(c, r, 9.0, shadow, fill, s);
            ui::icon(c, icon, r.left + px(12.0), r.top + px(12.0), 24.0 * s, rgb(fg));
            let Some(l) = text(name, 15.0, true) else { continue };
            let bottom = r.bottom - px(12.0);
            let y = if key.is_empty() { bottom - l.height() } else { bottom - px(24.0) - l.height() };
            c.text(&l, r.left + px(12.0), y, rgb(fg));
            if !key.is_empty() {
                kbd(key, r.left + px(12.0), bottom - px(20.0), fg);
            }
        }
        for y in HR {
            c.fill_rect(rectf(lw, px(y), px(W) - lw, px(y) + lw), rgb(ui::INK));
        }
        if let Some(l) = text("屏幕上", 11.5, true) {
            c.text(&l, px(IN + 8.0), px(GROUP_Y), rgb(ui::MUTED));
        }
        // 钉图、文字卡片：名字、柠檬黄的数量、「全部关闭」；一个都没有时整行置灰
        for (cmd, icon, name, n) in [(Cmd::ClosePins, "pin", "钉图", self.m.pins), (Cmd::CloseCards, "ocr", "文字卡片", self.m.cards)] {
            let y = ROWS.iter().find(|r| r.0 == cmd).map_or(0.0, |r| r.1);
            let row = rectf(px(IN), px(y), px(W - IN), px(y + 36.0));
            let fg = if n > 0 { ui::INK } else { ui::FAINT };
            ui::icon(c, icon, px(IN + 8.0), (row.top + row.bottom) / 2.0 - 10.0 * s, 20.0 * s, rgb(fg));
            let Some(l) = text(name, 13.0, true) else { continue };
            mid_text(&l, px(IN + 38.0), &row, fg);
            if n > 0
                && let Some(t) = text(&n.to_string(), 11.5, true)
            {
                let x = px(IN + 38.0) + l.width() + px(8.0);
                let cw = (t.width() + 12.0 * s).max(22.0 * s);
                let chip = rectf(x, (row.top + row.bottom) / 2.0 - 10.0 * s, x + cw, (row.top + row.bottom) / 2.0 + 10.0 * s);
                c.fill_round(chip, 6.0 * s, rgb(ui::PICK));
                let k = (1.5 * s).max(1.0);
                c.stroke_round(rectf(chip.left + k / 2.0, chip.top + k / 2.0, chip.right - k / 2.0, chip.bottom - k / 2.0), 6.0 * s - k / 2.0, rgb(ui::INK), k);
                c.text(&t, x + (cw - t.width()) / 2.0, (chip.top + chip.bottom - t.height()) / 2.0, rgb(ui::INK));
            }
            let b = self.rect(cmd);
            let face = if n > 0 {
                ui::secondary(c, b, ui::state(true, false, self.hot == Some(cmd), self.down == Some(cmd)), s)
            } else {
                ui::block(c, b, 7.0, 0.0, ui::SURF, s);
                b
            };
            if let Some(t) = text("全部关闭", 12.5, true) {
                c.text(&t, (face.left + face.right - t.width()) / 2.0, (face.top + face.bottom - t.height()) / 2.0, rgb(fg));
            }
        }
        // 设置、退出：一行一个，悬停浅紫
        let quit = if cfg!(target_os = "macos") { "退出 Jie" } else { "退出" };
        for (cmd, icon, name) in [(Cmd::Settings, "sliders", "设置…"), (Cmd::Quit, "power", quit)] {
            let r = self.rect(cmd);
            if self.hot == Some(cmd) {
                c.fill_round(r, 7.0 * s, rgb(if self.down == Some(cmd) { ui::PRESS } else { ui::HOVER }));
            }
            ui::icon(c, icon, r.left + px(8.0), (r.top + r.bottom) / 2.0 - 10.0 * s, 20.0 * s, rgb(ui::INK));
            if let Some(l) = text(name, 13.0, true) {
                mid_text(&l, r.left + px(38.0), &r, ui::INK);
            }
        }
    }
}

impl Handler for Panel {
    fn event(&mut self, w: Win, e: Event) {
        match e {
            Event::Move { p, .. } => {
                let hot = self.at(p);
                if hot != self.hot {
                    self.hot = hot;
                    w.invalidate_all();
                }
            }
            Event::Leave => {
                if self.down.is_none() && self.hot.take().is_some() {
                    w.invalidate_all();
                }
            }
            Event::Down { p, button: Button::Left, .. } => {
                self.down = self.at(p);
                w.invalidate_all();
            }
            Event::Up { button: Button::Left, .. } => {
                let p = w.cursor_pos();
                match self.down.take() {
                    Some(c) if self.at(p) == Some(c) => self.run(w, c),
                    _ => w.invalidate_all(),
                }
            }
            Event::CaptureLost => self.down = None,
            Event::Key { key, mods } => match key {
                Key::Esc => w.close(),
                Key::Up | Key::Left => self.step(w, -1),
                Key::Down | Key::Right => self.step(w, 1),
                Key::Tab => self.step(w, if mods.shift { -1 } else { 1 }),
                Key::Enter | Key::Space => {
                    if let Some(c) = self.hot {
                        self.run(w, c);
                    }
                }
                _ => {}
            },
            // 点到面板外面（别的窗口拿到前台）就关
            Event::Deactivate => w.close(),
            Event::Timer(T_CLOCK) => w.invalidate_all(),
            _ => {}
        }
    }

    fn paint(&mut self, _w: Win, c: &Canvas) {
        self.draw(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn clock_text() {
        assert_eq!((clock(12).as_str(), clock(605).as_str()), ("00:12", "10:05"));
    }

    /// 弹出真的面板：画成 target/test-tmp/tray/*.png（平时、录制中），方向键跳过置灰的「全部关闭」，Enter 执行后面板关掉。
    /// 会在屏幕上出现一下，平时不跑：cargo test tray_panel -- --ignored
    #[test]
    #[ignore]
    fn tray_panel() {
        static HIT: AtomicUsize = AtomicUsize::new(usize::MAX);
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("tray");
        std::fs::create_dir_all(&dir).unwrap();
        let mon = plat::monitor_at(Point::default());
        let model = |recording: bool| Model {
            shot_key: "Alt+Shift+A".into(),
            rec_key: "Alt+Shift+S".into(),
            recording,
            rec_since: recording.then(|| Instant::now() - std::time::Duration::from_secs(12)),
            pins: 3,
            cards: 0,
        };
        let snap = |w: Win, name: &str| {
            let r = w.rect();
            let b = plat::Bitmap::new(r.right - r.left, r.bottom - r.top).unwrap();
            w.peek::<Panel>().paint(w, &Canvas::new(&b, b.full(), true));
            let mut out = plat::Bitmap::new(b.w, b.h).unwrap();
            for (o, p) in out.pixels_mut().chunks_mut(4).zip(b.pixels().chunks(4)) {
                for k in 0..3 {
                    o[k] = (p[k] as u32 + 0xC0 * (255 - p[3] as u32) / 255) as u8;
                }
                o[3] = 255;
            }
            std::fs::write(dir.join(name), plat::encode(&out.crop(&out.full()), false).unwrap()).unwrap();
        };
        let at = Point { x: mon.work.right - 100, y: mon.work.bottom + 10 };
        let t0 = Instant::now();
        show(at, model(true), |c| HIT.store(c as usize, Ordering::Relaxed));
        println!("弹出面板（建窗口 + 画好）{:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
        let w = OPEN.get().expect("面板没出来");
        // show 完当场就画好了，不等鼠标挪上去
        #[cfg(windows)]
        assert!(w.drawn(), "面板显示了但没画");
        assert!(w.rect().bottom <= mon.work.bottom && w.rect().right <= mon.work.right);
        snap(w, "recording.png");
        w.close();

        let t1 = Instant::now();
        show(at, model(false), |c| HIT.store(c as usize, Ordering::Relaxed));
        println!("第二次弹出面板 {:.1} ms", t1.elapsed().as_secs_f64() * 1000.0);
        let w = OPEN.get().unwrap();
        let key = |k: Key| w.send(Event::Key { key: k, mods: plat::Mods::default() });
        // 截图、录屏、钉图的「全部关闭」，文字卡片那行置灰跳过，到设置
        for _ in 0..4 {
            key(Key::Down);
        }
        assert_eq!(w.peek::<Panel>().hot, Some(Cmd::Settings));
        snap(w, "normal.png");
        key(Key::Enter);
        let t0 = Instant::now();
        while HIT.load(Ordering::Relaxed) == usize::MAX && t0.elapsed() < std::time::Duration::from_secs(2) {
            plat::pump();
        }
        assert_eq!(HIT.load(Ordering::Relaxed), Cmd::Settings as usize);
        assert!(OPEN.get().is_none());
    }
}
