//! 取字入口：把截图发给 OpenAI 兼容的看图模型（请求在 translate.rs），本机不识别。

use crate::doc;
use crate::plat::{self, Image, Rect};
use crate::translate;
use std::time::Instant;

pub enum Outcome {
    /// 识别出的文字（简化的 Markdown，见 doc.rs）、标题栏上的用时「1.4 秒」、从截图里裁出的图片（文字里写成 ![图片](#序号)）
    Text { text: String, took: String, images: Vec<Image> },
    Empty,
    Failed(String),
}

/// 服务会把大图缩到约这么多像素（DeepSeek 约 1300×1300），整屏小字缩完就认不清
const MAX_PX: i32 = 1300 * 1300;

/// 选区编成 PNG 发给 `cfg` 的服务，阻塞，要在工作线程里调。换行交给模型。
/// 超过 `MAX_PX` 的大图横着切成几条整宽的，同时发出去，结果按从上到下拼起来。
pub fn recognize(img: &Image, cfg: &translate::Config) -> Outcome {
    let t0 = Instant::now();
    // 编码在本线程按顺序做（WIC 每次进出临时 MTA），只有请求并发
    let mut pngs = Vec::new();
    let strips = strips(img);
    for &(y0, y1) in &strips {
        match plat::encode(&cut(img, Rect { left: 0, top: y0, right: img.w, bottom: y1 }), false) {
            Ok(p) => pngs.push(base64(&p)),
            Err(e) => return Outcome::Failed(e),
        }
    }
    let parts: Vec<_> = std::thread::scope(|sc| {
        let hs: Vec<_> = pngs.iter().map(|png| sc.spawn(move || translate::read_image(cfg, png, translate::OCR_TIMEOUT))).collect();
        hs.into_iter().map(|h| h.join().expect("识别请求线程")).collect()
    });
    let (mut text, mut images) = (String::new(), Vec::new());
    for (p, &(y0, y1)) in parts.into_iter().zip(&strips) {
        match p {
            Ok(t) if t.is_empty() => {}
            Ok(t) => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&cut_images(&t, img, y0, y1, &mut images));
            }
            Err(e) => return Outcome::Failed(e.to_string()),
        }
    }
    if text.is_empty() {
        Outcome::Empty
    } else {
        Outcome::Text { took: format!("{:.1} 秒", t0.elapsed().as_secs_f32()), text, images }
    }
}

/// 第 [y0, y1) 行那一条的结果里单占一行的 `![图片](左,上,右,下)`：按坐标从整张图里裁出来放进 `out`，
/// 换成 `![图片](#序号)`；坐标没给或不对的换成 `![图片]()`（卡片上画占位框）
fn cut_images(text: &str, img: &Image, y0: i32, y1: i32, out: &mut Vec<Image>) -> String {
    let lines: Vec<String> = text
        .lines()
        .map(|line| match doc::image_ref(line) {
            Some(inside) => match image_box(inside, img.w, y1 - y0) {
                Some(r) => {
                    out.push(cut(img, Rect { top: r.top + y0, bottom: r.bottom + y0, ..r }));
                    format!("![图片](#{})", out.len() - 1)
                }
                None => "![图片]()".into(),
            },
            None => line.into(),
        })
        .collect();
    lines.join("\n")
}

/// 「左,上,右,下」在 w × h 的图里的像素框，按 0–1000 的千分比算；不是四个数、太小（不到 8 像素见方）是 None
fn image_box(s: &str, w: i32, h: i32) -> Option<Rect> {
    let n: Vec<f32> = s.split([',', ' ', '[', ']']).filter(|t| !t.is_empty()).map(|t| t.parse().ok()).collect::<Option<_>>()?;
    let &[l, t, r, b] = &n[..] else { return None };
    // shortcut: 有的模型给像素，有数超过 1000 才认得出；小图上给像素会被当成千分比，碰上了再按模型分
    let (sx, sy) = if n.iter().any(|&v| v > 1000.0) { (1.0, 1.0) } else { (w as f32 / 1000.0, h as f32 / 1000.0) };
    let px = |v: f32, k: f32, max: i32| ((v * k).round() as i32).clamp(0, max);
    let r = Rect { left: px(l, sx, w), top: px(t, sy, h), right: px(r, sx, w), bottom: px(b, sy, h) };
    (r.right - r.left >= 8 && r.bottom - r.top >= 8).then_some(r)
}

/// 横着切成几条整宽的：每条不超过 `MAX_PX` 像素；切口在每条下沿往上 1/4 条高以内，挑和行首像素不一样的最少的那一行，尽量切在字行之间
fn strips(img: &Image) -> Vec<(i32, i32)> {
    let hmax = (MAX_PX / img.w.max(1)).max(32);
    let busy = |y: i32| {
        let row = &img.px[(y * img.w * 4) as usize..][..(img.w * 4) as usize];
        row.chunks_exact(4).filter(|p| p[..3] != row[..3]).count()
    };
    let (mut v, mut y0) = (Vec::new(), 0);
    while img.h - y0 > hmax {
        let cut = (y0 + hmax * 3 / 4..=y0 + hmax).min_by_key(|&y| busy(y)).unwrap();
        v.push((y0, cut));
        y0 = cut;
    }
    v.push((y0, img.h));
    v
}

/// r 那一块拷成一张图（r 在图里面）
fn cut(img: &Image, r: Rect) -> Image {
    let (w, row) = ((r.right - r.left) as usize * 4, img.w as usize * 4);
    let mut px = Vec::with_capacity(w * (r.bottom - r.top) as usize);
    for y in r.top as usize..r.bottom as usize {
        px.extend_from_slice(&img.px[y * row + r.left as usize * 4..][..w]);
    }
    Image { w: r.right - r.left, h: r.bottom - r.top, px }
}

/// 标准 base64，带 = 补齐。不用 WinRT 的 CryptographicBuffer：windows crate 全局缓存它的工厂，
/// 编码时临时进的 MTA 退出后第二次取字再用就崩（实测 STATUS_ACCESS_VIOLATION）
fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = c.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            s.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::plat::{Bitmap, Canvas, Face, Image, Text};
    use crate::ui::rgb;

    /// 把样张画成 100% 缩放下的屏幕文字（560 × 200 白底）
    pub(crate) fn render(face: Face, px: f32, text: &str) -> Image {
        render_on(face, px, text, 560, 200, (0x1A1A1A, 0xFFFFFF))
    }

    /// 画在 w × h、(字色, 底色) 的画布上，左上留 10 像素
    fn render_on(face: Face, px: f32, text: &str, w: i32, h: i32, (fg, bg): (u32, u32)) -> Image {
        let bmp = Bitmap::new(w, h).unwrap();
        let l = Text::wrapped(face, px, false, text, w as f32 - 20.0, Some(px * 1.5)).unwrap();
        {
            let c = Canvas::new(&bmp, bmp.full(), false);
            c.clear(rgb(bg));
            c.text(&l, 10.0, 10.0, rgb(fg));
        }
        bmp.crop(&bmp.full())
    }

    /// 字符错误率（%）：去掉空白后，编辑距离 ÷ 标准答案的字数
    fn cer(want: &str, got: &str) -> f64 {
        let strip = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<Vec<_>>();
        let (a, b) = (strip(want), strip(got));
        let mut d: Vec<usize> = (0..=b.len()).collect();
        for i in 1..=a.len() {
            let mut prev = d[0];
            d[0] = i;
            for j in 1..=b.len() {
                let t = d[j];
                d[j] = (d[j] + 1).min(d[j - 1] + 1).min(prev + (a[i - 1] != b[j - 1]) as usize);
                prev = t;
            }
        }
        100.0 * d[b.len()] as f64 / a.len() as f64
    }

    /// 4K 整屏：白底上每 30 行有 18 行「字」。切出的条首尾相接、每条不超过 MAX_PX，切口都落在空白行
    #[test]
    fn strips_cut_between_lines() {
        let (w, h) = (3840, 2160);
        let mut px = vec![255u8; (w * h * 4) as usize];
        for y in (0..h).filter(|y| y % 30 >= 6 && y % 30 < 24) {
            for x in (0..w).step_by(7) {
                px[((y * w + x) * 4) as usize..][..3].fill(0);
            }
        }
        let img = Image { w, h, px };
        let s = super::strips(&img);
        assert!(s.len() > 1 && s[0].0 == 0 && s.last().unwrap().1 == h);
        for (a, b) in s.windows(2).map(|p| (p[0], p[1])) {
            assert_eq!(a.1, b.0);
            assert!(a.1 % 30 < 6 || a.1 % 30 >= 24, "切在字上：第 {} 行", a.1);
        }
        assert!(s.iter().all(|&(y0, y1)| (y1 - y0) * w <= super::MAX_PX));
        // 小图不切
        assert_eq!(super::strips(&Image { w: 800, h: 600, px: vec![255; 800 * 600 * 4] }), vec![(0, 600)]);
    }

    /// 图片标记：千分比按这一条的宽高换成像素、加上这一条的顶边，从整张图里裁出来；像素坐标、坏坐标、太小的框
    #[test]
    fn images_cut_from_strip() {
        let (w, h) = (200, 300);
        let px = (0..w * h).flat_map(|i| [(i % w) as u8, (i / w) as u8, 0, 255]).collect();
        let img = Image { w, h, px };
        let mut out = Vec::new();
        let t = super::cut_images("上\n![图片](100, 0, 600, 500)\n ![x]([50,50,100,150]) \n![图片](1, 2)\n![图片](0,0,10,10)\n下", &img, 100, 300, &mut out);
        assert_eq!(t, "上\n![图片](#0)\n![图片](#1)\n![图片]()\n![图片]()\n下");
        // 这一条 200 × 200：(20, 0)–(120, 100)、(10, 10)–(20, 30)，在整张图里往下挪 100；像素的蓝、绿是它的 x、y
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].w, out[0].h, out[0].px[0], out[0].px[1]), (100, 100, 20, 100));
        assert_eq!((out[1].w, out[1].h, out[1].px[0], out[1].px[1]), (10, 20, 10, 110));
        let last = &out[1].px[out[1].px.len() - 4..];
        assert_eq!((last[0], last[1]), (19, 129));
        // 有数超过 1000：当像素
        assert_eq!(super::image_box("10,20,1500,40", 2000, 100), Some(crate::plat::Rect { left: 10, top: 20, right: 1500, bottom: 40 }));
        assert_eq!(super::image_box("a,b,c,d", 100, 100), None);
    }

    #[test]
    fn base64() {
        for (raw, b64) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy"), ("\u{ff}\u{fe}", "w7/Dvg==")] {
            assert_eq!(super::base64(raw.as_bytes()), b64, "{raw:?}");
        }
    }

    /// 连着认两次不崩（编码时每次都进出一回临时 MTA），图片按 PNG 发出去
    #[test]
    fn llm_twice() {
        use crate::translate::tests::{http, mock};
        let img = render(Face::Segoe, 15.0, "hello");
        for _ in 0..2 {
            let (base, req) = mock(Some(http("200 OK", r#"{"choices":[{"message":{"content":"hello"}}]}"#)));
            let cfg = crate::translate::Config { endpoint: base, model: "vl".into(), api_key: String::new(), target: crate::translate::Lang::Zh };
            assert!(matches!(super::recognize(&img, &cfg), super::Outcome::Text { ref text, .. } if text == "hello"));
            assert!(req.recv().unwrap().contains("data:image/png;base64,iVBORw0KGgo"));
        }
    }

    /// (字体, 字号, 原文, 宽, 高, (字色, 底色))
    type Sample = (Face, f32, &'static str, i32, i32, (u32, u32));

    /// 12 张样张（4.4 起用的）
    fn sample_cases() -> Vec<Sample> {
        let zh = "Jie 是一个 Windows 截图工具，全程用 CPU 渲染，空闲时私有内存不超过 15 MB，按快捷键马上出遮罩。选区之外压暗，拖动框选，松手后弹出操作栏；标注直接画在选区上，钉图可以继续编辑。取字和翻译共用一张文字卡片，不自动复制。";
        let code = "// 截取鼠标所在的显示器\npub fn grab(mon: &Monitor) -> Frame {\n    let dc = unsafe { GetDC(None) };\n    let (w, h) = (mon.width, mon.height);\n}";
        let en = "Most screenshot tools keep a GPU device alive all day.\nA frozen frame is only a bitmap.";
        let (yh, light, dark) = (Face::Cjk, (0x1A1A1A, 0xFFFFFF), (0xD4D4D4, 0x1E1E1E));
        vec![
            (yh, 14.0, "使用 cargo build 命令编译 Rust 项目，\n空闲内存目标：15 MB 以内。", 560, 200, light),
            (Face::Mono, 13.0, "// 截取鼠标所在的显示器\npub fn grab(mon: &Monitor) -> Frame {\n    let dc = unsafe { GetDC(None) };", 560, 200, light),
            (yh, 16.0, "Jie 是一个 Windows 截图工具，全程用 CPU 渲染，\n空闲时私有内存不超过 15 MB，按快捷键马上出遮罩。", 560, 200, light),
            (Face::Segoe, 15.0, en, 560, 200, light),
            // 大选区小字
            (yh, 13.0, zh, 1400, 400, light),
            (yh, 18.0, zh, 1400, 400, light),
            // 深色、灰底灰字
            (yh, 13.0, zh, 520, 160, dark),
            (yh, 12.0, zh, 520, 160, (0x666666, 0xF3F3F3)),
            // 150% 缩放下的 14 像素中文、13 像素代码、15 像素英文
            (yh, 21.0, zh, 840, 260, light),
            (Face::Mono, 13.0, code, 520, 120, dark),
            (Face::Mono, 20.0, code, 800, 180, dark),
            (Face::Segoe, 22.0, en, 840, 160, light),
        ]
    }

    /// 验收 4.7：样张交给识别服务，打印每张的错误率和用时，不设门槛。
    /// 用 %APPDATA%\Jie 里真的设置和 API Key；样张会发给那家服务、按量计费，平时不跑：
    /// cargo test --release ocr::tests::llm_samples -- --ignored --exact --nocapture
    #[test]
    #[ignore]
    fn llm_samples() {
        crate::settings::load();
        let mut s = crate::settings::get();
        // 和设置窗口一样：翻译服务没填过就当是 DeepSeek
        if s.endpoint.is_empty() {
            (s.endpoint, s.model) = (crate::translate::DEEPSEEK.into(), crate::translate::DEEPSEEK_MODEL.into());
        }
        let cfg = s.ocr_service();
        assert!(cfg.ready(), "设置里的翻译服务或识别服务还没配好");
        let cases = sample_cases();
        let imgs: Vec<Image> = cases.iter().map(|&(font, px, text, w, h, colors)| render_on(font, px, text, w, h, colors)).collect();
        let (mut errs, mut secs) = (0.0, 0.0);
        for (&(font, px, want, w, h, _), img) in cases.iter().zip(&imgs) {
            let t0 = std::time::Instant::now();
            let got = match super::recognize(img, &cfg) {
                super::Outcome::Text { text, .. } => text,
                super::Outcome::Empty => String::new(),
                super::Outcome::Failed(e) => panic!("{font:?} {px}: {e}"),
            };
            // 结果是 Markdown：按卡片复制出来的纯文本算错误率
            let (e, t) = (cer(want, &crate::doc::parse(&got).plain), t0.elapsed().as_secs_f64());
            println!("[{font:?} {px} 像素 · {w}×{h}] 错 {e:.1}% · {t:.1} 秒\n{got}\n");
            (errs, secs) = (errs + e, secs + t);
        }
        let n = cases.len() as f64;
        println!("{}：平均错误率 {:.2}%，平均每张 {:.1} 秒", cfg.model, errs / n, secs / n);

        // 版面：标题、3 × 3 的表格、一块图片、一段字。打印认出的结构和裁出的图有多大（图在 (20, 200)–(220, 300)）
        let (img, want) = layout_sample();
        match super::recognize(&img, &cfg) {
            super::Outcome::Text { text, images, .. } => {
                let d = crate::doc::parse(&text);
                let tables: Vec<_> = d.nodes.iter().filter_map(|n| if let crate::doc::Node::Table { rows, head } = n { Some((rows.len(), rows[0].len(), *head)) } else { None }).collect();
                let boxes: Vec<_> = images.iter().map(|m| (m.w, m.h)).collect();
                println!("[版面] 错 {:.1}% · 表格（行, 列, 表头）{tables:?} · 裁出的图 {boxes:?}（应是 200×100）\n{text}\n", cer(want, &d.plain));
            }
            super::Outcome::Empty => println!("[版面] 没认出字"),
            super::Outcome::Failed(e) => panic!("版面: {e}"),
        }
    }

    /// 手工复核真实 PNG 的换行与完整性；沿用已配置的服务并计费，输出供逐项对照原图。
    /// 设置 JIE_OCR_IMAGE 后运行 cargo test llm_layout_file -- --ignored --nocapture。
    #[test]
    #[ignore]
    fn llm_layout_file() {
        crate::settings::load();
        let s = crate::settings::get();
        let cfg = s.ocr_service();
        assert!(cfg.ready(), "识别服务未配置");
        let png = std::fs::read(std::env::var_os("JIE_OCR_IMAGE").expect("请设置 JIE_OCR_IMAGE 为 PNG 路径")).unwrap();
        let text = crate::translate::read_image(&cfg, &super::base64(&png), crate::translate::OCR_TIMEOUT).unwrap();
        let d = crate::doc::parse(&text);
        let dir = std::path::Path::new("target/test-tmp/ocr-layout");
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("recognized.md"), &text).unwrap();
        std::fs::write(dir.join("plain.txt"), &d.plain).unwrap();
        println!("识别模型：{} · {} 行\n{text}", cfg.model, d.lines());
        let tr = crate::translate::translate(&s.translate_service(), &d.src, crate::translate::OCR_TIMEOUT).unwrap();
        std::fs::write(dir.join("translated.md"), &tr).unwrap();
        println!("译文：\n{tr}");
    }

    /// 版面样张（600 × 380 白底）和它的纯文本
    fn layout_sample() -> (Image, &'static str) {
        use crate::ui::rectf;
        let bmp = Bitmap::new(600, 380).unwrap();
        let cells = [["名称", "单价", "库存"], ["苹果", "3.5 元", "120"], ["香蕉", "2 元", "80"]];
        {
            let c = Canvas::new(&bmp, bmp.full(), false);
            c.clear(rgb(0xFFFFFF));
            let ink = rgb(0x1A1A1A);
            c.text(&Text::new(Face::Cjk, 18.0, true, "水果价格").unwrap(), 20.0, 14.0, ink);
            for i in 0..4 {
                let (x, y) = (20.0 + 140.0 * i as f32, 50.0 + 40.0 * i as f32);
                c.fill_rect(rectf(20.0, y, 440.0, y + 1.0), ink);
                c.fill_rect(rectf(x, 50.0, x + 1.0, 170.0), ink);
            }
            for (r, row) in cells.iter().enumerate() {
                for (k, t) in row.iter().enumerate() {
                    c.text(&Text::new(Face::Cjk, 14.0, r == 0, t).unwrap(), 30.0 + 140.0 * k as f32, 60.0 + 40.0 * r as f32, ink);
                }
            }
            // 「照片」：一块渐变
            for y in 0..100 {
                c.fill_rect(rectf(20.0, 200.0 + y as f32, 220.0, 201.0 + y as f32), rgb(0x2E7D32 + ((y as u32 * 2) << 16)));
            }
            c.text(&Text::new(Face::Cjk, 14.0, false, "上图是今天到货的苹果。").unwrap(), 20.0, 320.0, ink);
        }
        (bmp.crop(&bmp.full()), "水果价格\n名称单价库存\n苹果3.5元120\n香蕉2元80\n上图是今天到货的苹果。")
    }

    #[test]
    fn error_rate() {
        assert_eq!(cer("使用 cargo", "使用cargo"), 0.0);
        assert_eq!(cer("abcd", "abxd"), 25.0);
        assert_eq!(cer("abcd", "abd"), 25.0);
        assert_eq!(cer("ab", ""), 100.0);
    }
}
