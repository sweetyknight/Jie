//! 取字入口：把截图发给 OpenAI 兼容的看图模型（请求在 translate.rs），本机不识别。

use crate::plat::{self, Image};
use crate::translate;
use std::time::Instant;

pub enum Outcome {
    /// 识别出的文字（逐行，\n 分隔）、标题栏上的用时「1.4 秒」、行数
    Text { text: String, took: String, lines: usize },
    Empty,
    Failed(String),
}

/// 选区编成 PNG 发给 `cfg` 的服务，阻塞，要在工作线程里调。换行交给模型。
/// shortcut: 大图由服务自己缩小（DeepSeek 缩到约 1300×1300 像素），整屏小字可能认不清；真遇到了再切块分几次发。
pub fn recognize(img: &Image, cfg: &translate::Config) -> Outcome {
    let t0 = Instant::now();
    let png = match plat::encode(img, false) {
        Ok(p) => base64(&p),
        Err(e) => return Outcome::Failed(e),
    };
    match translate::read_image(cfg, &png, translate::OCR_TIMEOUT) {
        Ok(text) if text.is_empty() => Outcome::Empty,
        Ok(text) => Outcome::Text { lines: text.lines().count(), took: format!("{:.1} 秒", t0.elapsed().as_secs_f32()), text },
        Err(e) => Outcome::Failed(e.to_string()),
    }
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
            assert!(matches!(super::recognize(&img, &cfg), super::Outcome::Text { ref text, lines: 1, .. } if text == "hello"));
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
            let (e, t) = (cer(want, &got), t0.elapsed().as_secs_f64());
            println!("[{font:?} {px} 像素 · {w}×{h}] 错 {e:.1}% · {t:.1} 秒\n{got}\n");
            (errs, secs) = (errs + e, secs + t);
        }
        let n = cases.len() as f64;
        println!("{}：平均错误率 {:.2}%，平均每张 {:.1} 秒", cfg.model, errs / n, secs / n);
    }

    #[test]
    fn error_rate() {
        assert_eq!(cer("使用 cargo", "使用cargo"), 0.0);
        assert_eq!(cer("abcd", "abxd"), 25.0);
        assert_eq!(cer("abcd", "abd"), 25.0);
        assert_eq!(cer("ab", ""), 100.0);
    }
}
