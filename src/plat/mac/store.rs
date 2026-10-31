use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};
use std::path::PathBuf;

pub const KEY_STORE: &str = "macOS 钥匙串";
const SERVICE: &str = "Jie";

pub fn config_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("HOME").map(PathBuf::from)?.join("Library/Application Support/Jie");
    Some(dir)
}

pub fn pictures_dir() -> String {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Pictures").to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn now_stamp() -> String {
    use objc2_foundation::{NSDate, NSDateFormatter, NSLocale, NSString};
    let formatter = NSDateFormatter::new();
    formatter.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(&NSString::from_str("en_US_POSIX"))));
    formatter.setDateFormat(Some(&NSString::from_str("yyyyMMdd_HHmmss")));
    formatter.stringFromDate(&NSDate::now()).to_string()
}

pub fn api_key(which: &str) -> String {
    get_generic_password(SERVICE, which).ok().and_then(|p| String::from_utf8(p).ok()).unwrap_or_default()
}

pub fn set_api_key(which: &str, key: &str) -> Result<(), String> {
    if key.is_empty() {
        match delete_generic_password(SERVICE, which) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == -25300 => Ok(()), // errSecItemNotFound
            Err(e) => Err(e.to_string()),
        }
    } else {
        set_generic_password(SERVICE, which, key.as_bytes()).map_err(|e| e.to_string())
    }
}

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}
fn login_service() -> objc2::rc::Retained<objc2::runtime::AnyObject> {
    unsafe { objc2::msg_send![objc2::class!(SMAppService), mainAppService] }
}
pub fn autostart() -> bool {
    let service = login_service();
    let status: isize = unsafe { objc2::msg_send![&*service, status] };
    status == 1
}
pub fn set_autostart(on: bool) -> Result<(), String> {
    let service = login_service();
    let result: Result<(), objc2::rc::Retained<objc2_foundation::NSError>> =
        unsafe { if on { objc2::msg_send![&*service,registerAndReturnError:_] } else { objc2::msg_send![&*service,unregisterAndReturnError:_] } };
    result.map_err(|e| e.localizedDescription().to_string())?;
    if on && !autostart() {
        return Err("请在系统设置的「通用 → 登录项」中允许 Jie".into());
    }
    Ok(())
}

pub fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from(vk as u8).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x24 => "Home".into(),
        0x23 => "End".into(),
        0x25 => "←".into(),
        0x27 => "→".into(),
        0x26 => "↑".into(),
        0x28 => "↓".into(),
        0x2E => "⌦".into(),
        0x2D => "Insert".into(),
        _ => format!("0x{vk:02X}"),
    }
}
