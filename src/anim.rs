//! 录屏动图：边录边编 GIF，钉图播放时逐帧解码。只做计算和文件读写，不依赖界面和平台层。
//!
//! 编码：每一帧只编码和画面现在的样子不同的外接框，框里没变的像素设成透明（叠在上一帧上，LZW 也压得更小）；
//! 框里不超过 255 色时按原色建调色板（界面截图大多如此，不失真），超过时用 NeuQuant 量化；透明固定占最后一格。
//! 帧延迟按时间戳算，换成 1/100 秒时误差不累积；画面没变就不出新帧，只让上一帧停得更久。

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// 往文件里写，顺便数写了多少字节（控制条上实时显示大小）
struct Counted {
    f: BufWriter<File>,
    n: Arc<AtomicU64>,
}

impl Write for Counted {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let k = self.f.write(b)?;
        self.n.fetch_add(k as u64, Ordering::Relaxed);
        Ok(k)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.f.flush()
    }
}

pub struct Writer {
    enc: gif::Encoder<Counted>,
    w: usize,
    h: usize,
    /// 画面现在的样子（0x00RRGGBB）；还没有帧时是 u32::MAX，和任何颜色都不同
    shown: Vec<u32>,
    /// 编好、还没写出去的那一帧：下一帧来了才知道它要停多久
    pending: Option<gif::Frame<'static>>,
    /// 第一帧的时间（之后都相对它算），最近一次 push 的时间和当时的钟
    t0: Option<Duration>,
    last: Option<(Duration, Instant)>,
    /// 已经写出去的帧一共停了多久（1/100 秒）
    shown_cs: u64,
    frames: u32,
    bytes: Arc<AtomicU64>,
}

fn other(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

impl Writer {
    /// 在 `path` 新建 w × h 的动图（无限循环）
    pub fn create(path: &Path, w: i32, h: i32) -> io::Result<Writer> {
        let (Ok(gw), Ok(gh)) = (u16::try_from(w), u16::try_from(h)) else { return Err(other("录制区域太大")) };
        if gw == 0 || gh == 0 {
            return Err(other("录制区域是空的"));
        }
        let bytes = Arc::new(AtomicU64::new(0));
        let out = Counted { f: BufWriter::new(File::create(path)?), n: bytes.clone() };
        let mut enc = gif::Encoder::new(out, gw, gh, &[]).map_err(other)?;
        enc.set_repeat(gif::Repeat::Infinite).map_err(other)?;
        let (w, h) = (w as usize, h as usize);
        Ok(Writer { enc, w, h, shown: vec![u32::MAX; w * h], pending: None, t0: None, last: None, shown_cs: 0, frames: 0, bytes })
    }

    /// 已经写进文件的字节数，别的线程也能读
    pub fn bytes(&self) -> Arc<AtomicU64> {
        self.bytes.clone()
    }

    /// 加一帧：`px` 是 BGRA（alpha 不看），每行 `stride` 字节，`t` 是截下它的时间。和画面现在的样子一样就不出新帧
    pub fn push(&mut self, px: &[u8], stride: usize, t: Duration) -> io::Result<()> {
        let (w, h) = (self.w, self.h);
        if stride < w * 4 || px.len() < (h - 1) * stride + w * 4 {
            return Err(other("帧的大小不对"));
        }
        self.last = Some((t, Instant::now()));
        let color = |x: usize, y: usize| {
            let i = y * stride + x * 4;
            u32::from_le_bytes([px[i], px[i + 1], px[i + 2], 0])
        };
        // 变了的外接框
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for y in 0..h {
            let old = &self.shown[y * w..(y + 1) * w];
            let Some(l) = (0..w).find(|&x| color(x, y) != old[x]) else { continue };
            let r = (l..w).rev().find(|&x| color(x, y) != old[x]).unwrap_or(l);
            (x0, x1, y0, y1) = (x0.min(l), x1.max(r + 1), y0.min(y), y + 1);
        }
        if y1 == 0 {
            return Ok(());
        }
        let mut rgba = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
        for y in y0..y1 {
            for x in x0..x1 {
                let c = color(x, y);
                let s = &mut self.shown[y * w + x];
                if *s == c {
                    rgba.extend_from_slice(&[0, 0, 0, 0]);
                } else {
                    *s = c;
                    rgba.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]);
                }
            }
        }
        let mut frame = quantize(&rgba, (x1 - x0) as u16, (y1 - y0) as u16);
        (frame.left, frame.top, frame.dispose) = (x0 as u16, y0 as u16, gif::DisposalMethod::Keep);
        let t0 = *self.t0.get_or_insert(t);
        self.emit(t.saturating_sub(t0))?;
        self.pending = Some(frame);
        Ok(())
    }

    /// 等着的那帧停到 `until`（相对第一帧），写出去
    fn emit(&mut self, until: Duration) -> io::Result<()> {
        let Some(mut f) = self.pending.take() else { return Ok(()) };
        let target = (until.as_millis() as u64 + 5) / 10;
        // 浏览器把不到 2 的延迟当成 10，宁可多停一点，后面的帧会扣回来
        let delay = target.saturating_sub(self.shown_cs).clamp(2, u16::MAX as u64);
        f.delay = delay as u16;
        self.shown_cs += delay;
        self.enc.write_frame(&f).map_err(other)?;
        self.frames += 1;
        Ok(())
    }

    /// 写完收尾：最后一帧停到现在（按最近一次 push 以后过了多久算）。返回帧数；一帧都没有时报错
    pub fn finish(mut self) -> io::Result<u32> {
        let (Some(t0), Some((t, at))) = (self.t0, self.last) else { return Err(other("没有录到画面")) };
        self.emit((t + at.elapsed()).saturating_sub(t0))?;
        let mut out = self.enc.into_inner().map_err(other)?;
        out.flush()?;
        Ok(self.frames)
    }
}

/// RGBA（alpha 只有 0 和 255）→ 一帧。透明的像素用调色板最后一格
fn quantize(rgba: &[u8], w: u16, h: u16) -> gif::Frame<'static> {
    let key = |p: &[u8]| u32::from_le_bytes([p[0], p[1], p[2], 0]);
    let mut map: HashMap<u32, u8> = HashMap::new();
    let mut palette = Vec::new();
    let mut exact = true;
    // 界面上同色连成片，记住上一个颜色能省掉大半次查表
    let mut last = u32::MAX;
    for p in rgba.chunks_exact(4).filter(|p| p[3] != 0) {
        let c = key(p);
        if c == last || map.contains_key(&c) {
            last = c;
            continue;
        }
        if map.len() == 255 {
            exact = false;
            break;
        }
        map.insert(c, map.len() as u8);
        palette.extend_from_slice(&p[..3]);
        last = c;
    }
    let (buffer, transparent): (Vec<u8>, u8) = if exact {
        let t = map.len() as u8;
        let mut cache = (u32::MAX, 0u8);
        let idx = rgba
            .chunks_exact(4)
            .map(|p| {
                if p[3] == 0 {
                    return t;
                }
                let c = key(p);
                if c != cache.0 {
                    cache = (c, map[&c]);
                }
                cache.1
            })
            .collect();
        (idx, t)
    } else {
        let opaque: Vec<u8> = rgba.chunks_exact(4).filter(|p| p[3] != 0).flatten().copied().collect();
        // NeuQuant 隔几个像素学一个：小块要逐个学才认得全颜色，大块用快一档的采样，录制线程跟得上帧率
        let speed = match opaque.len() / 4 {
            n if n < 100_000 => 1,
            n if n < 400_000 => 10,
            _ => 20,
        };
        let nq = color_quant::NeuQuant::new(speed, 255, &opaque);
        palette = nq.color_map_rgb();
        (rgba.chunks_exact(4).map(|p| if p[3] == 0 { 255 } else { nq.index_of(p) as u8 }).collect(), 255)
    };
    palette.extend_from_slice(&[0, 0, 0]);
    gif::Frame { width: w, height: h, buffer: Cow::Owned(buffer), palette: Some(palette), transparent: Some(transparent), ..gif::Frame::default() }
}

/// 逐帧播放动图：每次把下一帧叠到调用方的画面上。帧解成调色板索引（每像素 1 字节）放进复用的缓冲，
/// 按调色板直接叠上去，不另外分配一整帧的 RGBA：钉图常驻的只有画面本身那一帧和这 1/4 帧
pub struct Player {
    path: PathBuf,
    dec: gif::Decoder<BufReader<File>>,
    pub w: i32,
    pub h: i32,
    /// 上一帧要求恢复成背景的那块（x, y, w, h）；Jie 自己录的不会用到
    clear: Option<(usize, usize, usize, usize)>,
    /// 当前帧的调色板索引
    buf: Vec<u8>,
}

fn decoder(path: &Path) -> Result<gif::Decoder<BufReader<File>>, String> {
    let mut o = gif::DecodeOptions::new();
    o.set_color_output(gif::ColorOutput::Indexed);
    o.read_info(BufReader::new(File::open(path).map_err(|e| e.to_string())?)).map_err(|e| e.to_string())
}

impl Player {
    pub fn open(path: &Path) -> Result<Player, String> {
        let dec = decoder(path)?;
        let (w, h) = (dec.width() as i32, dec.height() as i32);
        Ok(Player { path: path.to_path_buf(), dec, w, h, clear: None, buf: Vec::new() })
    }

    /// 下一帧叠进 `out`（BGRA，自上而下，宽高同动图），返回这一帧要停多久；播完了从头再来
    pub fn next(&mut self, out: &mut [u8]) -> Result<Duration, String> {
        let cw = self.w as usize;
        if let Some((x, y, w, h)) = self.clear.take() {
            for row in y..y + h {
                out[(row * cw + x) * 4..(row * cw + x + w) * 4].chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&[0, 0, 0, 255]));
            }
        }
        let mut restarted = false;
        let f = loop {
            match self.dec.next_frame_info().map_err(|e| e.to_string())? {
                Some(f) => break (f.left as usize, f.top as usize, f.width as usize, f.height as usize, f.transparent, f.dispose, f.delay),
                None if !restarted => {
                    self.dec = decoder(&self.path)?;
                    restarted = true;
                }
                None => return Err("动图里没有画面".into()),
            }
        };
        let (fx, fy, fw, fh, transparent, dispose, delay) = f;
        self.buf.resize(self.dec.buffer_size(), 0);
        self.dec.read_into_buffer(&mut self.buf).map_err(|e| e.to_string())?;
        let pal = self.dec.palette().map_err(|e| e.to_string())?;
        // 帧超出画布的部分不画；透明的索引和调色板外的索引都不画
        let (w, h) = (fw.min(cw.saturating_sub(fx)), fh.min((self.h as usize).saturating_sub(fy)));
        for y in 0..h {
            let src = &self.buf[y * fw..y * fw + w];
            let dst = &mut out[((fy + y) * cw + fx) * 4..((fy + y) * cw + fx + w) * 4];
            for (d, &i) in dst.chunks_exact_mut(4).zip(src) {
                if Some(i) != transparent
                    && let Some(c) = pal.get(i as usize * 3..i as usize * 3 + 3)
                {
                    d.copy_from_slice(&[c[2], c[1], c[0], 255]);
                }
            }
        }
        if dispose == gif::DisposalMethod::Background {
            self.clear = Some((fx, fy, w, h));
        }
        // 和浏览器一样：不到 2 的延迟按 10 算
        let cs = if delay < 2 { 10 } else { delay };
        Ok(Duration::from_millis(cs as u64 * 10))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        // 放在 target 下：项目目录是低完整性的，从这里跑的测试写不了 %TEMP%
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("anim").join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// w × h 的 BGRA 帧，颜色由 f(x, y) 给（0xRRGGBB）；alpha 是 0（截屏出来的 alpha 不可靠，编码时不看）
    fn frame(w: usize, h: usize, f: impl Fn(usize, usize) -> u32) -> Vec<u8> {
        (0..w * h).flat_map(|i| (f(i % w, i / w) & 0xFF_FFFF).to_le_bytes()).collect()
    }

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    /// 解码出每一帧叠好的画面（RGB 0xRRGGBB）和延迟（毫秒），直到绕回第一帧
    fn play(path: &Path, frames: usize) -> Vec<(Vec<u32>, u64)> {
        let mut p = Player::open(path).unwrap();
        let mut canvas = vec![0u8; (p.w * p.h * 4) as usize];
        (0..frames)
            .map(|_| {
                let d = p.next(&mut canvas).unwrap();
                (canvas.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], 0])).collect(), d.as_millis() as u64)
            })
            .collect()
    }

    fn rgb(px: &[u8]) -> Vec<u32> {
        px.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], 0])).collect()
    }

    /// 4K 全屏录下来的动图，钉图每换一帧在界面线程上解码要多久（整屏都变、只变一块两种）。
    /// 平时不跑：cargo test --release decode_4k_cost -- --ignored --nocapture
    #[test]
    #[ignore]
    fn decode_4k_cost() {
        let (w, h) = (3840usize, 2160usize);
        let path = dir("cost").join("4k.gif");
        let mut wr = Writer::create(&path, w as i32, h as i32).unwrap();
        let t0 = Instant::now();
        for i in 0..8usize {
            // 前 4 帧整屏都变（滚动），后 4 帧只变左上一块（打字）
            let f = frame(w, h, |x, y| if i < 4 || (x < 600 && y < 200) { ((x / 7 + i * 13) as u32) << 16 | ((y / 5 + i) as u32 & 0xFF) << 8 | ((x + y) / 11) as u32 & 0xFF } else { 0x202020 });
            wr.push(&f, w * 4, ms(i as u64 * 66)).unwrap();
        }
        wr.finish().unwrap();
        println!("编码 8 帧 {:?}，{} KB", t0.elapsed(), std::fs::metadata(&path).unwrap().len() / 1024);
        let mut p = Player::open(&path).unwrap();
        let mut canvas = vec![0u8; w * h * 4];
        for i in 0..8 {
            let t = Instant::now();
            p.next(&mut canvas).unwrap();
            println!("第 {i} 帧解码 {:?}", t.elapsed());
        }
    }

    /// 不超过 255 色：逐像素无损；没变的帧不出；只有变了的那块进新帧；延迟按时间戳；播完从头再来
    #[test]
    fn exact_frames_round_trip() {
        let (w, h) = (64, 48);
        let a = frame(w, h, |x, y| ((x / 8) as u32 * 30) << 16 | ((y / 8) as u32 * 40) << 8 | 0x10);
        let mut b = a.clone();
        for y in 10..20 {
            for x in 30..40 {
                b[(y * w + x) * 4..][..3].copy_from_slice(&[0xEC, 0x44, 0x9C]);
            }
        }
        let mut d = b.clone();
        d[(47 * w + 63) * 4..][..3].copy_from_slice(&[1, 2, 3]);
        let path = dir("exact").join("a.gif");
        let mut wr = Writer::create(&path, w as i32, h as i32).unwrap();
        for (px, t) in [(&a, 40), (&b, 140), (&b, 240), (&d, 340)] {
            wr.push(px, w * 4, ms(t)).unwrap();
        }
        let bytes = wr.bytes();
        assert_eq!(wr.finish().unwrap(), 3);
        assert_eq!(bytes.load(Ordering::Relaxed), std::fs::metadata(&path).unwrap().len());
        let out = play(&path, 4);
        assert_eq!(out[0].0, rgb(&a));
        assert_eq!(out[1].0, rgb(&b));
        assert_eq!(out[2].0, rgb(&d));
        assert_eq!((out[0].1, out[1].1, out[2].1), (100, 200, 20));
        // 绕回第一帧：第一帧是整幅，盖掉后面帧留下的东西
        assert_eq!(out[3].0, rgb(&a));
    }

    /// 超过 255 色：量化后颜色接近原色；之后只变一小块时，没变的地方和上一帧完全一样
    #[test]
    fn quantized_frames_keep_unchanged_pixels() {
        let (w, h) = (64, 64);
        let a = frame(w, h, |x, y| (x as u32 * 4) << 16 | (y as u32 * 4) << 8 | 0x80);
        let mut b = a.clone();
        b[(5 * w + 5) * 4..][..3].copy_from_slice(&[0, 0, 0]);
        let path = dir("quant").join("q.gif");
        let mut wr = Writer::create(&path, w as i32, h as i32).unwrap();
        wr.push(&a, w * 4, ms(0)).unwrap();
        wr.push(&b, w * 4, ms(70)).unwrap();
        assert_eq!(wr.finish().unwrap(), 2);
        let out = play(&path, 2);
        let err = |p: u32, q: u32| (0..3).map(|k| ((p >> (k * 8)) as u8).abs_diff((q >> (k * 8)) as u8)).max().unwrap();
        let errs: Vec<u8> = out[0].0.iter().zip(rgb(&a)).map(|(p, q)| err(*p, q)).collect();
        let (worst, mean) = (*errs.iter().max().unwrap(), errs.iter().map(|e| *e as f32).sum::<f32>() / errs.len() as f32);
        // 4096 色挤进 255 色：均匀网格每通道最多差 8；NeuQuant 实测平均 8.8、最大 30，这里只防它变差
        assert!(mean <= 10.0 && worst <= 40, "量化误差平均 {mean:.1}，最大 {worst}");
        let changed = 5 * w + 5;
        for (i, (p, q)) in out[1].0.iter().zip(&out[0].0).enumerate() {
            if i == changed {
                assert_eq!(*p, 0);
            } else {
                assert_eq!(p, q, "像素 {i} 不该变");
            }
        }
        assert_eq!(out[0].1, 70);
    }

    /// 一直不变：只有一帧；一帧都没有：报错
    #[test]
    fn static_and_empty() {
        let (w, h) = (16, 8);
        let a = frame(w, h, |x, _| x as u32);
        let path = dir("static").join("s.gif");
        let mut wr = Writer::create(&path, w as i32, h as i32).unwrap();
        for t in 0..10 {
            wr.push(&a, w * 4, ms(t * 66)).unwrap();
        }
        assert_eq!(wr.finish().unwrap(), 1);
        assert!(Writer::create(&path, w as i32, h as i32).unwrap().finish().is_err());
    }

    /// 时间戳换成 1/100 秒的误差不累积：15 fps 每帧都变，延迟加起来等于总时长
    #[test]
    fn delays_add_up() {
        let (w, h) = (8, 8);
        let path = dir("delays").join("d.gif");
        let mut wr = Writer::create(&path, w as i32, h as i32).unwrap();
        let n = 46;
        for i in 0..n {
            wr.push(&frame(w, h, |_, _| i as u32 * 5), w * 4, Duration::from_secs_f64(i as f64 / 15.0)).unwrap();
        }
        wr.finish().unwrap();
        let total: u64 = play(&path, n - 1).iter().map(|f| f.1).sum();
        assert_eq!(total, 3000, "前 {} 帧应该正好 3 秒", n - 1);
    }
}
