//! WIC 编码和缩放、剪贴板、保存和选文件夹对话框。

use super::Bitmap;
use crate::plat::Image;
use std::path::PathBuf;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPV5HEADER};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::{CF_DIBV5, CF_UNICODETEXT};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE};
use windows::core::{Error, GUID, HSTRING, Result, w};

/// 在 COM 里跑 f：界面线程启动时已经进了 STA，工作线程临时进 MTA，用完退出
fn com<T>(f: impl FnOnce() -> T) -> T {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        let r = f();
        // 已经进过（S_FALSE）也要配一次退出；模式不同（界面线程的 STA）时没进成，不退
        if hr.is_ok() {
            CoUninitialize();
        }
        r
    }
}

/// 编码成 PNG 或 JPEG 写进 stream；源图当作不带 alpha 的 32 位，编码器自己转成 24 位
fn encode_to(img: &Image, format: &GUID, stream: &IStream) -> Result<()> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let (w, h) = (img.w as u32, img.h as u32);
        let src = wic.CreateBitmapFromMemory(w, h, &GUID_WICPixelFormat32bppBGR, w * 4, &img.px)?;
        let enc = wic.CreateEncoder(format, std::ptr::null())?;
        enc.Initialize(stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        enc.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.ok_or_else(|| Error::from(E_FAIL))?;
        // 不传编码参数：JPEG 用 WIC 的默认质量 0.9，就是设置里写的「质量固定 90」
        frame.Initialize(None)?;
        frame.SetSize(w, h)?;
        frame.WriteSource(&src, std::ptr::null())?;
        frame.Commit()?;
        enc.Commit()
    }
}

fn encode_bytes(img: &Image, format: &GUID) -> Result<Vec<u8>> {
    unsafe {
        let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true)?;
        encode_to(img, format, &stream)?;
        let mut st = STATSTG::default();
        stream.Stat(&mut st, STATFLAG_NONAME)?;
        let mem = GetHGlobalFromStream(&stream)?;
        let p = GlobalLock(mem) as *const u8;
        if p.is_null() {
            return Err(Error::from_thread());
        }
        let v = std::slice::from_raw_parts(p, st.cbSize as usize).to_vec();
        let _ = GlobalUnlock(mem);
        Ok(v)
    }
}

/// 编码成 PNG（`jpeg` 为 true 时 JPEG）；哪个线程都能调
pub fn encode(img: &Image, jpeg: bool) -> std::result::Result<Vec<u8>, String> {
    let format = if jpeg { GUID_ContainerFormatJpeg } else { GUID_ContainerFormatPng };
    com(|| encode_bytes(img, &format)).map_err(|e| e.message())
}

/// CF_DIBV5：32 位 BI_RGB、自下而上（老程序都认），alpha 是 255，所以认不认 alpha 都是不透明
fn dibv5_bytes(img: &Image) -> Vec<u8> {
    let hdr = BITMAPV5HEADER {
        bV5Size: size_of::<BITMAPV5HEADER>() as u32,
        bV5Width: img.w,
        bV5Height: img.h,
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_RGB,
        bV5SizeImage: img.px.len() as u32,
        bV5CSType: 0x7352_4742, // LCS_sRGB
        bV5Intent: 4,           // LCS_GM_IMAGES
        ..Default::default()
    };
    let mut v = Vec::with_capacity(size_of_val(&hdr) + img.px.len());
    v.extend_from_slice(unsafe { std::slice::from_raw_parts(&hdr as *const _ as *const u8, size_of_val(&hdr)) });
    img.px.chunks_exact(img.w as usize * 4).rev().for_each(|row| v.extend_from_slice(row));
    v
}

thread_local! {
    /// 没有主窗口时（测试）剪贴板的所有者：这个线程上的仅消息窗口
    static HELPER: HWND = unsafe {
        CreateWindowExW(WINDOW_EX_STYLE::default(), w!("STATIC"), w!(""), WINDOW_STYLE::default(), 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None).unwrap_or_default()
    };
}

/// 剪贴板的所有者：主窗口，没有就用本线程的仅消息窗口（所有者为空时 SetClipboardData 会失败）
fn owner() -> HWND {
    let m = super::main_window();
    if m.is_invalid() { HELPER.with(|h| *h) } else { m }
}

/// 复制图片：同时写 PNG 和 CF_DIBV5，兼容微信、Word、画图
pub fn copy_image(img: &Image) -> std::result::Result<(), String> {
    // 编码放在打开剪贴板之前，不让别的程序等
    let dib = dibv5_bytes(img);
    let png = encode(img, false);
    unsafe {
        OpenClipboard(Some(owner())).map_err(|e| e.message())?;
        let r = EmptyClipboard().and_then(|_| {
            set_clip(CF_DIBV5.0 as u32, &dib)?;
            // PNG 编码或写入失败不影响已经写好的位图
            if let Ok(png) = &png {
                let _ = set_clip(RegisterClipboardFormatW(w!("PNG")), png);
            }
            Ok(())
        });
        let _ = CloseClipboard();
        r.map_err(|e| e.message())
    }
}

/// 复制文字：CF_UNICODETEXT，换行换成 \r\n（老程序只认这种）
pub fn copy_text(text: &str) -> std::result::Result<(), String> {
    let w: Vec<u16> = text.replace('\n', "\r\n").encode_utf16().chain([0]).collect();
    let bytes = unsafe { std::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 2) };
    unsafe {
        OpenClipboard(Some(owner())).map_err(|e| e.message())?;
        let r = EmptyClipboard().and_then(|_| set_clip(CF_UNICODETEXT.0 as u32, bytes));
        let _ = CloseClipboard();
        r.map_err(|e| e.message())
    }
}

/// 读剪贴板里的纯文字（CF_UNICODETEXT）；没有文字时为 None
pub fn paste_text() -> Option<String> {
    unsafe {
        OpenClipboard(Some(owner())).ok()?;
        let s = GetClipboardData(CF_UNICODETEXT.0 as u32).ok().and_then(|h| {
            let mem = HGLOBAL(h.0);
            let p = GlobalLock(mem) as *const u16;
            if p.is_null() {
                return None;
            }
            // 按内存块大小读，不信任结尾一定有 0
            let w = std::slice::from_raw_parts(p, GlobalSize(mem) / 2);
            let s = String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())]);
            let _ = GlobalUnlock(mem);
            Some(s)
        });
        let _ = CloseClipboard();
        s
    }
}

/// 拷进新分配的全局内存交给剪贴板；成功后内存归系统，失败时自己释放
unsafe fn set_clip(format: u32, bytes: &[u8]) -> Result<()> {
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
        let p = GlobalLock(mem) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err(Error::from_thread());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(mem);
        SetClipboardData(format, Some(HANDLE(mem.0))).map(drop).inspect_err(|_| {
            let _ = GlobalFree(Some(mem));
        })
    }
}

/// 高质量缩小（WIC Fant，按面积平均），钉图缩小显示用
pub fn scale_down(src: &Bitmap, w: i32, h: i32) -> Option<Bitmap> {
    let mut out = Bitmap::new(w, h)?;
    com(|| unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let from = wic.CreateBitmapFromMemory(src.w as u32, src.h as u32, &GUID_WICPixelFormat32bppBGR, src.w as u32 * 4, src.pixels()).ok()?;
        let scaler = wic.CreateBitmapScaler().ok()?;
        scaler.Initialize(&from, w as u32, h as u32, WICBitmapInterpolationModeFant).ok()?;
        scaler.CopyPixels(std::ptr::null(), w as u32 * 4, out.pixels_mut()).ok()
    })?;
    Some(out)
}

fn shell_item(path: &str) -> Option<IShellItem> {
    unsafe { SHCreateItemFromParsingName(&HSTRING::from(path), None).ok() }
}

/// 系统保存对话框：PNG 或 JPEG，从 `dir` 开始、默认文件名 `name`（不带扩展名）。取消时返回 None
pub fn ask_save_path(owner: HWND, dir: &str, name: &str, jpeg: bool) -> Option<PathBuf> {
    unsafe {
        let dlg: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let types = [
            COMDLG_FILTERSPEC { pszName: w!("PNG 图片"), pszSpec: w!("*.png") },
            COMDLG_FILTERSPEC { pszName: w!("JPEG 图片"), pszSpec: w!("*.jpg;*.jpeg") },
        ];
        dlg.SetFileTypes(&types).ok()?;
        dlg.SetFileTypeIndex(if jpeg { 2 } else { 1 }).ok()?;
        dlg.SetDefaultExtension(if jpeg { w!("jpg") } else { w!("png") }).ok()?;
        if let Some(dir) = shell_item(dir) {
            let _ = dlg.SetFolder(&dir);
        }
        let ext = if jpeg { "jpg" } else { "png" };
        dlg.SetFileName(&HSTRING::from(format!("{name}.{ext}"))).ok()?;
        let opts = dlg.GetOptions().ok()?;
        dlg.SetOptions(opts | FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST).ok()?;
        dlg.Show(Some(owner)).ok()?;
        let p = dlg.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = PathBuf::from(p.to_string().ok()?);
        CoTaskMemFree(Some(p.0 as _));
        Some(path)
    }
}

/// 系统选择文件夹对话框，从 start 开始；取消时返回 None
pub fn ask_folder(owner: HWND, start: &str) -> Option<String> {
    unsafe {
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dlg.GetOptions().ok()?;
        dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST).ok()?;
        if let Some(dir) = shell_item(start) {
            let _ = dlg.SetFolder(&dir);
        }
        dlg.Show(Some(owner)).ok()?;
        let p = dlg.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        path
    }
}
