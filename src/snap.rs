//! 吸附：在冻结的截图上找围住鼠标的矩形边界（面板、卡片、输入框、侧栏、表格格子、图片），给遮罩一层层地高亮。
//! 只看像素，不问系统，两端共用，不需要辅助功能权限；不依赖别的模块。
//!
//! 做法：从鼠标所在的点往上下左右走，碰到「长直边」（相邻两行或两列颜色差得明显，而且连成足够长的一段）就记成候选边界，
//! 每个方向留最近的几条，再加上窗口自己的边；上下左右各取一条拼成矩形，四条边都大半被真实的边盖住才算数。
//! 文字的笔画短，连不成长边，所以穿过文字不会停下。结果从小到大、一层套一层。

use crate::plat::{Point, Rect};

/// 相邻像素任一通道差到这么多就算有边（浅色界面上 #F3F3F3 和 #FFFFFF 这种分隔也认得出）
const EDGE: u8 = 10;
/// 每个方向最多留几条候选边（不算窗口的边）
const PER_SIDE: usize = 6;
/// 矩形每条边要被真实的边盖住多少：只许两头各缺一个圆角（8 DIP），中间断开的加起来不超过一成
const COVER: f32 = 0.9;
const CORNER: f32 = 8.0;
/// 边界差几个像素以内算同一层（1 像素的描边，里外两条边都会被找到）
const SAME: i32 = 2;
/// 最多给几层
const MAX_LEVELS: usize = 8;

struct Img<'a> {
    px: &'a [u8],
    w: i32,
    h: i32,
}

impl Img<'_> {
    fn differ(&self, (ax, ay): (i32, i32), (bx, by): (i32, i32)) -> bool {
        let at = |x: i32, y: i32| ((y * self.w + x) * 4) as usize;
        let (a, b) = (at(ax, ay), at(bx, by));
        (0..3).any(|k| self.px[a + k].abs_diff(self.px[b + k]) >= EDGE)
    }

    /// 边界 `b` 上、位置 `pos` 处有没有边。横线（`hor`）的边界 b 在第 b-1 行和第 b 行之间，pos 是列；竖线反过来
    fn line(&self, hor: bool, b: i32, pos: i32) -> bool {
        if hor {
            b >= 1 && b < self.h && pos >= 0 && pos < self.w && self.differ((pos, b - 1), (pos, b))
        } else {
            b >= 1 && b < self.w && pos >= 0 && pos < self.h && self.differ((b - 1, pos), (b, pos))
        }
    }

    /// 边界 `b` 上从 `p0` 往两头连成的边够不够 `need` 长（允许中间断一个像素），只在 [lo, hi) 里数
    fn long(&self, hor: bool, b: i32, p0: i32, lo: i32, hi: i32, need: i32) -> bool {
        let mut n = 1;
        for dir in [-1, 1] {
            let (mut q, mut gap) = (p0, 0);
            loop {
                q += dir;
                if q < lo || q >= hi {
                    break;
                }
                if self.line(hor, b, q) {
                    n += 1 + gap;
                    gap = 0;
                    if n >= need {
                        return true;
                    }
                } else {
                    gap += 1;
                    if gap > 1 {
                        break;
                    }
                }
            }
        }
        n >= need
    }
}

/// 一条候选边界：位置，和沿着它的覆盖计数（前缀和，从 `lo` 开始）；窗口的边没有计数，算全覆盖
struct Cand {
    b: i32,
    cum: Option<Vec<u32>>,
    lo: i32,
}

impl Cand {
    /// [a, b) 这段够不够当矩形的一条边：圆角缺的两头不算，见 COVER
    fn covers(&self, a: i32, b: i32, corner: f32) -> bool {
        let Some(c) = &self.cum else { return true };
        let (i, j) = ((a - self.lo) as usize, (b - self.lo) as usize);
        let span = (b - a) as f32;
        (c[j] - c[i]) as f32 >= (span * COVER).min(span - 2.0 * corner)
    }
}

/// 围住 `p` 的矩形边界，从小到大、后一个包住前一个，都在 `within`（通常是鼠标下的窗口）里面、不含 `within` 本身。
/// `px` 是 w × h 的 BGRA 截图，`scale` 是显示器的缩放比例
pub fn boxes(px: &[u8], w: i32, h: i32, p: Point, within: Rect, scale: f32) -> Vec<Rect> {
    let img = Img { px, w, h };
    let a = Rect { left: within.left.max(0), top: within.top.max(0), right: within.right.min(w), bottom: within.bottom.min(h) };
    if px.len() < (w * h * 4) as usize || p.x < a.left || p.x >= a.right || p.y < a.top || p.y >= a.bottom {
        return Vec::new();
    }
    // 长边至少 16 DIP：正文的笔画比这短
    let need = (16.0 * scale).round().max(8.0) as i32;
    let corner = CORNER * scale;
    // 一个方向上的候选：从 p 往外走，`bs` 是依次经过的边界，`pos` 是 p 在边线方向上的位置，`edge` 是窗口那条边
    let side = |hor: bool, bs: &mut dyn Iterator<Item = i32>, pos: i32, edge: i32| -> Vec<Cand> {
        let (lo, hi) = if hor { (a.left, a.right) } else { (a.top, a.bottom) };
        let mut v: Vec<Cand> = bs
            .filter(|&b| img.line(hor, b, pos) && img.long(hor, b, pos, lo, hi, need))
            .take(PER_SIDE)
            .map(|b| {
                // 差一个像素也算盖住：抗锯齿的边常常糊成两行
                let mut cum = Vec::with_capacity((hi - lo + 1) as usize);
                cum.push(0);
                let mut n = 0;
                for q in lo..hi {
                    n += u32::from(img.line(hor, b, q) || img.line(hor, b - 1, q) || img.line(hor, b + 1, q));
                    cum.push(n);
                }
                Cand { b, cum: Some(cum), lo }
            })
            .collect();
        v.push(Cand { b: edge, cum: None, lo });
        v
    };
    let tops = side(true, &mut (a.top + 1..=p.y).rev(), p.x, a.top);
    let bottoms = side(true, &mut (p.y + 1..a.bottom), p.x, a.bottom);
    let lefts = side(false, &mut (a.left + 1..=p.x).rev(), p.y, a.left);
    let rights = side(false, &mut (p.x + 1..a.right), p.y, a.right);

    let mut found = Vec::new();
    for t in &tops {
        for b in &bottoms {
            if b.b - t.b < need {
                continue;
            }
            for l in &lefts {
                for r in &rights {
                    // 四条都是窗口的边就是窗口本身，不算
                    if r.b - l.b < need || (t.cum.is_none() && b.cum.is_none() && l.cum.is_none() && r.cum.is_none()) {
                        continue;
                    }
                    let ok = t.covers(l.b, r.b, corner) && b.covers(l.b, r.b, corner) && l.covers(t.b, b.b, corner) && r.covers(t.b, b.b, corner);
                    if ok {
                        found.push(Rect { left: l.b, top: t.b, right: r.b, bottom: b.b });
                    }
                }
            }
        }
    }
    nest(found)
}

/// 默认高亮哪一层（`chain` 从小到大）：不超过窗口七成大、短边至少 120 DIP 的里面最大的那个，也就是主要的面板、侧栏、编辑区。
/// 没有就返回 None（用窗口本身）。按钮、输入框、列表项这些小块要滚轮往里换
pub fn default_level(chain: &[Rect], win: &Rect, scale: f32) -> Option<usize> {
    let (max, min_side) = (area(win) as f64 * 0.7, (120.0 * scale) as i32);
    chain.iter().rposition(|r| area(r) as f64 <= max && (r.right - r.left).min(r.bottom - r.top) >= min_side)
}

fn area(r: &Rect) -> i64 {
    (r.right - r.left) as i64 * (r.bottom - r.top) as i64
}

/// 边界都只差几个像素：算同一层
pub fn same(a: &Rect, b: &Rect) -> bool {
    (a.left - b.left).abs() <= SAME && (a.top - b.top).abs() <= SAME && (a.right - b.right).abs() <= SAME && (a.bottom - b.bottom).abs() <= SAME
}

/// 从最小的开始，每次取包住上一层的最小的那个，串成一层套一层；同一层留大的那个（带上 1 像素的描边）
fn nest(mut v: Vec<Rect>) -> Vec<Rect> {
    // 按面积从小到大（稳定）。框只有几十个：插入排序就够，不用把标准库的通用排序拉进来（发布版省 6 KB）
    for i in 1..v.len() {
        let mut j = i;
        while j > 0 && area(&v[j - 1]) > area(&v[j]) {
            v.swap(j - 1, j);
            j -= 1;
        }
    }
    let mut out: Vec<Rect> = Vec::new();
    for r in v {
        let Some(last) = out.last() else {
            out.push(r);
            continue;
        };
        if same(last, &r) {
            *out.last_mut().unwrap() = r;
        } else if r.left <= last.left + SAME && r.top <= last.top + SAME && r.right >= last.right - SAME && r.bottom >= last.bottom - SAME {
            if out.len() == MAX_LEVELS {
                break;
            }
            out.push(r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// w × h 的白底画布，可以填矩形
    struct Canvas {
        w: i32,
        h: i32,
        px: Vec<u8>,
    }

    impl Canvas {
        fn new(w: i32, h: i32) -> Canvas {
            Canvas { w, h, px: vec![0xFF; (w * h * 4) as usize] }
        }

        fn fill(&mut self, l: i32, t: i32, r: i32, b: i32, c: u32) {
            for y in t..b {
                for x in l..r {
                    let i = ((y * self.w + x) * 4) as usize;
                    self.px[i..i + 4].copy_from_slice(&(c | 0xFF00_0000).to_le_bytes());
                }
            }
        }

        /// 1 像素描边的框，`corner` 是四个角各缺掉的长度（像圆角）
        fn frame(&mut self, l: i32, t: i32, r: i32, b: i32, c: u32, corner: i32) {
            self.fill(l + corner, t, r - corner, t + 1, c);
            self.fill(l + corner, b - 1, r - corner, b, c);
            self.fill(l, t + corner, l + 1, b - corner, c);
            self.fill(r - 1, t + corner, r, b - corner, c);
        }

        /// 一片「文字」：深色小块，笔画都短
        fn text(&mut self, l: i32, t: i32, r: i32, b: i32) {
            for y in (t..b - 10).step_by(16) {
                for x in (l..r - 6).step_by(9) {
                    self.fill(x, y, x + 6, y + 10, 0x202020);
                }
            }
        }

        fn boxes(&self, x: i32, y: i32) -> Vec<Rect> {
            boxes(&self.px, self.w, self.h, Point { x, y }, Rect { left: 0, top: 0, right: self.w, bottom: self.h }, 1.0)
        }
    }

    fn rect(l: i32, t: i32, r: i32, b: i32) -> Rect {
        Rect { left: l, top: t, right: r, bottom: b }
    }

    /// 带 1 像素灰边的卡片，里面全是字：认出整张卡片（连描边）
    #[test]
    fn card_with_text() {
        let mut c = Canvas::new(400, 300);
        c.frame(50, 40, 250, 200, 0xDDDDDD, 0);
        c.text(60, 50, 240, 190);
        assert_eq!(c.boxes(120, 100), vec![rect(50, 40, 250, 200)]);
        // 卡片外面的空白：没有边界
        assert!(c.boxes(320, 250).is_empty());
    }

    /// 卡片里一块浅灰底的面板（没有描边），面板外面的卡片也是一层；圆角也认得出
    #[test]
    fn nested_panels_and_round_corners() {
        let mut c = Canvas::new(400, 300);
        c.frame(50, 40, 300, 260, 0xCCCCCC, 6);
        c.fill(70, 60, 200, 160, 0xF3F3F3);
        c.text(80, 70, 190, 150);
        assert_eq!(c.boxes(100, 100), vec![rect(70, 60, 200, 160), rect(50, 40, 300, 260)]);
        assert_eq!(c.boxes(250, 200), vec![rect(50, 40, 300, 260)]);
    }

    /// 表格：每个格子是一层，整张表是外面一层
    #[test]
    fn table_cells() {
        let mut c = Canvas::new(400, 300);
        for x in [40, 140, 240] {
            c.fill(x, 40, x + 1, 241, 0xBBBBBB);
        }
        for y in [40, 140, 240] {
            c.fill(40, y, 241, y + 1, 0xBBBBBB);
        }
        let v = c.boxes(190, 90);
        assert_eq!(v.first(), Some(&rect(140, 40, 241, 141)), "{v:?}");
        assert_eq!(v.last(), Some(&rect(40, 40, 241, 241)), "{v:?}");
    }

    /// 平滑渐变、只有字的地方：什么都不认
    #[test]
    fn gradient_and_plain_text() {
        let mut c = Canvas::new(300, 200);
        for x in 0..300 {
            c.fill(x, 0, x + 1, 200, (x as u32 / 2) * 0x010101);
        }
        assert!(c.boxes(150, 100).is_empty());
        let mut t = Canvas::new(300, 200);
        t.text(20, 20, 280, 180);
        assert!(t.boxes(150, 100).is_empty());
    }

    fn sample_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("snap")
    }

    /// 截下鼠标所在的显示器存成样本（原始 BGRA + 窗口列表），给 real_screens 反复用。
    /// 会截到屏幕内容，平时不跑：cargo test dump_screen -- --ignored
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn dump_screen() {
        let (mon, bmp) = crate::plat::capture_cursor_monitor().unwrap();
        let dir = sample_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let n = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "bgra")).count();
        let wins: Vec<String> = crate::plat::window_rects(&mon).iter().map(|r| format!("{} {} {} {}", r.left, r.top, r.right, r.bottom)).collect();
        std::fs::write(dir.join(format!("{n}.txt")), format!("{} {} {}\n{}", bmp.w, bmp.h, mon.scale(), wins.join("\n"))).unwrap();
        std::fs::write(dir.join(format!("{n}.bgra")), bmp.pixels()).unwrap();
    }

    /// 样本上每隔一段取一个点，打印一层层的框（占窗口面积的比例），把默认那层画成图 n-out.png 看。
    /// 平时不跑：cargo test real_screens -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_screens() {
        let dir = sample_dir();
        let mut n = 0;
        while let Ok(meta) = std::fs::read_to_string(dir.join(format!("{n}.txt"))) {
            let mut lines = meta.lines();
            let head: Vec<f32> = lines.next().unwrap().split(' ').map(|x| x.parse().unwrap()).collect();
            let (w, h, scale) = (head[0] as i32, head[1] as i32, head[2]);
            let wins: Vec<Rect> = lines
                .map(|l| {
                    let v: Vec<i32> = l.split(' ').map(|x| x.parse().unwrap()).collect();
                    rect(v[0], v[1], v[2], v[3])
                })
                .collect();
            let px = std::fs::read(dir.join(format!("{n}.bgra"))).unwrap();
            let mut out = px.clone();
            let full = rect(0, 0, w, h);
            let step = (w / 9).max(1);
            let mut k = 0u32;
            for y in (step / 2..h).step_by(step as usize) {
                for x in (step / 2..w).step_by(step as usize) {
                    let p = Point { x, y };
                    let win = wins.iter().find(|r| p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom).copied().unwrap_or(full);
                    let t = std::time::Instant::now();
                    let v = boxes(&px, w, h, p, win, scale);
                    let warea = area(&win) as f32;
                    let desc: Vec<String> = v.iter().map(|r| format!("{}x{}({:.0}%)", r.right - r.left, r.bottom - r.top, area(r) as f32 / warea * 100.0)).collect();
                    println!("{n} ({x},{y}) win {}x{} {:?}: {}", win.right - win.left, win.bottom - win.top, t.elapsed(), desc.join(" < "));
                    let pick = default_level(&v, &win, scale).map_or(win, |i| v[i]);
                    // 每个点一种颜色：框和点
                    k += 1;
                    let c = [0x00E0_3030u32, 0x0030_B030, 0x00D0_8000, 0x0090_30D0, 0x0000_A0A0][(k % 5) as usize].to_le_bytes();
                    let mut put = |x: i32, y: i32| {
                        if x >= 0 && y >= 0 && x < w && y < h {
                            let i = ((y * w + x) * 4) as usize;
                            out[i..i + 3].copy_from_slice(&c[..3]);
                        }
                    };
                    for d in -4..=4 {
                        for e in -4..=4 {
                            put(x + d, y + e);
                        }
                    }
                    for t in 0..2 {
                        for xx in pick.left..pick.right {
                            put(xx, pick.top + t);
                            put(xx, pick.bottom - 1 - t);
                        }
                        for yy in pick.top..pick.bottom {
                            put(pick.left + t, yy);
                            put(pick.right - 1 - t, yy);
                        }
                    }
                }
            }
            let img = crate::plat::Image { w, h, px: out };
            std::fs::write(dir.join(format!("{n}-out.png")), crate::plat::encode(&img, false).unwrap()).unwrap();
            n += 1;
        }
        assert!(n > 0, "先跑 dump_screen");
    }

    /// 默认层：小块（列表项、按钮）跳过，太大的（快和窗口一样大）也跳过，取中间最大的；都不合适就是窗口
    #[test]
    fn default_prefers_main_panel() {
        let win = rect(0, 0, 1000, 800);
        let chain = [rect(100, 100, 400, 140), rect(100, 100, 400, 700), rect(100, 0, 1000, 800)];
        assert_eq!(default_level(&chain, &win, 1.0), Some(1));
        assert_eq!(default_level(&chain[..1], &win, 1.0), None);
        assert_eq!(default_level(&[], &win, 1.0), None);
        // 高缩放下 120 DIP 更大：300 × 150 在 2 倍屏上算小块
        assert_eq!(default_level(&[rect(0, 0, 300, 150)], &win, 2.0), None);
    }

    /// 都围着同一个点：同一层留大的；比上一层大却没包住它的跳过
    #[test]
    fn nest_keeps_outer_of_same_level() {
        let v = nest(vec![rect(10, 10, 50, 50), rect(11, 11, 49, 49), rect(0, 0, 100, 100), rect(20, 0, 70, 45)]);
        assert_eq!(v, vec![rect(10, 10, 50, 50), rect(0, 0, 100, 100)]);
    }
}
