//! 马赛克：把原图做成马赛克位图，用它沿笔迹描边（Windows 是 Direct2D 的位图画刷）。
//! 马赛克位图每格只占一个像素，画的时候放大 block 倍、最近邻取样，格子就对齐到原图的整格。
//! 每次只做这次看得见的那几格（增量重画只有几十格），不缓存整图，也就不占常驻内存（钉图的内存预算）。

use super::{BRUSH, Editor, PEN, Pt, grow, span};
use crate::plat::{Bitmap, Canvas, Cursor};

/// 沿笔迹 pts 涂宽 brush、格子 block（像素）的马赛克，像素取自原图 src
pub fn draw(c: &Canvas, pts: &[Pt], brush: f32, block: f32, src: &Bitmap) {
    let b = block.round().max(1.0);
    let Some(g) = super::smooth(pts) else { return };
    // 这次画的那块换回原图坐标（钉图带缩放），和笔迹范围、原图取交集，再扩到整格
    let vis = c.visible();
    let r = grow(span(pts.iter().copied()), brush / 2.0 + 1.0);
    let (l, t) = (r.left.max(vis.left).max(0.0), r.top.max(vis.top).max(0.0));
    let (rr, bb) = (r.right.min(vis.right).min(src.w as f32), r.bottom.min(vis.bottom).min(src.h as f32));
    if rr <= l || bb <= t {
        return;
    }
    let (i0, j0, i1, j1) = ((l / b).floor() as i32, (t / b).floor() as i32, (rr / b).ceil() as i32, (bb / b).ceil() as i32);
    // 每格取格子中心的原图像素，出了原图的半格取边上的
    let (px, bi, w) = (src.pixels(), b as i32, src.w as usize);
    let mut cells = Vec::with_capacity(((i1 - i0) * (j1 - j0) * 4) as usize);
    for j in j0..j1 {
        let y = (j * bi + bi / 2).min(src.h - 1) as usize;
        for i in i0..i1 {
            let k = (y * w + (i * bi + bi / 2).min(src.w - 1) as usize) * 4;
            cells.extend_from_slice(&px[k..k + 4]);
        }
    }
    // 格子图的坐标在原图坐标里，钉图的缩放、平移会带着它走
    c.stroke_cells(&g, brush, &cells, (i1 - i0) as u32, (j1 - j0) as u32, b, (i0 as f32 * b, j0 as f32 * b));
}

/// 选着马赛克工具时的光标：笔刷大小的圆圈（白 3 + 黑 1 双线），直径 = 笔刷 × 钉图缩放，最大 120
pub fn cursor(ed: &Editor) -> Cursor {
    Cursor::Ring((BRUSH[PEN.get().1] * ed.scale * ed.zoom).min(120.0), ed.scale)
}

#[cfg(test)]
mod tests {
    use super::super::{Ann, BLOCK, Doc, Shape, Tool};
    use super::*;
    use crate::plat::{Affine, Face, Rect as RECT, Text, ring_pixels};
    use crate::ui::{self, rectf};
    use std::time::{Duration, Instant};

    /// 每个像素都不一样的底图：B = x、G = y、R = x ^ y
    fn pattern(w: i32, h: i32) -> Bitmap {
        let mut b = Bitmap::new(w, h).unwrap();
        for (i, p) in b.pixels_mut().chunks_exact_mut(4).enumerate() {
            let (x, y) = (i as i32 % w, i as i32 / w);
            p.copy_from_slice(&[x as u8, y as u8, (x ^ y) as u8, 0]);
        }
        b
    }

    /// 在 out 的 r 这块按缩放 k 画标注（钉图的画法：原图原点在 out 的左上角）
    fn render(ed: &Editor, src: &Bitmap, out: &Bitmap, r: RECT, k: f32) {
        let c = Canvas::new(out, r, false);
        c.set_transform(Affine { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: 0.0, M32: 0.0 });
        ed.draw(&c, src, None);
    }

    fn rgb_at(b: &Bitmap, x: i32, y: i32) -> [u8; 3] {
        let k = ((y * b.w + x) * 4) as usize;
        b.pixels()[k..k + 3].try_into().unwrap()
    }

    #[test]
    fn cells_follow_source_at_any_zoom() {
        let src = pattern(100, 64);
        let p = Pt::new;
        // 红箭头在下面，粗笔刷（44、格子 12）的马赛克盖住它
        let ed = Editor::new(
            Doc::with(vec![
                Ann { shape: Shape::Arrow(p(4.0, 30.0), p(90.0, 30.0)), color: 0, size: 2 },
                Ann { shape: Shape::Mosaic(vec![p(10.0, 30.0), p(50.0, 32.0), p(90.0, 30.0)]), color: 0, size: 2 },
            ]),
            1.0,
        );
        for (k, part) in [(1.0, RECT { left: 37, top: 11, right: 83, bottom: 47 }), (2.5, RECT { left: 93, top: 29, right: 201, bottom: 121 })] {
            let (w, h) = ((100.0 * k) as i32, (64.0 * k) as i32);
            let (full, redraw) = (Bitmap::new(w, h).unwrap(), Bitmap::new(w, h).unwrap());
            render(&ed, &src, &full, RECT { left: 0, top: 0, right: w, bottom: h }, k);
            // 只重画一块（遮罩、钉图的增量重画）：和整张画出来的一模一样
            render(&ed, &src, &redraw, part, k);
            for y in part.top..part.bottom {
                for x in part.left..part.right {
                    assert_eq!(rgb_at(&full, x, y), rgb_at(&redraw, x, y), "k={k} ({x}, {y})");
                }
            }
            // 笔迹里面每个像素都是它所在格子中心的原图颜色，箭头的红色漏不进来
            for y in 0..h {
                for x in 0..w {
                    let (ox, oy) = ((x as f32 + 0.5) / k, (y as f32 + 0.5) / k);
                    if !(12.0..88.0).contains(&ox) || !(14.0..48.0).contains(&oy) {
                        continue;
                    }
                    let (cx, cy) = ((ox as i32 / 12 * 12 + 6).min(99), (oy as i32 / 12 * 12 + 6).min(63));
                    assert_eq!(rgb_at(&full, x, y), rgb_at(&src, cx, cy), "k={k} ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn drawing_stroke_reports_only_its_tail() {
        let mut e = Editor::new(Doc::default(), 1.0);
        e.set_tool(Tool::Mosaic);
        e.down(Pt::new(0.0, 0.0), true);
        for i in 1..=50 {
            e.drag_to(Pt::new(i as f32 * 20.0, (i % 2) as f32 * 300.0), false);
        }
        // 拖动时只报最后三个点（x 960–1000）那截，加上半个笔刷和抗锯齿
        let (r, ..) = e.looks()[0];
        assert!(r.left > 900 && r.right < 1100 && r.top < 0 && r.bottom > 300, "{r:?}");
        // 松手后报整条
        e.up();
        let (r, ..) = e.looks()[0];
        assert!(r.left < 0 && r.right > 1000, "{r:?}");
    }

    #[test]
    fn ring_cursor_follows_zoom() {
        let mut e = Editor::new(Doc::default(), 1.5);
        e.set_tool(Tool::Mosaic);
        assert_eq!(cursor(&e), Cursor::Ring(42.0, 1.5));
        // 钉图放大：圆圈跟着变大，到 120 封顶
        e.zoom = 2.0;
        assert_eq!(cursor(&e), Cursor::Ring(84.0, 1.5));
        e.zoom = 4.0;
        assert_eq!(cursor(&e), Cursor::Ring(120.0, 1.5));
    }

    fn png(img: &Bitmap, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join("annotate");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), crate::plat::encode(&img.crop(&img.full()), false).unwrap()).unwrap();
    }

    /// 有字有色块的底图上画三档马赛克，一支红箭头被盖住、一支绿箭头画在马赛克上面；另存 250% 的钉图画法和三档圆圈光标。
    /// 存成 target/test-tmp/annotate/mosaic*.png 对设计稿。平时不跑：cargo test mosaic_samples -- --ignored
    #[test]
    #[ignore]
    fn mosaic_samples() {
        let s = 1.5;
        let (w, h) = (960, 360);
        let mut src = Bitmap::new(w, h).unwrap();
        for (i, p) in src.pixels_mut().chunks_exact_mut(4).enumerate() {
            let (x, y) = (i as i32 % w, i as i32 / w);
            p.copy_from_slice(&[(255 - x * 255 / w) as u8, (y * 255 / h) as u8, (x * 255 / w) as u8, 0]);
        }
        {
            let c = Canvas::new(&src, src.full(), false);
            for (i, t) in ["账号 jie@example.com", "密码 hunter2  验证码 4096", "订单号 2026-1021-0074"].iter().enumerate() {
                let l = Text::new(Face::Cjk, 36.0, true, t).unwrap();
                c.text(&l, 30.0, 30.0 + i as f32 * 100.0, ui::rgb(0x1B1B1F));
            }
        }
        let p = Pt::new;
        let a = |shape, color, size| Ann { shape, color, size };
        let zig = |y: f32| (0..12).map(|i| p(40.0 + i as f32 * 45.0, y + (i % 2) as f32 * 14.0)).collect::<Vec<_>>();
        let ed = Editor::new(
            Doc::with(vec![
                a(Shape::Arrow(p(620.0, 330.0), p(900.0, 60.0)), 0, 2),
                a(Shape::Mosaic(zig(45.0)), 0, 0),
                a(Shape::Mosaic(zig(140.0)), 0, 1),
                a(Shape::Mosaic(zig(240.0)), 0, 2),
                a(Shape::Mosaic(vec![p(640.0, 250.0), p(760.0, 200.0), p(880.0, 140.0), p(800.0, 100.0)]), 0, 2),
                a(Shape::Arrow(p(640.0, 60.0), p(900.0, 300.0)), 2, 2),
            ]),
            s,
        );
        let out = src.sub(&src.full()).unwrap();
        render(&ed, &src, &out, out.full(), 1.0);
        png(&out, "mosaic.png");
        // 钉图放大 250%：原图最近邻放大，马赛克的格子要和原图的整格对齐
        let k = 2.5;
        let zoom = Bitmap::new(800, 500).unwrap();
        {
            let c = Canvas::new(&zoom, zoom.full(), false);
            c.set_transform(Affine { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: -600.0 * k, M32: -80.0 * k });
            c.pixels(src.pixels(), w as u32, h as u32, rectf(0.0, 0.0, w as f32, h as f32));
            ed.draw(&c, &src, None);
        }
        png(&zoom, "mosaic_zoom.png");
        // 三档笔刷的圆圈光标，再加钉图 500% 时封顶的那个；灰底上看白黑两圈
        let mut cur = Bitmap::new(560, 160).unwrap();
        cur.pixels_mut().fill(0x80);
        let mut e = Editor::new(Doc::default(), s);
        e.set_tool(Tool::Mosaic);
        for (i, (size, z)) in [(0, 1.0), (1, 1.0), (2, 1.0), (2, 5.0)].into_iter().enumerate() {
            e.set_size(size);
            e.zoom = z;
            let Cursor::Ring(d, s) = cursor(&e) else { unreachable!() };
            // 预乘 alpha 的圆圈叠到灰底上：src + dst × (1 - a)
            let (n, px) = ring_pixels(d, s);
            let (x0, w) = (10 + i as i32 * 135, cur.w);
            let dst = cur.pixels_mut();
            for (k, p) in px.iter().enumerate() {
                let (x, y) = (x0 + k as i32 % n, 10 + k as i32 / n);
                let o = ((y * w + x) * 4) as usize;
                let a = p >> 24;
                for (j, sh) in [0, 8, 16].into_iter().enumerate() {
                    dst[o + j] = ((p >> sh & 0xFF) + dst[o + j] as u32 * (255 - a) / 255) as u8;
                }
            }
        }
        png(&cur, "mosaic_cursor.png");
    }

    /// 3840 × 2160 的底图上一边拖一边按遮罩的办法增量重画：比 looks() 的前后差别，重画变了的那块（底图两次 BitBlt + 全部标注），
    /// 打印每段的耗时，再打印松手时整条重画和整屏重画的耗时。平时不跑：cargo test --release mosaic_drag_4k -- --ignored --nocapture
    #[test]
    #[ignore]
    fn mosaic_drag_4k() {
        let (w, h) = (3840, 2160);
        let src = pattern(w, h);
        let (dim, back) = (src.dimmed().unwrap(), src.sub(&src.full()).unwrap());
        let screen = src.full();
        let repaint = |ed: &Editor, r: RECT| -> Duration {
            let c = RECT { left: r.left.max(0), top: r.top.max(0), right: r.right.min(screen.right), bottom: r.bottom.min(screen.bottom) };
            let t = Instant::now();
            let cv = Canvas::new(&back, c, false);
            for b in [&dim, &src] {
                cv.blit(b, c, c.left, c.top);
            }
            ed.draw(&cv, &src, Some(0x9C44EC));
            drop(cv);
            t.elapsed()
        };
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let p = Pt::new;
        // 来回横扫，每段 24 像素（每秒 3000 像素的快拖、125 Hz 的鼠标），600 段铺满大半个屏幕
        let sweep = |i: usize, y0: f32| {
            let (row, t) = (i / 150, (i % 150) as f32 / 150.0);
            let x = if row % 2 == 0 { 200.0 + t * 3400.0 } else { 3600.0 - t * 3400.0 };
            p(x, y0 + row as f32 * 420.0 + t * 200.0)
        };
        for (s, size) in [(1.0, 0), (1.5, 1), (1.5, 2)] {
            // 屏幕上已经有几样标注，包括一条同样长的旧马赛克
            let a = |shape, size| Ann { shape, color: 0, size };
            let mut e = Editor::new(
                Doc::with(vec![
                    a(Shape::Mosaic((0..600).map(|i| sweep(i, 300.0)).collect()), size),
                    a(Shape::Rect(p(300.0, 300.0), p(1800.0, 1200.0)), 1),
                    a(Shape::Arrow(p(400.0, 1800.0), p(3000.0, 400.0)), 2),
                    a(Shape::Pen((0..200).map(|i| p(500.0 + i as f32 * 15.0, 1500.0 + (i % 7) as f32 * 30.0)).collect()), 1),
                ]),
                s,
            );
            e.set_size(size);
            e.set_tool(Tool::Mosaic);
            let mut l0 = e.looks();
            e.down(sweep(0, 100.0), true);
            let (mut times, mut area) = (Vec::new(), 0i64);
            for i in 1..600 {
                e.drag_to(sweep(i, 100.0), false);
                let l1 = e.looks();
                // 遮罩 update() 的办法：变了的标注新旧范围都重画，paint() 合成它们的外接矩形
                let mut dirty: Option<RECT> = None;
                for j in 0..l0.len().max(l1.len()) {
                    if l0.get(j) != l1.get(j) {
                        for (r, ..) in [l0.get(j), l1.get(j)].into_iter().flatten() {
                            dirty = Some(dirty.map_or(*r, |d| RECT { left: d.left.min(r.left), top: d.top.min(r.top), right: d.right.max(r.right), bottom: d.bottom.max(r.bottom) }));
                        }
                    }
                }
                l0 = l1;
                if let Some(d) = dirty {
                    area += ((d.right - d.left) * (d.bottom - d.top)) as i64;
                    times.push(repaint(&e, d));
                }
            }
            e.up();
            let whole = repaint(&e, e.looks().last().unwrap().0);
            let full = repaint(&e, screen);
            times.sort();
            let avg = times.iter().sum::<Duration>() / times.len() as u32;
            println!(
                "缩放 {s}、笔刷 {} 格子 {}：增量重画 {} 次，平均 {:.2} ms，中位 {:.2} ms，最慢 {:.2} ms，平均重画 {:.0} 像素；松手整条重画 {:.1} ms，整屏重画 {:.1} ms",
                BRUSH[size] * s,
                BLOCK[size] * s,
                times.len(),
                ms(avg),
                ms(times[times.len() / 2]),
                ms(*times.last().unwrap()),
                area as f64 / times.len() as f64,
                ms(whole),
                ms(full)
            );
        }
    }
}
