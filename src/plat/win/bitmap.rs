//! 位图：选进内存 DC 的 32 位自上而下 DIB，可以直接和窗口互相 BitBlt。

use windows::Win32::Graphics::Gdi::*;

pub struct Bitmap {
    pub(crate) dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub w: i32,
    pub h: i32,
}

impl Bitmap {
    pub fn new(w: i32, h: i32) -> Option<Bitmap> {
        unsafe {
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let dc = CreateCompatibleDC(None);
            let old = SelectObject(dc, bmp.into());
            Some(Bitmap { dc, bmp, old, bits: bits as _, w, h })
        }
    }

    /// BGRA，自上而下逐行
    pub fn pixels(&self) -> &[u8] {
        // GDI 画过的要先落进内存再读
        let _ = unsafe { GdiFlush() };
        unsafe { std::slice::from_raw_parts(self.bits, (self.w * self.h * 4) as usize) }
    }

    pub fn pixels_mut(&mut self) -> &mut [u8] {
        let _ = unsafe { GdiFlush() };
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h * 4) as usize) }
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteDC(self.dc);
            let _ = DeleteObject(self.bmp.into());
        }
    }
}
