//! 窗口：Win32 窗口过程把消息翻成 Event 交给 Handler。
//! 普通窗口重画时，按要重画的区域一块块画进离屏小位图再贴上去；透明窗口（操作栏、胶囊）整张画好用 UpdateLayeredWindow。
//!
//! 重入：处理一个窗口的事时（Handler 被借着），同一个窗口又来的事先排队，处理完再补发；
//! 关窗口、弹菜单、对话框这些会转消息循环的事推迟到这次处理完再做（`Win::later`）。

use super::field::{self, Fields, WM_FIELDKEY};
use super::{Bitmap, Canvas, menu_draw, menu_measure, popup_menu};
use crate::plat::{Button, Cursor, Edge, Event, FieldKind, Handler, Hit, Key, Kind, MenuItem, Monitor, Mods, Point, Rect, SaveAs, Spec, ring_pixels};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::PathBuf;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{EM_SETSEL, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::Ime::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

/// 工作线程交回窗口的活：lParam 是 Box<Job>
const WM_RUN: u32 = WM_APP + 0x100;
/// 遮罩重画后补刷一遍的定时器（见 WM_PAINT）；和 Handler 自己的定时器编号错开
const ECHO_TIMER: usize = 0x4A49;

type Job = Box<dyn FnOnce(&mut dyn Any, Win) + Send>;

/// 一个窗口：可以随便拷、跨线程传；窗口关了以后再用什么都不做（默认值是一个不存在的窗口）
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Win(isize);

struct State {
    kind: Kind,
    handler: RefCell<Box<dyn Handler>>,
    /// 按着左键：鼠标归这个窗口
    captured: Cell<bool>,
    /// 已经要过 WM_MOUSELEAVE
    tracking: Cell<bool>,
    /// 正在输入文字：插入光标（内容区坐标），输入法的候选窗跟着它
    ime: Cell<Option<Rect>>,
    /// 透明窗口：上次画好的画面（调整体透明度时直接用）和整体透明度
    layer: RefCell<Option<Bitmap>>,
    alpha: Cell<f32>,
    /// 文字卡片的大小由它自己定：系统的拖动循环会按开始拖时的大小摆窗口，要拦下来
    size: Cell<(i32, i32)>,
    /// 设置窗口的系统输入框
    fields: RefCell<Fields>,
    /// 遮罩：画过、等着补刷一遍的范围
    echo: Cell<Option<Rect>>,
}

thread_local! {
    /// 这个线程上开着的窗口
    static WINDOWS: RefCell<Vec<(isize, Kind)>> = const { RefCell::new(Vec::new()) };
    /// 正在处理事件的层数（各个窗口加起来）；为 0 时才做推迟的事
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    /// 窗口忙时到的事，处理完再补发
    static PENDING: RefCell<VecDeque<(isize, Event)>> = const { RefCell::new(VecDeque::new()) };
    /// 推迟到这次处理完再做的事
    static LATER: RefCell<VecDeque<Box<dyn FnOnce()>>> = const { RefCell::new(VecDeque::new()) };
    /// 要重画的透明窗口
    static DIRTY: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
    /// 马赛克笔刷的圆圈光标：(直径, 缩放 × 96) 和光标；换了大小才重做
    static RING: Cell<Option<((i32, i32), HCURSOR)>> = const { Cell::new(None) };
}

fn class(kind: Kind) -> PCWSTR {
    match kind {
        Kind::Overlay => w!("Jie.Overlay"),
        Kind::Pin => w!("Jie.Pin"),
        Kind::PinBar => w!("Jie.PinBar"),
        Kind::Card => w!("Jie.Card"),
        Kind::Toast => w!("Jie.Toast"),
        Kind::Settings => w!("Jie.Settings"),
        Kind::RecFrame => w!("Jie.RecFrame"),
        Kind::RecBar => w!("Jie.RecBar"),
        // 不能叫 Jie.Tray：那是外壳隐藏主窗口的类（app.rs），重名时面板用了主窗口的过程，一关面板就退出程序
        Kind::Tray => w!("Jie.TrayPanel"),
    }
}

/// 透明窗口：整张画好用 UpdateLayeredWindow 贴，没有 WM_PAINT。文字卡片、托盘面板自己画圆角和实心阴影，也是透明窗口
fn layered(kind: Kind) -> bool {
    matches!(kind, Kind::PinBar | Kind::Toast | Kind::RecFrame | Kind::RecBar | Kind::Card | Kind::Tray)
}

fn styles(kind: Kind) -> (WINDOW_EX_STYLE, WINDOW_STYLE) {
    let pop = WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
    match kind {
        Kind::Overlay | Kind::Pin | Kind::Card | Kind::Tray => (pop | WS_EX_LAYERED, WS_POPUP),
        Kind::PinBar | Kind::RecBar => (pop | WS_EX_LAYERED | WS_EX_NOACTIVATE, WS_POPUP),
        Kind::Toast | Kind::RecFrame => (pop | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE, WS_POPUP),
        // 设计稿的标题栏只有关闭按钮
        Kind::Settings => (WINDOW_EX_STYLE::default(), WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN),
    }
}

fn register(kind: Kind) {
    thread_local! {
        static DONE: RefCell<Vec<Kind>> = const { RefCell::new(Vec::new()) };
    }
    if DONE.with_borrow(|d| d.contains(&kind)) {
        return;
    }
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default().into();
        let mut style = CS_DBLCLKS;
        if kind == Kind::Pin {
            // 系统阴影
            style |= CS_DROPSHADOW;
        }
        let (icon, small) = if kind == Kind::Settings { (super::app_icon(GetSystemMetrics(SM_CXICON)), super::app_icon(GetSystemMetrics(SM_CXSMICON))) } else { Default::default() };
        RegisterClassExW(&WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hIcon: icon,
            hIconSm: small,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class(kind),
            ..Default::default()
        });
    }
    DONE.with_borrow_mut(|d| d.push(kind));
}

fn state<'a>(hwnd: HWND) -> Option<&'a State> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const State;
    // 窗口还在、还没进 WM_NCDESTROY
    (!p.is_null()).then(|| unsafe { &*p })
}

fn dwm<T>(hwnd: HWND, attr: DWMWINDOWATTRIBUTE, v: T) {
    let _ = unsafe { DwmSetWindowAttribute(hwnd, attr, &v as *const T as _, size_of::<T>() as u32) };
}

impl Win {
    pub(crate) fn hwnd(self) -> HWND {
        HWND(self.0 as _)
    }

    /// 建窗口（先不显示，调 `show`）；`h` 收这个窗口的事
    pub fn open(spec: Spec, h: impl Handler) -> Option<Win> {
        let kind = spec.kind;
        register(kind);
        let (ex, style) = styles(kind);
        let mut r = spec.rect;
        if kind == Kind::Settings {
            let dpi = super::monitor_at(Point { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 }).dpi;
            let _ = unsafe { AdjustWindowRectExForDpi(&mut r, style, false, ex, dpi) };
        }
        let st = Box::new(State {
            kind,
            handler: RefCell::new(Box::new(h)),
            captured: Cell::new(false),
            tracking: Cell::new(false),
            ime: Cell::new(None),
            layer: RefCell::new(None),
            alpha: Cell::new(1.0),
            size: Cell::new((r.right - r.left, r.bottom - r.top)),
            fields: RefCell::new(Fields::default()),
            echo: Cell::new(None),
        });
        let ptr = Box::into_raw(st);
        let hwnd = unsafe {
            let hinst = GetModuleHandleW(None).ok()?.into();
            let owner = spec.owner.map(Win::hwnd);
            CreateWindowExW(ex, class(kind), &HSTRING::from(spec.title), style, r.left, r.top, r.right - r.left, r.bottom - r.top, owner, None, Some(hinst), Some(ptr as _))
        };
        let Ok(hwnd) = hwnd else {
            drop(unsafe { Box::from_raw(ptr) });
            return None;
        };
        // 关掉系统的窗口出现动画，否则会淡入
        if kind != Kind::Settings {
            dwm(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, TRUE);
        }
        // Win10 不认圆角、边框颜色这几个属性
        match kind {
            // 钉图不要圆角
            Kind::Pin => {
                dwm(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND);
                let _ = unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA) };
            }
            // 铺满整屏的普通窗口最后一次重画常常不上屏（见 WM_PAINT 的补刷）；分层后少见很多
            Kind::Overlay => {
                let _ = unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA) };
            }
            // 标题栏奶油色、边框墨色，接上自绘内容（COLORREF 是 0xBBGGRR）；用户开了「在标题栏显示主题色」时也不出现蓝色
            Kind::Settings => {
                dwm(hwnd, DWMWA_CAPTION_COLOR, COLORREF(0xDCF4FF));
                dwm(hwnd, DWMWA_BORDER_COLOR, COLORREF(0x171315));
            }
            _ => {}
        }
        WINDOWS.with_borrow_mut(|v| v.push((hwnd.0 as isize, kind)));
        Some(Win(hwnd.0 as isize))
    }

    /// 显示出来：遮罩、钉图、卡片、设置拿到前台；操作栏、胶囊不抢焦点。遮罩当场画完第一帧
    pub fn show(self) {
        let Some(st) = state(self.hwnd()) else { return };
        let h = self.hwnd();
        unsafe {
            match st.kind {
                Kind::PinBar | Kind::Toast | Kind::RecFrame | Kind::RecBar => {
                    render(h);
                    let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                }
                kind => {
                    // 透明窗口（托盘面板、文字卡片）先画好再显示：没画过的透明窗口看不见，要等下一次重画（比如鼠标挪动）才出来
                    if layered(kind) {
                        render(h);
                    }
                    let _ = ShowWindow(h, SW_SHOW);
                    if !SetForegroundWindow(h).as_bool() && kind == Kind::Overlay {
                        eprintln!("遮罩没拿到前台");
                    }
                    if kind == Kind::Overlay {
                        let _ = UpdateWindow(h);
                    }
                }
            }
        }
    }

    pub fn hide(self) {
        let _ = unsafe { ShowWindow(self.hwnd(), SW_HIDE) };
    }

    /// 不录进录屏，别人截屏、远程桌面照样看得到：显示前记下它底下的画面，录屏时每帧盖回去（见 record.rs）。
    /// 要在 `show` 之前调，窗口之后别挪；截不下来时返回 false
    pub fn hide_from_capture(self) -> bool {
        let mut r = Rect::default();
        unsafe { GetWindowRect(self.hwnd(), &mut r) }.is_ok() && super::record::cover(self.0, r)
    }

    /// 这个线程上开着的某种窗口
    pub fn all(kind: Kind) -> Vec<Win> {
        WINDOWS.with_borrow(|v| v.iter().filter(|w| w.1 == kind).map(|w| Win(w.0)).collect())
    }

    /// 透明窗口画过没有
    #[cfg(test)]
    pub fn drawn(self) -> bool {
        state(self.hwnd()).is_some_and(|s| s.layer.borrow().is_some())
    }

    #[cfg(test)]
    pub fn alive(self) -> bool {
        WINDOWS.with_borrow(|v| v.iter().any(|w| w.0 == self.0))
    }

    /// 拿到这个窗口的 Handler（得是 H 类型）；窗口关了、或者正在处理事件时为 None
    pub fn with<H: Handler, R>(self, f: impl FnOnce(&mut H) -> R) -> Option<R> {
        let r = call(self.hwnd(), |h, _| (h as &mut dyn Any).downcast_mut::<H>().map(f)).flatten();
        settle();
        r
    }

    /// 当作系统发来的事交给 Handler（测试、模拟操作用）
    #[cfg(test)]
    pub fn send(self, e: Event) {
        deliver(self.hwnd(), e);
    }

    /// 测试里直接看 Handler 的状态：不经过借用检查，只在两次事件之间用
    #[cfg(test)]
    pub fn peek<'a, H: Handler>(self) -> &'a mut H {
        let st = state(self.hwnd()).expect("窗口没了");
        let h: &mut dyn Handler = unsafe { &mut **st.handler.as_ptr() };
        (h as &mut dyn Any).downcast_mut::<H>().expect("不是这种窗口")
    }

    /// 工作线程把结果交回窗口：到界面线程上调 f；窗口已经关了就丢掉
    pub fn post<H: Handler>(self, f: impl FnOnce(&mut H, Win) + Send + 'static) {
        let job: Job = Box::new(move |h, w| {
            if let Some(h) = h.downcast_mut::<H>() {
                f(h, w);
            }
        });
        let p = Box::into_raw(Box::new(job));
        unsafe {
            if PostMessageW(Some(self.hwnd()), WM_RUN, WPARAM(0), LPARAM(p as isize)).is_err() {
                drop(Box::from_raw(p));
            }
        }
    }

    /// 等这次处理完（没在处理事件就是现在）再做 f：关窗口、弹菜单、对话框、建别的窗口都这样做
    pub fn later(f: impl FnOnce() + 'static) {
        if DEPTH.get() > 0 {
            LATER.with_borrow_mut(|q| q.push_back(Box::new(f)));
        } else {
            f();
        }
    }

    pub fn close(self) {
        Self::later(move || {
            let _ = unsafe { DestroyWindow(self.hwnd()) };
        });
    }

    /// 重画内容区的 r 这块
    pub fn invalidate(self, r: Rect) {
        if self.is_layered() {
            mark_dirty(self.hwnd());
        } else {
            let _ = unsafe { InvalidateRect(Some(self.hwnd()), Some(&r), false) };
        }
    }

    /// 攒着的重画现在就画掉（测试、量耗时用）
    #[cfg(test)]
    pub fn repaint(self) {
        let _ = unsafe { UpdateWindow(self.hwnd()) };
    }

    pub fn invalidate_all(self) {
        if self.is_layered() {
            mark_dirty(self.hwnd());
        } else {
            let _ = unsafe { InvalidateRect(Some(self.hwnd()), None, false) };
        }
    }

    fn is_layered(self) -> bool {
        state(self.hwnd()).is_some_and(|s| layered(s.kind))
    }

    /// 窗口在桌面上的位置（含标题栏和边框）
    pub fn rect(self) -> Rect {
        let mut r = Rect::default();
        let _ = unsafe { GetWindowRect(self.hwnd(), &mut r) };
        r
    }

    pub fn set_rect(self, r: Rect) {
        let h = self.hwnd();
        if let Some(st) = state(h) {
            st.size.set((r.right - r.left, r.bottom - r.top));
            if layered(st.kind) {
                mark_dirty(h);
            }
        }
        let _ = unsafe { SetWindowPos(h, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    pub fn move_to(self, x: i32, y: i32) {
        let _ = unsafe { SetWindowPos(self.hwnd(), None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    /// 整体不透明度 0–1
    pub fn set_alpha(self, a: f32) {
        let h = self.hwnd();
        let Some(st) = state(h) else { return };
        st.alpha.set(a);
        unsafe {
            if layered(st.kind) {
                let _ = UpdateLayeredWindow(h, None, None, None, None, None, COLORREF(0), Some(&blend(a)), ULW_ALPHA);
            } else {
                let _ = SetLayeredWindowAttributes(h, COLORREF(0), (a * 255.0).round() as u8, LWA_ALPHA);
            }
        }
    }

    /// 每 ms 毫秒来一次 `Event::Timer(id)`，直到 `kill_timer`
    pub fn timer(self, id: usize, ms: u32) {
        unsafe { SetTimer(Some(self.hwnd()), id, ms, None) };
    }

    pub fn kill_timer(self, id: usize) {
        let _ = unsafe { KillTimer(Some(self.hwnd()), id) };
    }

    /// 鼠标在内容区里的位置
    pub fn cursor_pos(self) -> Point {
        let mut p = super::cursor_pos();
        let _ = unsafe { ScreenToClient(self.hwnd(), &mut p) };
        p
    }

    /// 窗口所在显示器的缩放比例
    pub fn scale(self) -> f32 {
        unsafe { GetDpiForWindow(self.hwnd()) as f32 / 96.0 }
    }

    /// 窗口中心所在的显示器
    pub fn monitor(self) -> Monitor {
        let r = self.rect();
        super::monitor_at(Point { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 })
    }

    /// 提到前台（最小化了就先还原）
    pub fn foreground(self) {
        unsafe {
            if IsIconic(self.hwnd()).as_bool() {
                let _ = ShowWindow(self.hwnd(), SW_RESTORE);
            }
            let _ = SetForegroundWindow(self.hwnd());
        }
    }

    /// 能不能接受输入（弹着模态对话框时不能）
    pub fn enabled(self) -> bool {
        unsafe { IsWindowEnabled(self.hwnd()) }.as_bool()
    }

    /// 打开或关掉文字输入：`caret` 是插入光标（内容区坐标），输入法的候选窗跟着它；None 时输入法消息交给系统
    pub fn ime(self, caret: Option<Rect>) {
        if let Some(st) = state(self.hwnd()) {
            st.ime.set(caret);
        }
    }

    /// 在内容区的 `at` 弹菜单（等这次处理完），选完把选中的 id（没选为 0）交给 `then`
    pub fn menu(self, items: &[MenuItem], at: Point, then: impl FnOnce(usize) + 'static) {
        let owned: Vec<_> = items.iter().map(|i| (i.id, i.text.to_string(), i.key.to_string(), i.enabled, i.default, i.checked)).collect();
        let mut at = at;
        let _ = unsafe { ClientToScreen(self.hwnd(), &mut at) };
        Self::later(move || {
            let items: Vec<MenuItem> = owned.iter().map(|(id, t, k, e, d, c)| MenuItem { id: *id, text: t, key: k, enabled: *e, default: *d, checked: *c }).collect();
            then(popup_menu(self.hwnd(), &items, at.x, at.y));
        });
    }

    /// 系统保存对话框（等这次处理完；期间这个窗口照常显示、不能操作）：文件类型 `kind`（PNG、JPEG 时两种都能选），
    /// 从 `dir` 开始、默认文件名 `name`（不带扩展名）；取消时交给 `then` 的是 None
    pub fn ask_save(self, dir: String, name: String, kind: SaveAs, then: impl FnOnce(Option<PathBuf>) + 'static) {
        Self::later(move || then(super::ask_save_path(self.hwnd(), &dir, &name, kind)));
    }

    /// 系统选择文件夹对话框（等这次处理完），从 `start` 开始
    pub fn ask_folder(self, start: String, then: impl FnOnce(Option<String>) + 'static) {
        Self::later(move || then(super::ask_folder(self.hwnd(), &start)));
    }
}

impl Win {
    fn fields<R>(self, f: impl FnOnce(&mut Fields) -> R) -> Option<R> {
        state(self.hwnd()).map(|st| f(&mut st.fields.borrow_mut()))
    }

    fn field_hwnd(self, id: usize) -> Option<HWND> {
        self.fields(|f| f.get(id)).flatten()
    }

    /// 建一个系统输入框（先不显示）；`id` 是调用方自己编的号，`cue` 是空着时的灰字提示
    pub fn field(self, id: usize, kind: FieldKind, cue: &str) {
        self.fields(|f| f.add(self.hwnd(), id, kind, cue));
    }

    /// 输入框放进自绘外框 `frame`（内容区坐标）里，字号按 `scale`
    pub fn place_field(self, id: usize, frame: Rect, scale: f32) {
        self.fields(|f| f.place(self.hwnd(), id, frame, scale));
    }

    /// 显示或藏起输入框；藏起的正拿着焦点时焦点回到窗口
    pub fn show_field(self, id: usize, on: bool) {
        let Some(h) = self.field_hwnd(id) else { return };
        unsafe {
            if !on && GetFocus() == h {
                let _ = SetFocus(Some(self.hwnd()));
            }
            let _ = ShowWindow(h, if on { SW_SHOWNA } else { SW_HIDE });
        }
    }

    pub fn field_text(self, id: usize) -> String {
        self.field_hwnd(id).map(field::text).unwrap_or_default()
    }

    pub fn set_field_text(self, id: usize, t: &str) {
        if let Some(h) = self.field_hwnd(id) {
            field::set_text(h, t);
        }
    }

    /// 快捷键框里的组合：(MOD_* 修饰键位, 虚拟键码)
    pub fn hotkey_field(self, id: usize) -> (u32, u32) {
        self.field_hwnd(id).map_or((0, 0), field::hotkey)
    }

    pub fn set_hotkey_field(self, id: usize, mods: u32, vk: u32) {
        if let Some(h) = self.field_hwnd(id) {
            field::set_hotkey(h, mods, vk);
        }
    }

    /// 键盘焦点给输入框（全选里面的字，快捷键框除外），None 给窗口本身
    pub fn focus_field(self, id: Option<usize>) {
        unsafe {
            match id.and_then(|id| self.field_hwnd(id)) {
                Some(h) => {
                    let _ = SetFocus(Some(h));
                    SendMessageW(h, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                }
                None => _ = SetFocus(Some(self.hwnd())),
            }
        }
    }

    /// 焦点在哪个输入框上；在窗口本身或者别处时为 None
    pub fn focused_field(self) -> Option<usize> {
        let f = unsafe { GetFocus() };
        self.fields(|fs| fs.id_of(f)).flatten()
    }

    /// 焦点在窗口本身（不在输入框上）
    pub fn focused(self) -> bool {
        unsafe { GetFocus() == self.hwnd() }
    }
}

/// 处理这个线程上的一条消息，没有消息时返回 false（测试里一步步等工作线程交回结果用）
#[cfg(test)]
pub fn pump() -> bool {
    let mut msg = MSG::default();
    unsafe {
        if !PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            return false;
        }
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    true
}

/// 借着 Handler 调 f；窗口没了或者正忙时为 None
fn call<R>(hwnd: HWND, f: impl FnOnce(&mut dyn Handler, Win) -> R) -> Option<R> {
    let st = state(hwnd)?;
    let mut h = st.handler.try_borrow_mut().ok()?;
    DEPTH.set(DEPTH.get() + 1);
    let r = f(&mut **h, Win(hwnd.0 as isize));
    DEPTH.set(DEPTH.get() - 1);
    Some(r)
}

/// 把事交给窗口；窗口正忙就排队，等它忙完再补发
fn deliver(hwnd: HWND, e: Event) {
    let Some(st) = state(hwnd) else { return };
    if st.handler.try_borrow().is_err() {
        PENDING.with_borrow_mut(|q| q.push_back((hwnd.0 as isize, e)));
        return;
    }
    call(hwnd, |h, w| h.event(w, e));
    settle();
}

/// 所有窗口都处理完时：补发排队的事，做推迟的事，重画透明窗口
fn settle() {
    if DEPTH.get() > 0 {
        return;
    }
    loop {
        if let Some((h, e)) = PENDING.with_borrow_mut(|q| q.pop_front()) {
            call(HWND(h as _), |hd, w| hd.event(w, e));
            continue;
        }
        if let Some(job) = LATER.with_borrow_mut(|q| q.pop_front()) {
            job();
            continue;
        }
        let Some(h) = DIRTY.with_borrow_mut(|d| d.pop()) else { break };
        render(HWND(h as _));
    }
}

fn mark_dirty(hwnd: HWND) {
    DIRTY.with_borrow_mut(|d| {
        if !d.contains(&(hwnd.0 as isize)) {
            d.push(hwnd.0 as isize);
        }
    });
    settle();
}

fn blend(a: f32) -> BLENDFUNCTION {
    BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: (a * 255.0) as u8, AlphaFormat: AC_SRC_ALPHA as u8 }
}

/// 透明窗口：整张画进预乘 alpha 的位图，连同位置大小一起交给系统
fn render(hwnd: HWND) {
    let Some(st) = state(hwnd) else { return };
    let (w, h) = st.size.get();
    if w <= 0 || h <= 0 {
        return;
    }
    let mut layer = st.layer.borrow_mut();
    if layer.as_ref().is_none_or(|b| (b.w, b.h) != (w, h)) {
        *layer = Bitmap::new(w, h);
    }
    let Some(bmp) = layer.as_ref() else { return };
    call(hwnd, |hd, win| {
        let c = Canvas::new(bmp, bmp.full(), true);
        c.clear(Default::default());
        hd.paint(win, &c);
    });
    let mut r = Rect::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut r);
        let _ = GdiFlush();
        let pos = POINT { x: r.left, y: r.top };
        let size = SIZE { cx: w, cy: h };
        let _ = UpdateLayeredWindow(hwnd, None, Some(&pos), Some(&size), Some(bmp.dc), Some(&POINT::default()), COLORREF(0), Some(&blend(st.alpha.get())), ULW_ALPHA);
    }
}

/// 普通窗口重画：要重画的区域一块块画进离屏小位图（一块最多 1M 像素）再贴到窗口，不闪
unsafe fn paint(hwnd: HWND) {
    unsafe {
        let rgn = CreateRectRgn(0, 0, 0, 0);
        GetUpdateRgn(hwnd, rgn, false);
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rects = region_rects(rgn);
        let _ = DeleteObject(rgn.into());
        // 几块挨得近（外接矩形不到它们总面积的 2 倍）就合成一块画，省得每块都把整个画面过一遍
        let area = |r: &Rect| (r.right - r.left) as i64 * (r.bottom - r.top) as i64;
        if let Some(b) = rects.iter().copied().reduce(|a, b| Rect { left: a.left.min(b.left), top: a.top.min(b.top), right: a.right.max(b.right), bottom: a.bottom.max(b.bottom) })
            && area(&b) <= 2 * rects.iter().map(area).sum::<i64>()
        {
            rects = vec![b];
        }
        // 离屏位图按这次最宽、最高的那块分配，最多 1M 像素，大块就分条画
        let wmax = rects.iter().map(|r| r.right - r.left).max().unwrap_or(0);
        let hmax = rects.iter().map(|r| r.bottom - r.top).max().unwrap_or(0);
        if wmax > 0
            && let Some(scratch) = Bitmap::new(wmax, ((1 << 20) / wmax).clamp(1, hmax.max(1)))
        {
            let painted = call(hwnd, |h, w| {
                for r in &rects {
                    for y in (r.top..r.bottom).step_by(scratch.h as usize) {
                        let strip = Rect { left: r.left, top: y, right: r.right, bottom: (y + scratch.h).min(r.bottom) };
                        h.paint(w, &Canvas::offset(&scratch, strip, (strip.left, strip.top)));
                        let _ = BitBlt(hdc, strip.left, strip.top, strip.right - strip.left, strip.bottom - strip.top, Some(scratch.dc), 0, 0, SRCCOPY);
                    }
                }
            });
            // 正在处理事件时又要重画（很少见）：这次先放过，处理完再整个重画
            if painted.is_none() {
                let target = Win(hwnd.0 as isize);
                Win::later(move || target.invalidate_all());
            }
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

/// 区域拆成的矩形
unsafe fn region_rects(rgn: HRGN) -> Vec<Rect> {
    unsafe {
        let n = GetRegionData(rgn, 0, None);
        if n == 0 {
            return Vec::new();
        }
        // RGNDATA 头后面跟着 nCount 个 RECT；用 u64 对齐分配
        let mut buf = vec![0u64; (n as usize).div_ceil(8)];
        let data = buf.as_mut_ptr() as *mut RGNDATA;
        if GetRegionData(rgn, n, Some(data)) == 0 {
            return Vec::new();
        }
        let hdr = &(*data).rdh;
        std::slice::from_raw_parts((*data).Buffer.as_ptr() as *const Rect, hdr.nCount as usize).to_vec()
    }
}

fn mods() -> Mods {
    let down = |k: VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) } < 0;
    Mods { cmd: down(VK_CONTROL), shift: down(VK_SHIFT), alt: down(VK_MENU) }
}

fn key(vk: u16) -> Key {
    match VIRTUAL_KEY(vk) {
        VK_ESCAPE => Key::Esc,
        VK_RETURN => Key::Enter,
        VK_TAB => Key::Tab,
        VK_SPACE => Key::Space,
        VK_BACK => Key::Back,
        VK_DELETE => Key::Delete,
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        _ if matches!(vk, 0x30..=0x39 | 0x41..=0x5A) => Key::Char(vk as u8),
        _ => Key::Other,
    }
}

fn point(lp: LPARAM) -> Point {
    Point { x: lp.0 as i16 as i32, y: (lp.0 >> 16) as i16 as i32 }
}

fn load(c: Cursor) -> HCURSOR {
    let id = match c {
        Cursor::Arrow => IDC_ARROW,
        Cursor::Cross => IDC_CROSS,
        Cursor::IBeam => IDC_IBEAM,
        Cursor::Move => IDC_SIZEALL,
        Cursor::SizeNS => IDC_SIZENS,
        Cursor::SizeWE => IDC_SIZEWE,
        Cursor::SizeNWSE => IDC_SIZENWSE,
        Cursor::SizeNESW => IDC_SIZENESW,
        Cursor::Ring(d, s) => return ring(d, s).unwrap_or_default(),
    };
    unsafe { LoadCursorW(None, id) }.unwrap_or_default()
}

/// 马赛克笔刷的圆圈光标；换了大小才重做
fn ring(d: f32, s: f32) -> Option<HCURSOR> {
    let key = (d.round() as i32, (s * 96.0).round() as i32);
    if let Some((k, c)) = RING.get()
        && k == key
    {
        return Some(c);
    }
    let (n, px) = ring_pixels(d, s);
    unsafe {
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: n, biHeight: -n, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, px.len());
        // 带 alpha 的光标按 alpha 透明，掩码给全 0（单色图每行按 2 字节对齐）
        let zeros = vec![0u8; ((n + 15) / 16 * 2 * n) as usize];
        let mask = CreateBitmap(n, n, 1, 1, Some(zeros.as_ptr() as _));
        let info = ICONINFO { fIcon: FALSE, xHotspot: (n / 2) as u32, yHotspot: (n / 2) as u32, hbmMask: mask, hbmColor: color };
        // 系统会拷一份，原来的位图用完就删
        let cur = CreateIconIndirect(&info);
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        let cur = HCURSOR(cur.ok()?.0);
        // 旧光标可能正显示着，系统会等它不用了再真正释放
        if let Some((_, old)) = RING.replace(Some((key, cur))) {
            let _ = DestroyCursor(old);
        }
        Some(cur)
    }
}

fn edge_code(e: Edge) -> u32 {
    match e {
        Edge::Left => HTLEFT,
        Edge::Right => HTRIGHT,
        Edge::Top => HTTOP,
        Edge::Bottom => HTBOTTOM,
        Edge::TopLeft => HTTOPLEFT,
        Edge::TopRight => HTTOPRIGHT,
        Edge::BottomLeft => HTBOTTOMLEFT,
        Edge::BottomRight => HTBOTTOMRIGHT,
    }
}

fn sizing_edge(wmsz: u32) -> Option<Edge> {
    Some(match wmsz {
        WMSZ_LEFT => Edge::Left,
        WMSZ_RIGHT => Edge::Right,
        WMSZ_TOP => Edge::Top,
        WMSZ_BOTTOM => Edge::Bottom,
        WMSZ_TOPLEFT => Edge::TopLeft,
        WMSZ_TOPRIGHT => Edge::TopRight,
        WMSZ_BOTTOMLEFT => Edge::BottomLeft,
        WMSZ_BOTTOMRIGHT => Edge::BottomRight,
        _ => return None,
    })
}

/// 输入法上下文里的组合串或上屏的字
fn ime_str(himc: HIMC, kind: IME_COMPOSITION_STRING) -> String {
    unsafe {
        let n = ImmGetCompositionStringW(himc, kind, None, 0).max(0) as u32;
        let mut w = vec![0u16; n as usize / 2];
        ImmGetCompositionStringW(himc, kind, Some(w.as_mut_ptr() as _), n);
        String::from_utf16_lossy(&w)
    }
}

/// 组合窗放在插入光标处（不显示，有的输入法按它放候选窗），候选窗躲开光标那一行
fn ime_follow(hwnd: HWND, r: Rect) {
    unsafe {
        let himc = ImmGetContext(hwnd);
        let _ = ImmSetCompositionWindow(himc, &COMPOSITIONFORM { dwStyle: CFS_POINT, ptCurrentPos: POINT { x: r.left, y: r.top }, rcArea: r });
        let _ = ImmSetCandidateWindow(himc, &CANDIDATEFORM { dwIndex: 0, dwStyle: CFS_EXCLUDE, ptCurrentPos: POINT { x: r.left, y: r.bottom }, rcArea: r });
        let _ = ImmReleaseContext(hwnd, himc);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        }
        // 菜单项自绘不碰窗口状态
        if (msg == WM_MEASUREITEM && menu_measure(lp)) || (msg == WM_DRAWITEM && menu_draw(lp)) {
            return LRESULT(1);
        }
        let Some(st) = state(hwnd) else { return DefWindowProcW(hwnd, msg, wp, lp) };
        let kind = st.kind;
        let def = || DefWindowProcW(hwnd, msg, wp, lp);
        match msg {
            WM_PAINT => {
                // 铺满整屏的遮罩最后一次重画有时不上屏，要等下一次重画才带出来（Win11 25H2、NVIDIA 240 Hz 实测：松手后工具栏不出现）。
                // 停下来 32 ms 后把画过的范围原样再贴一遍，内容不变，只为让它上屏
                let mut r = Rect::default();
                let echo = kind == Kind::Overlay && GetUpdateRect(hwnd, Some(&mut r), false).as_bool();
                paint(hwnd);
                if echo {
                    let mut u = r;
                    if let Some(e) = st.echo.get() {
                        let _ = UnionRect(&mut u, &r, &e);
                    }
                    st.echo.set(Some(u));
                    SetTimer(Some(hwnd), ECHO_TIMER, 32, None);
                }
                settle();
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_SETCURSOR => {
                let c = match (lp.0 & 0xFFFF) as u32 {
                    HTCAPTION if kind == Kind::Card => Cursor::Move,
                    HTCLIENT => {
                        let p = Win(hwnd.0 as isize).cursor_pos();
                        let Some(c) = call(hwnd, |h, w| h.cursor(w, p)) else { return def() };
                        settle();
                        c
                    }
                    _ => return def(),
                };
                SetCursor(Some(load(c)));
                return LRESULT(1);
            }
            WM_NCHITTEST => {
                return match kind {
                    // 胶囊：鼠标穿透
                    Kind::Toast | Kind::RecFrame => LRESULT(HTTRANSPARENT as isize),
                    Kind::Card => {
                        let mut p = point(lp);
                        let _ = ScreenToClient(hwnd, &mut p);
                        let ht = match call(hwnd, |h, w| h.hit(w, p)) {
                            Some(Hit::Caption) => HTCAPTION,
                            Some(Hit::Edge(e)) => edge_code(e),
                            _ => HTCLIENT,
                        };
                        LRESULT(ht as isize)
                    }
                    _ => def(),
                };
            }
            // 点操作栏不抢前台，钉图保持在前台、继续收键盘
            WM_MOUSEACTIVATE if matches!(kind, Kind::PinBar | Kind::RecBar) => return LRESULT(MA_NOACTIVATE as isize),
            // 卡片的大小只由它自己定（见 State::size）
            WM_WINDOWPOSCHANGING if kind == Kind::Card => {
                let p = &mut *(lp.0 as *mut WINDOWPOS);
                if !p.flags.contains(SWP_NOSIZE) {
                    (p.cx, p.cy) = st.size.get();
                }
                return def();
            }
            WM_SIZING => {
                let r = &mut *(lp.0 as *mut Rect);
                if let Some(e) = sizing_edge(wp.0 as u32)
                    && let Some(nr) = call(hwnd, |h, w| h.sizing(w, e, *r))
                {
                    *r = nr;
                    st.size.set((r.right - r.left, r.bottom - r.top));
                    Win(hwnd.0 as isize).invalidate_all();
                }
                settle();
                return LRESULT(1);
            }
            // 标题栏的右键不弹系统菜单
            WM_NCRBUTTONUP if kind == Kind::Card => {}
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                // 按着左键期间鼠标归这个窗口
                SetCapture(hwnd);
                st.captured.set(true);
                deliver(hwnd, Event::Down { p: point(lp), button: Button::Left, double: msg == WM_LBUTTONDBLCLK, mods: mods() });
            }
            WM_LBUTTONUP => {
                deliver(hwnd, Event::Up { button: Button::Left });
                if let Some(st) = state(hwnd)
                    && st.captured.replace(false)
                {
                    let _ = ReleaseCapture();
                }
            }
            WM_RBUTTONDOWN => deliver(hwnd, Event::Down { p: point(lp), button: Button::Right, double: false, mods: mods() }),
            WM_RBUTTONUP => deliver(hwnd, Event::Up { button: Button::Right }),
            WM_CAPTURECHANGED => {
                if st.captured.replace(false) {
                    deliver(hwnd, Event::CaptureLost);
                }
            }
            WM_MOUSEMOVE => {
                if !st.tracking.get() {
                    let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    st.tracking.set(TrackMouseEvent(&mut t).is_ok());
                }
                deliver(hwnd, Event::Move { p: point(lp), mods: mods() });
            }
            WM_MOUSELEAVE => {
                st.tracking.set(false);
                deliver(hwnd, Event::Leave);
            }
            // 滚轮消息里的坐标是屏幕坐标
            WM_MOUSEWHEEL => deliver(hwnd, Event::Wheel { delta: (wp.0 >> 16) as i16 as i32, p: point(lp), mods: mods() }),
            WM_KEYDOWN => deliver(hwnd, Event::Key { key: key(wp.0 as u16), mods: mods() }),
            WM_CHAR => {
                // 控制字符（退格、回车、Ctrl+V 这些）走 WM_KEYDOWN；UTF-16 的代理对不认
                if let Some(c) = char::from_u32(wp.0 as u32).filter(|c| !c.is_control()) {
                    deliver(hwnd, Event::Char(c));
                }
            }
            // 正在输入文字：输入法消息不交给 DefWindowProc，系统就不出它自己的组合窗
            WM_IME_STARTCOMPOSITION | WM_IME_COMPOSITION | WM_IME_ENDCOMPOSITION if st.ime.get().is_some() => {
                match msg {
                    WM_IME_COMPOSITION => {
                        let himc = ImmGetContext(hwnd);
                        let result = (lp.0 as u32 & GCS_RESULTSTR.0 != 0).then(|| ime_str(himc, GCS_RESULTSTR));
                        let cursor = ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0).max(0) as usize;
                        let comp = ime_str(himc, GCS_COMPSTR);
                        let _ = ImmReleaseContext(hwnd, himc);
                        deliver(hwnd, Event::Ime { result, comp, cursor });
                    }
                    WM_IME_ENDCOMPOSITION => deliver(hwnd, Event::Ime { result: None, comp: String::new(), cursor: 0 }),
                    _ => {}
                }
                if let Some(st) = state(hwnd)
                    && let Some(r) = st.ime.get()
                {
                    ime_follow(hwnd, r);
                }
            }
            WM_TIMER if wp.0 == ECHO_TIMER => {
                let _ = KillTimer(Some(hwnd), ECHO_TIMER);
                if let Some(r) = st.echo.take() {
                    let dc = GetDC(Some(hwnd));
                    let _ = BitBlt(dc, r.left, r.top, r.right - r.left, r.bottom - r.top, Some(dc), r.left, r.top, SRCCOPY);
                    ReleaseDC(Some(hwnd), dc);
                }
            }
            WM_TIMER => deliver(hwnd, Event::Timer(wp.0)),
            WM_SETFOCUS | WM_KILLFOCUS => deliver(hwnd, Event::Focus(msg == WM_SETFOCUS)),
            // 设置窗口：切走时记下焦点在哪个输入框，切回来还给它
            WM_ACTIVATE if kind == Kind::Settings => {
                let mut fs = st.fields.borrow_mut();
                if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE {
                    let f = GetFocus();
                    fs.last_focus = fs.id_of(f).map(|_| f);
                } else if let Some(f) = fs.last_focus.take().filter(|f| IsWindowVisible(*f).as_bool()) {
                    let _ = SetFocus(Some(f));
                } else {
                    let _ = SetFocus(Some(hwnd));
                }
                return LRESULT(0);
            }
            // 别的窗口拿到前台；自己弹的对话框（归这个窗口所有）不算
            WM_ACTIVATE => {
                if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE && GetWindow(HWND(lp.0 as _), GW_OWNER).ok() != Some(hwnd) {
                    deliver(hwnd, Event::Deactivate);
                }
                return def();
            }
            WM_DPICHANGED => deliver(hwnd, Event::Dpi { scale: (wp.0 & 0xFFFF) as f32 / 96.0, rect: *(lp.0 as *const Rect) }),
            WM_CLOSE if kind == Kind::Settings => deliver(hwnd, Event::Close),
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => return field::ctl_color(HDC(wp.0 as _)),
            WM_COMMAND if (wp.0 >> 16) as u32 == EN_CHANGE => deliver(hwnd, Event::FieldChanged(wp.0 & 0xFFFF)),
            WM_FIELDKEY => deliver(hwnd, Event::FieldKey { key: key(wp.0 as u16), shift: lp.0 != 0 }),
            WM_RUN => {
                let job = *Box::from_raw(lp.0 as *mut Job);
                call(hwnd, |h, w| job(h as &mut dyn Any, w));
                settle();
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                super::record::uncover(hwnd.0 as isize);
                WINDOWS.with_borrow_mut(|v| v.retain(|w| w.0 != hwnd.0 as isize));
                let p = st as *const State as *mut State;
                // 正在处理它自己的事时被关（不该发生）：宁可漏掉也不释放还在用的
                if st.handler.try_borrow_mut().is_ok() {
                    drop(Box::from_raw(p));
                }
                return def();
            }
            _ => return def(),
        }
        LRESULT(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plat::{Canvas, Event, Handler};

    /// 窗口类名两两不同，也不和外壳隐藏主窗口的重名：重名时后注册的失败，窗口用的是先注册的那个过程
    #[test]
    fn class_names_unique() {
        let kinds = [Kind::Overlay, Kind::Pin, Kind::PinBar, Kind::Card, Kind::Toast, Kind::Settings, Kind::RecFrame, Kind::RecBar, Kind::Tray];
        let mut names: Vec<String> = kinds.iter().map(|k| unsafe { class(*k).to_string().unwrap() }).collect();
        names.push(unsafe { super::super::app::TRAY_CLASS.to_string().unwrap() });
        let n = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), n, "窗口类名重名");
    }

    /// 一张空白卡片
    struct Blank;

    impl Handler for Blank {
        fn event(&mut self, _w: Win, _e: Event) {}

        fn paint(&mut self, _w: Win, c: &Canvas) {
            c.clear(Default::default());
        }
    }

    /// 拖着卡片时内容变了（卡片变高）：系统的拖动循环会按开始拖时的大小摆窗口，卡片不能被缩回去。
    /// 会弹出真的窗口、用键盘移动窗口（鼠标会被挪到窗口上，测完放回），平时不跑：
    /// cargo test drag_keeps_size -- --ignored
    #[test]
    #[ignore]
    fn drag_keeps_size() {
        let m = super::super::monitor_at(Point { x: 0, y: 0 });
        let rect = Rect { left: m.work.left + 100, top: m.work.top + 100, right: m.work.left + 480, bottom: m.work.top + 300 };
        let w = Win::open(Spec { kind: Kind::Card, rect, title: "test", owner: None }, Blank).unwrap();
        w.show();
        let cur = super::super::cursor_pos();
        unsafe {
            // 进了拖动循环才会处理这几条：工作线程交回结果（卡片变高）、键盘挪一步、Esc 取消拖动
            w.post(|_: &mut Blank, w| {
                let r = w.rect();
                w.set_rect(Rect { bottom: r.bottom + 100, ..r });
            });
            for k in [VK_RIGHT, VK_ESCAPE] {
                PostMessageW(Some(w.hwnd()), WM_KEYDOWN, WPARAM(k.0 as usize), LPARAM(0)).unwrap();
            }
            SendMessageW(w.hwnd(), WM_SYSCOMMAND, Some(WPARAM(SC_MOVE as usize)), None);
            let _ = SetCursorPos(cur.x, cur.y);
        }
        let r = w.rect();
        assert_eq!((r.right - r.left, r.bottom - r.top), (380, 300), "拖动循环把卡片缩回去了");
        w.close();
    }
}
