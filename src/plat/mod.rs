//! 平台层：只有 win/、mac/ 能调系统 API，功能模块只用这里导出的类型和函数。
//! 两个平台导出同一组名字（按 cfg 选一个），签名对不上时编译另一个平台就会报错。
//! 纯数据的类型（Rect、Point）在 Windows 上直接是 Win32 的同名结构，免得来回转换。

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::*;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::*;

/// 平台层回调外壳（app.rs）的地方
pub struct App {
    /// 主窗口、托盘都建好以后调一次
    pub start: fn(),
    /// 按了全局快捷键、单击了托盘图标
    pub capture: fn(),
    /// 右键托盘图标：在屏幕坐标这里弹菜单
    pub tray_menu: fn(Point),
    /// 又启动了一个 Jie：提示「已经在运行」
    pub again: fn(),
    #[cfg(target_os = "macos")]
    pub notify: fn(&str, &str, Option<fn()>),
}

/// 一个菜单项；`id` 为 0 表示分组线
pub struct MenuItem<'a> {
    pub id: usize,
    pub text: &'a str,
    /// 右边灰字（快捷键、「推荐」）
    pub key: &'a str,
    pub enabled: bool,
    /// 默认项加粗
    pub default: bool,
    /// 当前选中的一项：左边画实心圆点（设置里的下拉列表）
    pub checked: bool,
}

impl<'a> MenuItem<'a> {
    pub fn new(id: usize, text: &'a str, key: &'a str) -> Self {
        MenuItem { id, text, key, enabled: true, default: false, checked: false }
    }

    pub fn sep() -> Self {
        MenuItem { id: 0, text: "", key: "", enabled: false, default: false, checked: false }
    }
}

// ---- 窗口：功能模块实现 Handler，平台层建窗口、把系统的消息翻成 Event 交给它 ----

/// 窗口种类：决定样式（置顶、阴影、能不能拿焦点、鼠标穿透、透明），平台按种类建窗口
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// 截图遮罩：盖住整块显示器，置顶，拿键盘
    Overlay,
    /// 钉图：置顶、有阴影、可以调整体透明度
    Pin,
    /// 钉图编辑的操作栏：归钉图所有，透明处鼠标穿透，不抢焦点
    PinBar,
    /// 文字卡片：置顶、有阴影、圆角，标题栏拖动、边框改大小
    Card,
    /// 结果胶囊：透明、鼠标穿透、不抢焦点
    Toast,
    /// 设置：普通窗口，有标题栏和关闭按钮
    Settings,
}

/// 建窗口的参数。`rect` 是窗口在桌面上的位置（物理像素）；设置窗口给的是内容区，标题栏另加
pub struct Spec<'a> {
    pub kind: Kind,
    pub rect: Rect,
    pub title: &'a str,
    /// 归谁所有：总在它上面，它关了跟着关
    pub owner: Option<Win>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Left,
    Right,
}

/// 修饰键。`cmd` 在 Windows 上是 Ctrl，在 mac 上是 ⌘：应用内快捷键都看它
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Mods {
    pub cmd: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    /// 字母（大写）和数字
    Char(u8),
    Esc,
    Enter,
    Tab,
    Space,
    Back,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Other,
}

/// 系统交给窗口的事。坐标是窗口内容区的物理像素，标了「屏幕」的除外
#[derive(Debug)]
pub enum Event {
    /// 按下鼠标；`double`：这是双击的第二下。按下左键期间鼠标归这个窗口（移出窗口也照样收到 Move、Up）
    Down { p: Point, button: Button, double: bool, mods: Mods },
    Move { p: Point, mods: Mods },
    Up { button: Button },
    /// 鼠标离开了窗口
    Leave,
    /// 滚轮：一格 120，往上（离开用户）为正；`p` 是屏幕坐标
    Wheel { delta: i32, p: Point, mods: Mods },
    Key { key: Key, mods: Mods },
    /// 打出来的字（不含控制字符）
    Char(char),
    /// 输入法：上屏的字、组合串、组合串里的光标位置（UTF-16）；组合结束时组合串为空。只在 `Win::ime` 打开时来
    Ime { result: Option<String>, comp: String, cursor: usize },
    Timer(usize),
    /// 键盘焦点进出这个窗口
    Focus(bool),
    /// 别的窗口到了前台（自己弹的对话框不算）
    Deactivate,
    /// 按着左键拖动时鼠标被抢走了：不会再有 Up
    CaptureLost,
    /// 窗口挪到了缩放比例不同的显示器上：新的缩放比例、系统建议的窗口位置
    Dpi { scale: f32, rect: Rect },
    /// 点了标题栏的关闭按钮（只有设置窗口有）
    Close,
    /// 输入框（`Win::field`）里的字变了
    FieldChanged(usize),
    /// 在输入框里按了 Tab、Enter、Esc（交给窗口处理）
    FieldKey { key: Key, shift: bool },
}

/// 设置窗口里的系统输入框
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldKind {
    Text,
    /// 显示成圆点（API Key）
    Password,
    /// 只能看、能选中复制（保存位置）
    ReadOnly,
    /// 色值：等宽字，最多 7 个字符
    Hex,
    /// 录全局快捷键：按下组合键就填进去
    Hotkey,
}

/// 窗口上一点是什么：内容区、可以拖动的标题栏、改大小的边框
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Client,
    Caption,
    Edge(Edge),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Edge {
    /// 拖这条边（角）时动的是左、右、上、下哪几边
    pub fn sides(self) -> (bool, bool, bool, bool) {
        use Edge::*;
        (matches!(self, Left | TopLeft | BottomLeft), matches!(self, Right | TopRight | BottomRight), matches!(self, Top | TopLeft | TopRight), matches!(self, Bottom | BottomLeft | BottomRight))
    }
}

/// 鼠标光标。`Ring` 是马赛克笔刷的圆圈：直径（物理像素）和缩放比例
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Cursor {
    Arrow,
    Cross,
    IBeam,
    Move,
    SizeNS,
    SizeWE,
    SizeNWSE,
    SizeNESW,
    Ring(f32, f32),
}

/// 直径 d 的双线圆圈（白 3 + 黑 1，按缩放 s）：逐像素按到圆周的距离算覆盖率（自带抗锯齿）。
/// 返回边长 n（奇数，圆心在正中那个像素的中心）和预乘 alpha 的 BGRA 像素
pub fn ring_pixels(d: f32, s: f32) -> (i32, Vec<u32>) {
    let n = ((d + 3.0 * s).ceil() as i32 + 2) | 1;
    let c = n as f32 / 2.0;
    let cover = |dist: f32, lw: f32| (lw / 2.0 + 0.5 - (dist - d / 2.0).abs()).clamp(0.0, 1.0);
    let px = (0..n * n)
        .map(|i| {
            let dist = ((i % n) as f32 + 0.5 - c).hypot((i / n) as f32 + 0.5 - c);
            let (white, black) = (cover(dist, 3.0 * s), cover(dist, s));
            // 黑线压在白线上
            let a = ((black + white * (1.0 - black)) * 255.0).round() as u32;
            let v = (white * (1.0 - black) * 255.0).round() as u32;
            a << 24 | v << 16 | v << 8 | v
        })
        .collect();
    (n, px)
}

/// 功能模块的窗口：收事件、画画面。都在界面线程上调，同一个窗口不会重入
pub trait Handler: std::any::Any {
    fn event(&mut self, w: Win, e: Event);

    /// 画 `c.area()` 这块（窗口内容区坐标）；透明窗口要把整块都画出来，没画的地方是透明的
    fn paint(&mut self, w: Win, c: &Canvas);

    fn cursor(&mut self, _w: Win, _p: Point) -> Cursor {
        Cursor::Arrow
    }

    /// 这一点是内容区、标题栏还是边框（文字卡片用）
    fn hit(&mut self, _w: Win, _p: Point) -> Hit {
        Hit::Client
    }

    /// 拖边框改大小：系统给的新窗口位置 `r`，返回限制过的
    fn sizing(&mut self, _w: Win, _edge: Edge, r: Rect) -> Rect {
        r
    }
}

/// 一块显示器：整块矩形、工作区（去掉任务栏、菜单栏、程序坞），都是全局桌面上的物理像素
#[derive(Clone, Copy)]
pub struct Monitor {
    pub rect: Rect,
    pub work: Rect,
    pub dpi: u32,
}

impl Monitor {
    /// DIP → 物理像素的倍数
    pub fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }
}

/// 普通内存里的图片：BGRA，自上而下，alpha 都是 255；可以跨线程传
pub struct Image {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u8>,
}

impl Bitmap {
    pub fn full(&self) -> Rect {
        Rect { left: 0, top: 0, right: self.w, bottom: self.h }
    }

    /// 变暗的副本：像素值 × 0.55，等于盖一层 45% 的黑
    pub fn dimmed(&self) -> Option<Bitmap> {
        let mut d = Bitmap::new(self.w, self.h)?;
        // 141/256 ≈ 0.55；整数乘移位能被自动向量化
        for (out, src) in d.pixels_mut().chunks_exact_mut(4).zip(self.pixels().chunks_exact(4)) {
            for i in 0..3 { out[i] = ((src[i] as u16 * 141) >> 8) as u8; }
            out[3] = src[3];
        }
        Some(d)
    }

    /// 拷出一块成为新位图（钉图用）
    pub fn sub(&self, r: &Rect) -> Option<Bitmap> {
        let mut b = Bitmap::new(r.right - r.left, r.bottom - r.top)?;
        let (row, x0, w) = (b.w as usize * 4, r.left as usize * 4, self.w as usize * 4);
        for (y, dst) in b.pixels_mut().chunks_exact_mut(row).enumerate() {
            let start = (r.top as usize + y) * w + x0;
            dst.copy_from_slice(&self.pixels()[start..start + row]);
        }
        Some(b)
    }

    /// 裁出一块，alpha 填 255（截屏出来的 alpha 不一定是 255）
    pub fn crop(&self, r: &Rect) -> Image {
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let (row, x0) = (w as usize * 4, r.left as usize * 4);
        let mut px = Vec::with_capacity(row * h as usize);
        for y in r.top..r.bottom {
            let start = y as usize * self.w as usize * 4 + x0;
            px.extend_from_slice(&self.pixels()[start..start + row]);
        }
        px.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
        Image { w, h, px }
    }
}

#[cfg(windows)]
pub fn request_capture(ready: impl FnOnce(Option<(Monitor, Bitmap)>) + Send + 'static) { ready(capture_cursor_monitor()); }
