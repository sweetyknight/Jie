//! 系统里存的东西：API Key（凭据管理器）、开机自启（注册表）、配置目录、图片目录；还有按键名、本地时间。

use std::path::PathBuf;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::Security::Credentials::*;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::*;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyNameTextW, MAPVK_VK_TO_VSC, MapVirtualKeyW};
use windows::Win32::UI::Shell::{FOLDERID_Pictures, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::core::{PCWSTR, PWSTR, w};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// %APPDATA%\Jie
pub fn config_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Jie"))
}

/// 系统的「图片」文件夹
pub fn pictures_dir() -> String {
    unsafe {
        let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None) else { return String::new() };
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

/// 本地时间「yyyyMMdd_HHmmss」，默认文件名用
pub fn now_stamp() -> String {
    let t = unsafe { GetLocalTime() };
    format!("{:04}{:02}{:02}_{:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

// ---- API Key：Windows 凭据管理器，普通凭据，只有当前用户能读 ----

/// API Key 存在哪（设置窗口的提示里用）
pub const KEY_STORE: &str = " Windows 凭据管理器";

/// `which` 是凭据名，比如「Jie/translate-api-key」；没有时为空
pub fn api_key(which: &str) -> String {
    let target = wide(which);
    unsafe {
        let mut p: *mut CREDENTIALW = std::ptr::null_mut();
        if CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut p).is_err() || p.is_null() {
            return String::new();
        }
        let c = &*p;
        let key = if c.CredentialBlob.is_null() {
            String::new()
        } else {
            String::from_utf8_lossy(std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)).into_owned()
        };
        CredFree(p as _);
        key
    }
}

/// 空字符串表示删掉
pub fn set_api_key(which: &str, key: &str) -> Result<(), String> {
    let mut target = wide(which);
    let r = unsafe {
        if key.is_empty() {
            match CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) {
                Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
                r => r,
            }
        } else {
            let cred = CREDENTIALW {
                Type: CRED_TYPE_GENERIC,
                TargetName: PWSTR(target.as_mut_ptr()),
                CredentialBlobSize: key.len() as u32,
                CredentialBlob: key.as_ptr() as *mut u8,
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                ..Default::default()
            };
            CredWriteW(&cred, 0)
        }
    };
    r.map_err(|e| e.message())
}

// ---- 开机自启：HKCU\...\Run 里的 Jie ----

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

pub fn autostart() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Jie"), RRF_RT_REG_SZ, None, None, None) == ERROR_SUCCESS }
}

/// 打开时总是写当前 exe 的路径：便携版挪了位置，重新确定一次就能修好
pub fn set_autostart(on: bool) -> Result<(), String> {
    let err = unsafe {
        if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let v = wide(&format!("\"{}\"", exe.display()));
            RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Jie"), REG_SZ.0, Some(v.as_ptr() as _), (v.len() * 2) as u32)
        } else {
            match RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Jie")) {
                ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
                e => e,
            }
        }
    };
    if err == ERROR_SUCCESS { Ok(()) } else { Err(windows::core::Error::from_hresult(err.to_hresult()).message()) }
}

// ---- 按键名 ----

/// 方向键、Insert、Delete 这些在扫描码里属于扩展键，取名字和热键控件都要带上扩展标志
pub fn extended(vk: u32) -> bool {
    matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x6F | 0x90)
}

/// 虚拟键码的名字：字母数字、F1–F24 自己写，其余问系统（跟着键盘布局和系统语言）
pub fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from(vk as u8).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => unsafe {
            let scan = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) as i32;
            let mut buf = [0u16; 64];
            let n = GetKeyNameTextW((scan << 16) | (i32::from(extended(vk)) << 24), &mut buf);
            if n > 0 { String::from_utf16_lossy(&buf[..n as usize]) } else { format!("0x{vk:02X}") }
        },
    }
}
