use super::{Bitmap, Win};
use crate::plat::{Image, SaveAs};
use objc2::{AnyThread, class, msg_send, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSOpenPanel, NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeTIFF, NSSavePanel};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSNumber, NSString, NSURL, ns_string};
use std::path::PathBuf;

fn encode_type(img: &Image, typ: NSBitmapImageFileType) -> Result<Vec<u8>, String> {
    if img.w <= 0 || img.h <= 0 {
        return Err("无效图片尺寸".into());
    }
    let len = (img.w as usize).checked_mul(img.h as usize).and_then(|n| n.checked_mul(4)).ok_or("图片太大")?;
    if img.px.len() != len {
        return Err("图片像素数据长度不对".into());
    }
    let mut b = Bitmap::new(img.w, img.h).ok_or("无法创建图片缓冲")?;
    b.pixels_mut().copy_from_slice(&img.px);
    let cg = b.image().ok_or("无法生成图片")?;
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
    let props = NSDictionary::from_slices(&[unsafe { objc2_app_kit::NSImageCompressionFactor }], &[&*NSNumber::new_f64(0.9)]);
    let data = unsafe { rep.representationUsingType_properties(typ, props.cast_unchecked()) }.ok_or("编码图片失败")?;
    Ok(data.to_vec())
}

pub fn encode(img: &Image, jpeg: bool) -> Result<Vec<u8>, String> {
    encode_type(img, if jpeg { NSBitmapImageFileType::JPEG } else { NSBitmapImageFileType::PNG })
}

pub fn copy_image(img: &Image) -> Result<(), String> {
    let png = encode_type(img, NSBitmapImageFileType::PNG)?;
    let tiff = encode_type(img, NSBitmapImageFileType::TIFF)?;
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let a = NSData::with_bytes(&png);
    let b = NSData::with_bytes(&tiff);
    if !pb.setData_forType(Some(&a), unsafe { NSPasteboardTypePNG }) || !pb.setData_forType(Some(&b), unsafe { NSPasteboardTypeTIFF }) {
        return Err("写入剪贴板失败".into());
    }
    Ok(())
}

/// 复制文件：剪贴板里放文件 URL（访达、聊天软件粘贴的是这个文件）；GIF 另放一份原始数据，认图片的程序直接贴动图
pub fn copy_file(path: &std::path::Path) -> Result<(), String> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let s = url.absoluteString().ok_or("文件路径不对")?;
    let ok = pb.setString_forType(&s, unsafe { objc2_app_kit::NSPasteboardTypeFileURL });
    if !ok {
        return Err("写入剪贴板失败".into());
    }
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"))
        && let Ok(bytes) = std::fs::read(path)
    {
        let _ = pb.setData_forType(Some(&NSData::with_bytes(&bytes)), ns_string!("com.compuserve.gif"));
    }
    Ok(())
}

pub fn copy_text(text: &str) -> Result<(), String> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let s = NSString::from_str(text);
    if pb.setString_forType(&s, unsafe { objc2_app_kit::NSPasteboardTypeString }) { Ok(()) } else { Err("写入剪贴板失败".into()) }
}
pub fn paste_text() -> Option<String> {
    NSPasteboard::generalPasteboard().stringForType(unsafe { objc2_app_kit::NSPasteboardTypeString }).map(|s| s.to_string())
}
pub fn scale_down(src: &Bitmap, w: i32, h: i32) -> Option<Bitmap> {
    let b = Bitmap::new(w, h)?;
    let image = src.image()?;
    objc2_core_graphics::CGContext::set_interpolation_quality(Some(&b.ctx), objc2_core_graphics::CGInterpolationQuality::High);
    objc2_core_graphics::CGContext::draw_image(
        Some(&b.ctx),
        objc2_core_foundation::CGRect::new(objc2_core_foundation::CGPoint::new(0.0, 0.0), objc2_core_foundation::CGSize::new(w as f64, h as f64)),
        Some(&image),
    );
    Some(b)
}

pub fn ask_save_path(owner: Win, dir: &str, name: &str, kind: SaveAs) -> Option<PathBuf> {
    let panel = NSSavePanel::savePanel(objc2::MainThreadMarker::new()?);
    let d = NSURL::fileURLWithPath(&NSString::from_str(dir));
    panel.setDirectoryURL(Some(&d));
    panel.setNameFieldStringValue(&NSString::from_str(&format!("{}.{}", name, kind.ext())));
    panel.setCanCreateDirectories(true);
    panel.setAllowsOtherFileTypes(false);
    let typ: Retained<AnyObject> = unsafe { msg_send![class!(UTType), typeWithFilenameExtension:&*NSString::from_str(kind.ext())] };
    let types = NSArray::from_slice(&[&*typ]);
    unsafe {
        let _: () = msg_send![&*panel,setAllowedContentTypes:&*types];
    }

    if owner.dialog(|| panel.runModal()) == objc2_app_kit::NSModalResponseOK
    {
        panel.URL().and_then(|u| u.path()).map(|p| PathBuf::from(p.to_string()))
    } else {
        None
    }
}
pub fn ask_folder(owner: Win, start: &str) -> Option<String> {
    let panel = NSOpenPanel::openPanel(objc2::MainThreadMarker::new()?);
    let d = NSURL::fileURLWithPath(&NSString::from_str(start));
    panel.setDirectoryURL(Some(&d));
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(false);
    panel.setAllowsMultipleSelection(false);
    if owner.dialog(|| panel.runModal()) == objc2_app_kit::NSModalResponseOK
    {
        panel.URL().and_then(|u| u.path()).map(|p| p.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encoders_validate_data_and_preserve_rows() {
        assert!(encode(&Image { w: 2, h: 3, px: vec![0; 4] }, false).is_err());
        let image = Image { w: 2, h: 2, px: vec![0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255] };
        for typ in [NSBitmapImageFileType::PNG, NSBitmapImageFileType::TIFF] {
            let bytes = encode_type(&image, typ).unwrap();
            if typ == NSBitmapImageFileType::PNG {
                assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
            } else {
                assert!(bytes.starts_with(b"II") || bytes.starts_with(b"MM"));
            }
            let rep = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &NSData::with_bytes(&bytes)).unwrap();
            let cg = rep.CGImage().unwrap();
            let bmp = Bitmap::new(2, 2).unwrap();
            objc2_core_graphics::CGContext::draw_image(
                Some(&bmp.ctx),
                objc2_core_foundation::CGRect::new(objc2_core_foundation::CGPoint::new(0.0, 0.0), objc2_core_foundation::CGSize::new(2.0, 2.0)),
                Some(&cg),
            );
            assert_eq!(bmp.pixels(), image.px);
        }
        assert!(encode(&image, true).unwrap().starts_with(&[0xFF, 0xD8]));
    }
}

#[link(name = "UniformTypeIdentifiers", kind = "framework")]
unsafe extern "C" {}
