//! 长截图：截图选区上点「长截图」后遮罩关掉，选区外画一圈边框（同录屏），旁边是操作栏（取消、完成），开始截这块活的屏幕。
//! 用户滚滚轮、划触控板（底下的窗口自己收到），或按 ↑↓（临时占用这两个键，替用户往区域中间那个窗口发滚轮）；
//! 每来一帧和拼好的长图对一下，接上新露出来的行（`Stitch`）。Esc、Enter 也临时占用：取消、完成。
//! 完成后长图钉在原位置（太高先缩小），下面挂结果条：复制、保存、钉住、丢弃。
//! 只依赖平台层、录屏（边框和操作栏的摆法）、钉图、胶囊、设置和操作栏；不知道遮罩，选区由调用方给。

use crate::pin;
use crate::plat::{self, Bitmap, Button, Canvas, Color, Event, Handler, Monitor, Point, Rect, Spec, Win};
use crate::record;
use crate::settings;
use crate::toast::{self, Kind};
use crate::toolbar::{self, Act, Bar};
use crate::ui::{self, Hud};
use std::cell::RefCell;
use std::sync::{Arc, Mutex, MutexGuard};

/// 每秒截几帧：越快两帧之间挪得越少，越容易对上
const FPS: u32 = 20;
/// 工具提示：悬停 400ms 后出现
const TIP: usize = 1;
/// 长图最多多少行、多少像素（约 240 MB）：再长就不接了
const MAX_ROWS: usize = 30_000;
const MAX_PIXELS: usize = 60_000_000;

// ---- 拼图：纯计算 ----

/// 这一帧拼得怎么样
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// 和上一帧一样，或者还没开始滚
    Same,
    /// 对上了（可能接上了新的行，也可能只是哪里变了色）
    Joined,
    /// 对不上：和已截内容没有足够重叠，或者内容整个换了
    Lost,
    /// 到最长了，不再接
    Full,
}

/// 按行拼长图。每行算一个哈希（右边一截不算：滚动条的滑块跟着动，同样的内容前后两帧那截不一样）。
/// 第一次真滚动时，顶上、底下相同的行暂当固定栏；之后若能对上滚动位置，就把误认的正文空白归还给滚动部分。
/// 之后每帧的中间那段在整张长图里找位置（从上一帧的位置往两边找），伸出去的行接上去，往上滚过开头也接。
/// 不动的那几行取自碰到内容最上面、最下面的那一帧
pub struct Stitch {
    w: usize,
    h: usize,
    /// 每行只哈希前 `cols` 个像素
    cols: usize,
    /// 顶上、底下暂判不动的行数；可随滚动缩小，第一次真滚动以前是 None
    fixed: Option<(usize, usize)>,
    /// 还没接上滚动时：起始位置的像素（BGRA，一行 w × 4），大幅失配不能丢掉它
    first: Vec<u8>,
    /// 拼好的滚动部分（不含不动的行）：像素和每行的哈希
    px: Vec<u8>,
    rows: Vec<u64>,
    /// 上一帧的中间那段从 `rows` 的第几行开始
    pos: usize,
    head: Vec<u8>,
    foot: Vec<u8>,
    /// 上一帧每行的哈希；尚未接上滚动时保留起始位置作为比对基准
    last: Vec<u64>,
    /// 最近一次不是 `Same` 的结果（操作栏上的提示）
    pub step: Step,
}

/// 每行前 `cols` 个像素的哈希（FNV-1a）；不管 alpha：截屏出来的 alpha 不一定是 255
fn hashes(px: &[u8], stride: usize, h: usize, cols: usize) -> Vec<u64> {
    (0..h)
        .map(|y| px[y * stride..][..cols * 4].chunks_exact(4).fold(0xcbf2_9ce4_8422_2325u64, |a, p| (a ^ u32::from_le_bytes([p[0], p[1], p[2], 0]) as u64).wrapping_mul(0x100_0000_01b3)))
        .collect()
}

/// 帧里 `ys` 这几行紧排成一块
fn pack(px: &[u8], stride: usize, row: usize, ys: std::ops::Range<usize>) -> Vec<u8> {
    let mut v = Vec::with_capacity(ys.len() * row);
    for y in ys {
        v.extend_from_slice(&px[y * stride..][..row]);
    }
    v
}

/// 帧中间那段 `band` 对在长图的第几行：先比匹配证据，证据相同时取离 `pos` 最近的位置。
/// 对得上：重叠至少 1/8 段高（不少于 16 行），重叠里至少 8 行「有内容」，对不上的行不超过有内容的行的 1/8（鼠标停的地方变色这类）。
/// 「有内容」：和上下两行不全一样，一大片空白、纯色中间的行不算，不然空白处哪儿都对得上
fn find(band: &[u64], rows: &[u64], pos: usize) -> Option<isize> {
    let (n, len) = (band.len() as isize, rows.len() as isize);
    let min_ov = (n / 8).max(16).min(n);
    let mut inf = vec![0usize; band.len() + 1];
    for i in 0..band.len() {
        let flat = i > 0 && i + 1 < band.len() && band[i] == band[i - 1] && band[i] == band[i + 1];
        inf[i + 1] = inf[i] + usize::from(!flat);
    }
    let score = |p: isize| {
        let (a, e) = ((-p).max(0), (len - p).min(n));
        if e - a < min_ov {
            return None;
        }
        let k = inf[e as usize] - inf[a as usize];
        let mut bad = 0;
        let fits = k >= 8 && (a..e).all(|i| {
            bad += usize::from(band[i as usize] != rows[(p + i) as usize]);
            bad * 8 <= k
        });
        // 容错不是优先接受旧位置：每个错行抵消八行证据，避免重复笔画吞掉新露出的行。
        fits.then_some((k.saturating_sub(bad * 8), e - a, bad))
    };
    let pos = pos as isize;
    // 回滚可能跨过多屏已截内容；搜索所有仍有足够重叠的位置，而不是仅限上一帧附近一屏。
    let (start, end) = (min_ov - n, len - min_ov);
    let distance = (pos - start).max(end - pos);
    let mut best = None;
    for p in (0..=distance).flat_map(|d| [pos + d, pos - d].into_iter().take(if d == 0 { 1 } else { 2 })) {
        if let Some((evidence, overlap, bad)) = score(p) {
            if best.is_none_or(|(_, k, ov)| (evidence, overlap) > (k, ov)) {
                best = Some((p, evidence, overlap));
            }
            // 整段精确匹配已经是最多证据；保留最近位置，不必扫描剩余长图。
            if bad == 0 && overlap == n {
                break;
            }
        }
    }
    best.map(|(p, _, _)| p)
}

impl Stitch {
    /// 区域 w × h；右边 `skip` 列不比（滚动条）
    pub fn new(w: usize, h: usize, skip: usize) -> Stitch {
        let cols = w.saturating_sub(skip).max(1).min(w);
        Stitch { w, h, cols, fixed: None, first: Vec::new(), px: Vec::new(), rows: Vec::new(), pos: 0, head: Vec::new(), foot: Vec::new(), last: Vec::new(), step: Step::Same }
    }

    /// 拼出来的长图有多高（还没来过帧是 0）
    pub fn height(&self) -> usize {
        match self.fixed {
            Some((t, b)) => t + self.rows.len() + b,
            None => self.first.len() / (self.w * 4),
        }
    }

    pub fn width(&self) -> usize {
        self.w
    }

    /// 来了一帧：BGRA，每行 `stride` 字节
    pub fn push(&mut self, px: &[u8], stride: usize) -> Step {
        let (h, row) = (self.h, self.w * 4);
        let cur = hashes(px, stride, h, self.cols);
        if self.last.is_empty() {
            self.first = pack(px, stride, row, 0..h);
            self.last = cur;
            return Step::Same;
        }
        if cur == self.last {
            // 第一次滚太快后退回起点：画面恢复了，也清掉失配提示。
            if self.fixed.is_none() {
                self.step = Step::Same;
            }
            return Step::Same;
        }
        let step = match self.fixed {
            Some((t, b)) => match self.locate(&cur, t, b) {
                Some(p) => self.join(cur, px, stride, p),
                None => {
                    self.last = cur;
                    Step::Lost
                }
            },
            None => {
                // 还没接上滚动：顶上、底下一样的行先当成不动的，中间那段和起始位置比
                let t = cur.iter().zip(&self.last).take_while(|(a, b)| a == b).count();
                let b = cur.iter().rev().zip(self.last.iter().rev()).take(h - t).take_while(|(a, b)| a == b).count();
                match find(&cur[t..h - b], &self.last[t..h - b], 0) {
                    Some(p) if p != 0 => {
                        // 第一次接上滚动：起始帧拆成顶、中、底，中间那段就是拼好的内容
                        let first = std::mem::take(&mut self.first);
                        self.fixed = Some((t, b));
                        self.head = first[..t * row].to_vec();
                        self.foot = first[(h - b) * row..].to_vec();
                        self.px = first[t * row..(h - b) * row].to_vec();
                        self.rows = self.last[t..h - b].to_vec();
                        self.join(cur, px, stride, p)
                    }
                    // 少量局部变色只更新静止画面；大幅失配保留起点并提示回滚，避免漏掉开头。
                    _ if h - t - b <= (h / 8).max(1) => {
                        self.first = pack(px, stride, row, 0..h);
                        self.last = cur;
                        Step::Same
                    }
                    _ => Step::Lost,
                }
            }
        };
        self.step = step;
        step
    }

    /// 先尝试收回误判为固定栏的正文，再定位；校验失败不能改动已拼好的内容。
    fn locate(&mut self, cur: &[u64], t: usize, b: usize) -> Option<isize> {
        let (h, row) = (self.h, self.w * 4);
        let head = hashes(&self.head, row, t, self.cols);
        let foot = hashes(&self.foot, row, b, self.cols);
        let nt = cur.iter().zip(&head).take_while(|(a, b)| a == b).count();
        let nb = cur.iter().rev().zip(foot.iter().rev()).take_while(|(a, b)| a == b).count();
        if (nt, nb) != (t, b) {
            let (top, bottom) = (t - nt, b - nb);
            let rows: Vec<_> = head[nt..].iter().chain(&self.rows).chain(&foot[..bottom]).copied().collect();
            let band = &cur[nt..h - nb];
            if let Some(p) = find(band, &rows, self.pos) {
                // 新纳入的边缘必须按同一位移逐行吻合，不能把时钟、悬停变色等固定栏变化当作滚动。
                let matches = |i: isize, j: isize| {
                    i < 0 || j < 0 || i >= band.len() as isize || j >= rows.len() as isize || band[i as usize] == rows[j as usize]
                };
                let current = (0..top).chain(band.len() - bottom..band.len()).all(|i| matches(i as isize, p + i as isize));
                let saved = (0..top).chain(rows.len() - bottom..rows.len()).all(|j| matches(j as isize - p, j as isize));
                if current && saved {
                    // 只移动分界，不丢像素；两边同时扩回正文，原有帧的位置坐标无需平移。
                    self.px.splice(0..0, self.head.drain(nt * row..));
                    self.px.extend(self.foot.drain(..bottom * row));
                    self.rows = rows;
                    self.fixed = Some((nt, nb));
                    return Some(p);
                }
            }
        }
        find(&cur[t..h - b], &self.rows, self.pos)
    }

    /// 这一帧的中间那段对在内容的第 `p` 行：伸出去的接上，记下位置；碰到最上面、最下面就换上它的顶、底
    fn join(&mut self, cur: Vec<u64>, px: &[u8], stride: usize, p: isize) -> Step {
        let Some((t, b)) = self.fixed else { return Step::Lost };
        let (h, row, len) = (self.h, self.w * 4, self.rows.len());
        let n = h - t - b;
        let (up, down) = ((-p).max(0) as usize, (p + n as isize - len as isize).max(0) as usize);
        self.last = cur;
        if t + len + up + down + b > MAX_ROWS.min(MAX_PIXELS / self.w) {
            return Step::Full;
        }
        let cur = &self.last;
        if up > 0 {
            self.px.splice(0..0, pack(px, stride, row, t..t + up));
            self.rows.splice(0..0, cur[t..t + up].iter().copied());
        }
        if down > 0 {
            self.px.extend_from_slice(&pack(px, stride, row, t + n - down..t + n));
            self.rows.extend_from_slice(&cur[t + n - down..t + n]);
        }
        self.pos = p.max(0) as usize;
        if self.pos == 0 {
            self.head = pack(px, stride, row, 0..t);
        }
        if self.pos + n >= self.rows.len() {
            self.foot = pack(px, stride, row, h - b..h);
        }
        Step::Joined
    }

    /// 长图写进 `out`（BGRA，宽 `width()`、高 `height()`），alpha 填 255
    pub fn write(&self, out: &mut [u8]) {
        let parts: [&[u8]; 3] = match self.fixed {
            Some(_) => [&self.head, &self.px, &self.foot],
            None => [&self.first, &[], &[]],
        };
        let mut at = 0;
        for p in parts {
            out[at..at + p.len()].copy_from_slice(p);
            at += p.len();
        }
        out.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
    }
}

// ---- 一次长截图 ----

struct Session {
    mon: Monitor,
    region: Rect,
    stitch: Arc<Mutex<Stitch>>,
    rec: Option<plat::Recording>,
    /// 边框的四条和操作栏
    windows: Vec<Win>,
    bar: Option<Win>,
    /// 发不了滚轮（mac 没给辅助功能权限）已经提示过
    warned: bool,
}

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

fn lock(m: &Mutex<Stitch>) -> MutexGuard<'_, Stitch> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 正在长截图
pub fn active() -> bool {
    SESSION.with_borrow(|s| s.is_some())
}

/// 开始截 `region`（全局桌面物理像素，在显示器 `mon` 上）：框和操作栏出来，马上开始截；已经在截就不管
pub fn start(mon: Monitor, region: Rect) {
    if active() {
        return;
    }
    let (w, h) = ((region.right - region.left) as usize, (region.bottom - region.top) as usize);
    // 右边 24 DIP 不比：滚动条
    let skip = ((24.0 * mon.scale()).round() as usize).min(w / 4);
    let stitch = Arc::new(Mutex::new(Stitch::new(w, h, skip)));
    let mut windows = record::frame(&mon, region, settings::sel_color());
    let bar = LongBar::open(mon, region);
    windows.extend(bar);
    SESSION.set(Some(Session { mon, region, stitch: stitch.clone(), rec: None, windows, bar, warned: false }));
    // 截屏线程上拼；长了、提示变了才叫界面线程更新操作栏
    let on_frame: plat::OnFrame = Box::new(move |px, stride, _| {
        let mut st = lock(&stitch);
        let before = (st.height(), st.step);
        st.push(px, stride);
        if (st.height(), st.step) != before {
            drop(st);
            plat::post(refresh);
        }
    });
    match plat::record(region, FPS, false, on_frame, |e| plat::post(move || broken(e))) {
        Ok(rec) => {
            SESSION.with_borrow_mut(|s| {
                if let Some(s) = s {
                    s.rec = Some(rec);
                }
            });
            plat::grab_keys(true);
        }
        Err(e) => {
            cancel();
            toast::show(&mon, Kind::Err, &format!("长截图没能开始：{e}"));
        }
    }
}

/// 长截图时的快捷键：↑↓ 替用户滚区域，Esc 取消；Enter、截图和录屏快捷键、单击托盘都是完成
pub fn key(id: usize) {
    match id {
        plat::KEY_UP => scroll(120),
        plat::KEY_DOWN => scroll(-120),
        plat::KEY_ESC => cancel(),
        _ => finish(),
    }
}

/// 往区域中间那个窗口发滚轮；发不了（mac 没给权限）提示一次
fn scroll(delta: i32) {
    let Some((mon, r, warned)) = SESSION.with_borrow(|s| s.as_ref().map(|s| (s.mon, s.region, s.warned))) else { return };
    if let Err(e) = plat::scroll(Point { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 }, delta)
        && !warned
    {
        SESSION.with_borrow_mut(|s| s.as_mut().map(|s| s.warned = true));
        toast::show(&mon, Kind::Err, &e);
    }
}

/// 操作栏上的说明：长图多大，和现在该怎么做
fn status(w: usize, h: usize, step: Step) -> String {
    let hint = match step {
        Step::Lost => "没接上，往回滚一点",
        Step::Full => "已到最长",
        _ => "滚动或按 ↑↓",
    };
    format!("{w} × {h} · {hint}")
}

/// 截屏线程拼完一帧以后：操作栏上的尺寸、提示跟着变（先取出来再改操作栏：重画时可能顺手做推迟的完成、取消）
fn refresh() {
    let got = SESSION.with_borrow(|s| {
        let s = s.as_ref()?;
        let st = lock(&s.stitch);
        Some((s.bar?, status(st.width(), st.height(), st.step)))
    });
    if let Some((bar, info)) = got {
        bar.with(|lb: &mut LongBar| lb.set_info(bar, &info));
    }
}

/// 停住、关掉边框和操作栏、放掉临时占用的键
fn end() -> Option<Session> {
    let s = SESSION.take()?;
    plat::grab_keys(false);
    s.windows.iter().for_each(|w| w.close());
    Some(s)
}

/// 完成：长图钉在原位置，下面挂结果条
pub fn finish() {
    let Some(Session { mon, region, stitch, rec, .. }) = end() else { return };
    // 等截屏线程停下（Windows）、帧回调不再进来（mac），之后拼图只归这边
    drop(rec);
    let st = lock(&stitch);
    let (w, h) = (st.width() as i32, st.height() as i32);
    let Some(mut bmp) = (h > 0).then(|| Bitmap::new(w, h)).flatten() else {
        return toast::show(&mon, Kind::Err, if h > 0 { "内存不够，放不下这张长图" } else { "没有截到画面" });
    };
    st.write(bmp.pixels_mut());
    drop(st);
    drop(stitch);
    pin::create_result(bmp, region, format!("{w} × {h}"));
}

/// 取消：停掉，什么都不留
pub fn cancel() {
    drop(end());
}

/// 平台层报截不下去了（权限被收回、显示器断开）：放弃这次，告诉用户
fn broken(e: String) {
    let Some(mon) = SESSION.with_borrow(|s| s.as_ref().map(|s| s.mon)) else { return };
    cancel();
    toast::show(&mon, Kind::Err, &format!("长截图中断：{e}"));
}

// ---- 操作栏：「1200 × 3400 · 滚动或按 ↑↓ | [✕] [✓]」，样式同截图操作栏；摆法同录屏控制条（区域外面，放不下才放进去），
// 窗口按最宽的说明定好就不再变，说明变短时左边空着（透明、鼠标穿透）；提示放在不压着区域的那一边 ----

struct LongBar {
    scale: f32,
    /// 窗口宽、操作栏在窗口里的顶
    w: i32,
    top: i32,
    /// 提示在操作栏上方（否则下方）
    tip_up: bool,
    bar: Bar,
    hot: Option<Act>,
    down: Option<Act>,
    tip: Option<Act>,
}

impl LongBar {
    /// 带说明 `info` 排好，右边贴着窗口右边 `w`、顶在 `top`
    fn layout(s: f32, info: &str, w: i32, top: i32) -> Bar {
        const FAR: i32 = i32::MAX / 4;
        let b = Bar::layout(Rect::default(), FAR, FAR, s, 0, &toolbar::LONG, Some(info));
        let (dx, dy) = (w - b.rect.right, top - b.rect.top);
        b.moved(dx, dy)
    }

    fn open(mon: Monitor, region: Rect) -> Option<Win> {
        let s = mon.scale();
        let px = |d: f32| (d * s).round() as i32;
        let widest = Self::layout(s, &status(99_999, 99_999, Step::Lost), 0, 0).rect;
        let (bw, bh) = (widest.right - widest.left, widest.bottom - widest.top);
        let b = record::border(s);
        let (spot, inside) = record::bar_rect(&mon.rect, &mon.work, region, (bw, bh), b, px(8.0) + b, px(4.0));
        // 提示那一截（6 + 26）先找不压着区域、不出屏的一边（下面优先），都不行就找不出屏的
        let extra = px(32.0);
        let below = Rect { top: spot.bottom, bottom: spot.bottom + extra, ..spot };
        let above = Rect { top: spot.top - extra, bottom: spot.top, ..spot };
        let fits = |r: &Rect| r.top >= mon.rect.top && r.bottom <= mon.rect.bottom;
        let clear = |r: &Rect| r.right <= region.left || region.right <= r.left || r.bottom <= region.top || region.bottom <= r.top;
        let tip_up = if fits(&below) && clear(&below) {
            false
        } else if fits(&above) && clear(&above) {
            true
        } else {
            !fits(&below)
        };
        let (rect, top) = if tip_up { (Rect { top: above.top, ..spot }, extra) } else { (Rect { bottom: below.bottom, ..spot }, 0) };
        let info = status((region.right - region.left) as usize, (region.bottom - region.top) as usize, Step::Same);
        let bar = Self::layout(s, &info, bw, top);
        let lb = LongBar { scale: s, w: bw, top, tip_up, bar, hot: None, down: None, tip: None };
        let win = Win::open(Spec { kind: plat::Kind::RecBar, rect, title: "Jie 长截图", owner: None }, lb)?;
        // 放进区域里了（区域框满屏幕）：不截它，截它底下的画面
        if inside {
            win.hide_from_capture();
        }
        win.show();
        Some(win)
    }

    fn set_info(&mut self, w: Win, info: &str) {
        self.bar = Self::layout(self.scale, info, self.w, self.top);
        w.invalidate_all();
    }

    /// 鼠标换了按钮：提示先收起，停 400ms 再出来
    fn set_hot(&mut self, w: Win, hot: Option<Act>) {
        if hot != self.hot {
            (self.hot, self.tip) = (hot, None);
            w.kill_timer(TIP);
            if hot.is_some() && self.down.is_none() {
                w.timer(TIP, 400);
            }
            w.invalidate_all();
        }
    }
}

impl Handler for LongBar {
    fn event(&mut self, w: Win, e: Event) {
        match e {
            Event::Move { p, .. } => self.set_hot(w, self.bar.hit(p)),
            Event::Leave => self.set_hot(w, None),
            Event::Down { p, button: Button::Left, .. } => {
                (self.down, self.tip) = (self.bar.hit(p), None);
                w.kill_timer(TIP);
                w.invalidate_all();
            }
            // 松开时还在按下的那个按钮上才算点击；完成、取消都会关掉这个窗口，等这次处理完再做
            Event::Up { button: Button::Left } => {
                let clicked = self.down.take().filter(|a| self.hot == Some(*a));
                w.invalidate_all();
                match clicked {
                    Some(Act::Done) => Win::later(finish),
                    Some(_) => Win::later(cancel),
                    None => {}
                }
            }
            Event::CaptureLost => self.down = None,
            Event::Timer(TIP) => {
                w.kill_timer(TIP);
                self.tip = self.hot;
                w.invalidate_all();
            }
            // 滚轮落在操作栏上：也替用户滚区域
            Event::Wheel { delta, .. } => scroll(delta),
            _ => {}
        }
    }

    fn paint(&mut self, _w: Win, c: &Canvas) {
        c.clear(Color::default());
        self.bar.draw(c, self.hot, self.down, None, |_| true);
        // 提示：按钮正上方或正下方，夹在窗口里
        let Some(a) = self.tip else { return };
        let (name, key) = self.bar.tip(a);
        let Some(h) = Hud::new(&[(name, 0xFFFFFF), (key, ui::HUD_TEXT2)], self.scale) else { return };
        let r = self.bar.button(a);
        let g = (6.0 * self.scale).round() as i32;
        let x = ((r.left + r.right - h.w) / 2).clamp(0, (self.w - h.w).max(0));
        let y = if self.tip_up { self.bar.rect.top - g - h.h } else { self.bar.rect.bottom + g };
        h.draw(c, x, y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一页假网页：每行不一样（有一段 60 行的空白），宽 W
    const W: usize = 120;
    const SKIP: usize = 12;
    /// 帧高 200：顶栏 30、内容 150、底栏 20；右边 10 列是滚动条
    const H: usize = 200;
    const TOP: usize = 30;
    const VIEW: usize = 150;

    fn page_px(y: usize, x: usize) -> [u8; 4] {
        if (200..260).contains(&y) {
            return [0xFF, 0xFF, 0xFF, 0xFF];
        }
        let v = ((x / 8) as u32).wrapping_mul(2_654_435_761) ^ (y as u32).wrapping_mul(40_503);
        [v as u8, (v >> 8) as u8, (v >> 16) as u8, 0]
    }

    /// 网页滚到 `off` 时区域的一帧（每行末尾多 8 字节，和截屏的 stride 一样不一定紧排）；`hover` 时中间几行变色
    fn frame(off: usize, hover: bool) -> (Vec<u8>, usize) {
        let stride = W * 4 + 8;
        let mut v = vec![0u8; stride * H];
        for y in 0..H {
            for x in 0..W {
                let c = if x >= W - 10 {
                    // 滚动条：滑块跟着 off 动
                    if (off / 6 + 5..off / 6 + 45).contains(&y) { [0x30; 4] } else { [0xE0; 4] }
                } else if y < TOP {
                    [0x40, 0x80, (y * 3) as u8, 0]
                } else if y >= TOP + VIEW {
                    [0x90, (y * 5) as u8, 0x20, 0]
                } else if hover && (TOP + 10..TOP + 16).contains(&y) {
                    [0x11, 0x22, 0x33, 0]
                } else {
                    page_px(off + y - TOP, x)
                };
                v[y * stride + x * 4..][..4].copy_from_slice(&c);
            }
        }
        (v, stride)
    }

    fn feed(s: &mut Stitch, off: usize, hover: bool) -> Step {
        let (px, stride) = frame(off, hover);
        s.push(&px, stride)
    }

    /// 拼好的长图应该是：顶栏、网页 [from, to)、底栏（滚动条那几列不比）
    fn check(s: &Stitch, from: usize, to: usize) {
        let (w, h) = (s.width(), s.height());
        assert_eq!(h, TOP + (to - from) + (H - TOP - VIEW), "高度");
        let mut out = vec![0u8; w * h * 4];
        s.write(&mut out);
        let (f, stride) = frame(0, false);
        for y in 0..h {
            for x in 0..W - SKIP {
                let want = if y < TOP {
                    f[y * stride + x * 4..][..3].to_vec()
                } else if y < TOP + to - from {
                    page_px(from + y - TOP, x)[..3].to_vec()
                } else {
                    f[(y - (to - from) + VIEW) * stride + x * 4..][..3].to_vec()
                };
                assert_eq!(&out[(y * w + x) * 4..][..3], &want[..], "第 {y} 行第 {x} 列");
            }
        }
    }

    #[test]
    fn scroll_down_with_fixed_bars() {
        let mut s = Stitch::new(W, H, SKIP);
        assert_eq!(feed(&mut s, 0, false), Step::Same);
        assert_eq!(feed(&mut s, 0, false), Step::Same);
        // 鼠标停的地方变色，没滚：不算开始
        assert_eq!(feed(&mut s, 0, true), Step::Same);
        assert_eq!(feed(&mut s, 0, false), Step::Same);
        // 开始以后小步滚动时鼠标下面时不时变色：照样对得上，也不会接进去
        let mut off = 0;
        for (i, d) in [7, 40, 3, 90, 1, 60, 130, 25, 70].iter().cycle().enumerate() {
            if off + d > 900 - VIEW {
                break;
            }
            off += d;
            assert_eq!(feed(&mut s, off, i > 0 && *d < 50 && i % 3 == 0), Step::Joined, "滚到 {off}");
        }
        check(&s, 0, off + VIEW);
        // 滚回去一点又回来：不重复接
        assert_eq!(feed(&mut s, off - 100, false), Step::Joined);
        assert_eq!(feed(&mut s, off, false), Step::Joined);
        check(&s, 0, off + VIEW);
    }

    #[test]
    fn scroll_up_past_start() {
        let mut s = Stitch::new(W, H, SKIP);
        feed(&mut s, 500, false);
        for off in [530, 560, 520, 400, 290, 180, 60, 0] {
            assert_eq!(feed(&mut s, off, false), Step::Joined, "滚到 {off}");
        }
        check(&s, 0, 560 + VIEW);
    }

    #[test]
    fn fast_reverse_past_start() {
        let mut s = Stitch::new(W, H, SKIP);
        feed(&mut s, 300, false);
        for off in [360, 480, 600] {
            assert_eq!(feed(&mut s, off, false), Step::Joined);
        }
        // 从底部一次回滚到起点上方：与长图开头仍有重叠，不能受上一帧位置限制。
        for off in [280, 160, 40, 0] {
            assert_eq!(feed(&mut s, off, false), Step::Joined, "回滚到 {off}");
            check(&s, off, 600 + VIEW);
        }
        // 再跳回已截到的底部并继续向下，既不漏行，也不重复接上旧内容。
        for off in [600, 700] {
            assert_eq!(feed(&mut s, off, false), Step::Joined, "下滚到 {off}");
            check(&s, 0, off + VIEW);
        }
    }

    #[test]
    fn lost_then_reverse_to_captured_top() {
        let mut s = Stitch::new(W, H, SKIP);
        feed(&mut s, 300, false);
        for off in [400, 500, 600] {
            assert_eq!(feed(&mut s, off, false), Step::Joined);
        }
        // 跳到没有重叠的区域不能猜着接；回到长图任意位置都应能恢复。
        assert_eq!(feed(&mut s, 1000, false), Step::Lost);
        check(&s, 300, 600 + VIEW);
        assert_eq!(feed(&mut s, 300, false), Step::Joined);
        assert_eq!(feed(&mut s, 180, false), Step::Joined);
        check(&s, 180, 600 + VIEW);
    }

    #[test]
    fn reverse_with_blank_viewport_edges() {
        // 首次滚动时边缘恰好是空白，回滚后也必须保留真实顶端、底端的像素。
        for offsets in [
            &[80, 100, 140, 180, 220, 260, 300, 320, 60, 0, 320][..],
            &[210, 230, 270, 310, 350, 390, 430, 450, 190, 140, 90, 40, 0, 450][..],
        ] {
            let mut s = Stitch::new(W, H, SKIP);
            feed(&mut s, offsets[0], false);
            for &off in &offsets[1..] {
                assert_eq!(feed(&mut s, off, false), Step::Joined, "从 {} 滚到 {off}", offsets[0]);
            }
            check(&s, 0, offsets.last().unwrap() + VIEW);
        }
    }

    #[test]
    fn reverse_reclaims_initial_blank_header() {
        let mut s = Stitch::new(W, H, SKIP);
        feed(&mut s, 210, false);
        assert_eq!(feed(&mut s, 230, false), Step::Joined);
        // 首次相同的 30 行空白属于正文，不是顶栏；回滚时必须让它重新参与匹配。
        for off in [190, 90, 0] {
            assert_eq!(feed(&mut s, off, false), Step::Joined, "回滚到 {off}");
            assert_eq!(s.fixed, Some((TOP, H - TOP - VIEW)), "正文不能继续留在固定栏里");
            check(&s, off, 230 + VIEW);
        }
    }

    #[test]
    fn scroll_reclaims_initial_blank_footer() {
        let mut s = Stitch::new(W, H, SKIP);
        feed(&mut s, 80, false);
        // 同样验证底部空白：中间那段的重叠全是空白时，不能丢掉被误裁的有效内容。
        for off in [100, 140, 220, 300] {
            assert_eq!(feed(&mut s, off, false), Step::Joined, "下滚到 {off}");
            check(&s, 80, off + VIEW);
        }
    }

    #[test]
    fn changing_fixed_header_is_not_scrolling_content() {
        let mut s = Stitch::new(W, H, SKIP);
        feed(&mut s, 300, false);
        feed(&mut s, 320, false);
        let (mut px, stride) = frame(340, false);
        // 模拟固定顶栏时钟变色；边缘变化本身不能成为缩小固定栏的依据。
        for y in 10..12 {
            px[y * stride..][..W * 4].fill(0x55);
        }
        assert_eq!(s.push(&px, stride), Step::Joined);
        assert_eq!(s.fixed, Some((TOP, H - TOP - VIEW)));
        check(&s, 300, 340 + VIEW);
        assert_eq!(feed(&mut s, 280, false), Step::Joined);
        check(&s, 280, 340 + VIEW);
    }

    #[test]
    fn first_scroll_is_up() {
        let mut s = Stitch::new(W, H, SKIP);
        feed(&mut s, 350, false);
        // 不先向下初始化：从正文中间直接向上，一直补到页首。
        for off in [330, 270, 210, 150, 90, 30, 0] {
            assert_eq!(feed(&mut s, off, false), Step::Joined, "向上滚到 {off}");
            check(&s, off, 350 + VIEW);
        }
    }

    #[test]
    fn repeated_rows_prefer_the_stronger_shift() {
        // 表格、文字的重复笔画会让旧位置勉强通过容错；上方新行不能因此被吞掉。
        let mut rows: Vec<u64> = (0..100).map(|i| i % 3 + 1).collect();
        rows[60] = 80;
        let band: Vec<_> = [101, 102, 103].into_iter().chain(rows[..97].iter().copied()).collect();
        assert_eq!(find(&band, &rows, 0), Some(-3));
    }

    #[test]
    fn lost_then_found() {
        let mut s = Stitch::new(W, H, SKIP);
        for off in [0, 50, 100] {
            feed(&mut s, off, false);
        }
        // 一下挪了两屏：对不上，不接；挪回到接得上的地方继续
        assert_eq!(feed(&mut s, 400, false), Step::Lost);
        assert_eq!(s.step, Step::Lost);
        assert_eq!(s.height(), TOP + 100 + VIEW + 20);
        assert_eq!(feed(&mut s, 140, false), Step::Joined);
        assert_eq!(feed(&mut s, 260, false), Step::Joined);
        check(&s, 0, 260 + VIEW);
    }

    #[test]
    fn first_scroll_lost_keeps_start() {
        let mut s = Stitch::new(W, H, SKIP);
        feed(&mut s, 0, false);
        // 第一次就滚过两屏也不能丢掉起点；回滚以后仍能接出完整的开头。
        assert_eq!(feed(&mut s, 400, false), Step::Lost);
        check(&s, 0, VIEW);
        assert_eq!(feed(&mut s, 0, false), Step::Same);
        assert_eq!(s.step, Step::Same);
        assert_eq!(feed(&mut s, 40, false), Step::Joined);
        check(&s, 0, 40 + VIEW);
    }

    /// 能滚的测试窗口：一页假网页（每行不一样），滚轮一格挪 60 行
    #[cfg(windows)]
    struct Page {
        img: Bitmap,
        off: i32,
        view: i32,
    }

    #[cfg(windows)]
    impl Handler for Page {
        fn event(&mut self, w: Win, e: Event) {
            if let Event::Wheel { delta, .. } = e {
                self.off = (self.off - delta / 2).clamp(0, self.img.h - self.view);
                w.invalidate_all();
            }
        }

        fn paint(&mut self, _w: Win, c: &Canvas) {
            c.blit(&self.img, Rect { left: 0, top: self.off, right: self.img.w, bottom: self.off + self.view }, 0, 0);
        }
    }

    #[cfg(windows)]
    fn wait(ms: u64) {
        let t0 = std::time::Instant::now();
        while t0.elapsed() < std::time::Duration::from_millis(ms) {
            if !plat::pump() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    /// 整个流程：从页面中间开始，先下滚再快速回滚到起点上方，最后滚到底，拼出整页；完成后原位置出长图钉图和结果条。
    /// 操作栏的样子存成 target/test-tmp/longshot/scene.png，长图存成 long.png。会在屏幕上开窗口、截屏，平时不跑：
    /// cargo test longshot_flow -- --ignored
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn longshot_flow() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("longshot");
        std::fs::create_dir_all(&dir).unwrap();
        let mon = plat::monitor_at(plat::cursor_pos());
        let (pw, ph, view) = (480, 2000, 420);
        let mut img = Bitmap::new(pw, ph).unwrap();
        for (i, p) in img.pixels_mut().chunks_exact_mut(4).enumerate() {
            let (x, y) = (i as u32 % pw as u32, i as u32 / pw as u32);
            let v = (x / 16).wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503);
            // 开始截图的位置位于空白段，覆盖旧逻辑把正文误锁为固定顶栏的情况。
            let color = if (600..750).contains(&y) { [255; 4] } else { [v as u8, (v >> 8) as u8, (v >> 16) as u8, 255] };
            p.copy_from_slice(&color);
        }
        let expect: Vec<u8> = img.pixels().to_vec();
        let at = Point { x: mon.work.left + 300, y: mon.work.top + 200 };
        let rect = Rect { left: at.x, top: at.y, right: at.x + pw, bottom: at.y + view };
        let page = Win::open(Spec { kind: plat::Kind::Pin, rect, title: "Jie 测试网页", owner: None }, Page { img, off: 600, view }).unwrap();
        page.show();
        wait(300);
        start(mon, rect);
        assert!(active());
        let bar = SESSION.with_borrow(|s| s.as_ref().unwrap().bar).expect("操作栏没出来");
        let br = bar.rect();
        assert!(br.top >= rect.bottom, "操作栏该在区域下面：{br:?}");
        wait(300);
        // 先下滚一格，再回滚到起点上方；首次判定的空白顶栏必须被纠正。
        key(plat::KEY_DOWN);
        wait(120);
        for _ in 0..2 {
            key(plat::KEY_UP);
            wait(120);
        }
        SESSION.with_borrow(|s| {
            let st = lock(&s.as_ref().unwrap().stitch);
            assert_eq!(st.fixed, Some((0, 0)), "测试页面没有固定栏");
            assert_eq!(st.height(), 540, "起点上方的 60 行没补进来");
        });
        for _ in 0..11 {
            key(plat::KEY_DOWN);
            wait(120);
        }
        // 一次跨过两屏回到起点上方，但仍与已截长图有 120 行重叠。
        scroll(1920);
        wait(120);
        for _ in 0..4 {
            key(plat::KEY_UP);
            wait(120);
        }
        assert_eq!(page.peek::<Page>().off, 0, "↑ 没回到页首");
        for _ in 0..40 {
            key(plat::KEY_DOWN);
            wait(120);
        }
        assert_eq!(page.peek::<Page>().off, ph - view, "↓ 没滚到底");
        // 操作栏连同区域底下一截，截下来看样子
        let (m, shot) = plat::capture_cursor_monitor().unwrap();
        let area = Rect { left: rect.left - 40 - m.rect.left, top: rect.bottom - 60 - m.rect.top, right: br.right + 40 - m.rect.left, bottom: br.bottom + 20 - m.rect.top };
        std::fs::write(dir.join("scene.png"), plat::encode(&shot.crop(&area), false).unwrap()).unwrap();
        // 拼出来的就是整页（右边滚动条那截不比）
        let (w, h, out) = SESSION.with_borrow(|s| {
            let st = lock(&s.as_ref().unwrap().stitch);
            let mut out = vec![0u8; st.width() * st.height() * 4];
            st.write(&mut out);
            (st.width(), st.height(), out)
        });
        std::fs::write(dir.join("long.png"), plat::encode(&plat::Image { w: w as i32, h: h as i32, px: out.clone() }, false).unwrap()).unwrap();
        assert_eq!((w, h), (pw as usize, ph as usize));
        let cols = (w - ((24.0 * mon.scale()).round() as usize).min(w / 4)) * 4;
        for y in 0..h {
            assert!(out[y * w * 4..][..cols] == expect[y * w * 4..][..cols], "第 {y} 行不对");
        }
        let pins = Win::all(plat::Kind::Pin).len();
        finish();
        assert!(!active() && !bar.alive());
        // 原位置出长图钉图（太高缩到工作区里）和结果条
        assert_eq!(Win::all(plat::Kind::Pin).len(), pins + 1);
        let pin = *Win::all(plat::Kind::Pin).last().unwrap();
        let r = pin.rect();
        assert!(r.left == rect.left && r.top >= mon.work.top && r.bottom <= mon.work.bottom, "{r:?}");
        assert!(!Win::all(plat::Kind::PinBar).is_empty(), "该有结果条");
        pin.close();
        page.close();
        wait(100);
    }

    #[test]
    fn no_scroll_is_whole_frame() {
        let mut s = Stitch::new(W, H, SKIP);
        feed(&mut s, 300, false);
        assert_eq!(s.height(), H);
        let mut out = vec![0u8; W * H * 4];
        s.write(&mut out);
        let (f, stride) = frame(300, false);
        let mut want = pack(&f, stride, W * 4, 0..H);
        want.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
        assert!(out == want);
    }
}
