//! settings.json 读写。API Key、开机自启在系统里存（见 plat），不在这里。
//! 配置只在 UI 线程上读写；工作线程要用的值在派活时拷过去。

use crate::plat;
use crate::translate::{self, Lang};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

pub use crate::plat::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};

/// 全局快捷键：修饰键（MOD_*）和 Windows 的虚拟键码
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    pub mods: u32,
    pub vk: u32,
}

impl Hotkey {
    /// 「Alt + Shift + A」这样的名字（顺序同 Windows：Ctrl、Alt、Shift）；`sep` 是连接符
    pub fn name(self, sep: &str) -> String {
        plat::hotkey_name(self.mods, self.vk, sep)
    }

    /// 至少带 Ctrl 或 Alt，否则会抢掉普通按键
    pub fn valid(self) -> bool {
        self.vk != 0 && self.vk < 0x100 && self.mods & (MOD_CONTROL | MOD_ALT | if cfg!(target_os="macos") {MOD_WIN} else {0}) != 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Png,
    Jpeg,
}

/// 写进 settings.json 的设置。开机自启以系统为准，API Key 在系统的凭据存储里，都不在这里。
/// 故意不 derive Debug：将来有人加了带密钥的字段也不会被打进日志。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub hotkey: Hotkey,
    /// 录屏快捷键：打开录屏选区，录制中再按一次停止；不能和截图快捷键相同
    pub record_hotkey: Hotkey,
    /// 录屏帧率，FPS_CHOICES 里的一个
    pub record_fps: u32,
    /// 录好的动图钉在屏幕上时外面画一圈选区颜色的框（和屏幕内容一样时看得清范围）
    pub gif_frame: bool,
    /// 选区颜色 0xRRGGBB
    pub sel_color: u32,
    /// 保存对话框的起始文件夹；空表示系统的「图片」文件夹
    pub save_dir: String,
    pub save_format: Format,
    /// 取字的服务地址、模型；留空就用翻译的。旧版的 ocr_lang、ocr_llm 读到了也忽略（见 `translate::Config::overlay`）
    pub ocr_endpoint: String,
    pub ocr_model: String,
    pub endpoint: String,
    pub model: String,
    pub target: Lang,
}

pub const DEFAULT_COLOR: u32 = 0x9C44EC;
/// 设置里能选的录屏帧率
pub const FPS_CHOICES: [u32; 4] = [10, 15, 20, 30];

impl Default for Settings {
    fn default() -> Self {
        Settings {
            hotkey: Hotkey { mods: MOD_ALT | MOD_SHIFT, vk: 'A' as u32 },
            record_hotkey: Hotkey { mods: MOD_ALT | MOD_SHIFT, vk: 'S' as u32 },
            record_fps: 15,
            gif_frame: true,
            sel_color: DEFAULT_COLOR,
            save_dir: String::new(),
            save_format: Format::Png,
            ocr_endpoint: String::new(),
            ocr_model: String::new(),
            endpoint: String::new(),
            model: String::new(),
            target: Lang::Zh,
        }
    }
}

impl Settings {
    /// 手改出来的非法值换回默认值
    fn sanitized(mut self) -> Self {
        let d = Settings::default();
        if !self.hotkey.valid() {
            self.hotkey = d.hotkey;
        }
        if !self.record_hotkey.valid() || self.record_hotkey == self.hotkey {
            self.record_hotkey = if d.record_hotkey == self.hotkey { d.hotkey } else { d.record_hotkey };
        }
        if !FPS_CHOICES.contains(&self.record_fps) {
            self.record_fps = d.record_fps;
        }
        self.sel_color &= 0xFF_FFFF;
        self
    }

    /// 翻译服务，带上系统里存的 Key
    pub fn translate_service(&self) -> translate::Config {
        translate::Config { endpoint: self.endpoint.clone(), model: self.model.clone(), api_key: plat::api_key(TRANSLATE_KEY), target: self.target }
    }

    /// 取字实际用的服务
    pub fn ocr_service(&self) -> translate::Config {
        self.translate_service().overlay(&self.ocr_endpoint, &self.ocr_model, plat::api_key(OCR_KEY))
    }

    /// 保存对话框实际用的起始文件夹
    pub fn save_dir(&self) -> String {
        if self.save_dir.is_empty() { plat::pictures_dir() } else { self.save_dir.clone() }
    }
}

thread_local! {
    static CURRENT: RefCell<Settings> = RefCell::new(Settings::default());
}

pub fn get() -> Settings {
    CURRENT.with_borrow(|s| s.clone())
}

pub fn sel_color() -> u32 {
    CURRENT.with_borrow(|s| s.sel_color)
}

fn path() -> Option<PathBuf> {
    Some(plat::config_dir()?.join("settings.json"))
}

/// 启动时读一次
pub fn load() {
    if let Some(p) = path() {
        CURRENT.set(read(&p));
    }
}

/// 没有文件就用默认值；文件坏了先备份成 settings.json.bad 再用默认值，不让它挡住启动
fn read(p: &Path) -> Settings {
    let Ok(bytes) = std::fs::read(p) else { return Settings::default() };
    match serde_json::from_slice::<Settings>(&bytes) {
        Ok(s) => s.sanitized(),
        Err(e) => {
            eprintln!("设置文件读不了（{e}），已备份成 settings.json.bad，改用默认值");
            let _ = std::fs::rename(p, p.with_extension("json.bad"));
            Settings::default()
        }
    }
}

/// 换成新设置并写盘；写盘失败时内存里的设置照样生效
pub fn set(s: Settings) -> Result<(), String> {
    CURRENT.set(s.clone());
    let p = path().ok_or("找不到存设置的文件夹")?;
    write(&p, &s).map_err(|e| e.to_string())
}

/// 先写临时文件再改名，写到一半断电也不会把原来的设置弄坏
fn write(p: &Path, s: &Settings) -> std::io::Result<()> {
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s).unwrap_or_default())?;
    std::fs::rename(&tmp, p)
}

/// 保存成功后把那个文件夹记成下次的起始位置
pub fn remember_save_dir(dir: &Path) {
    let mut s = get();
    let d = dir.to_string_lossy().into_owned();
    if s.save_dir != d {
        s.save_dir = d;
        if let Err(e) = set(s) {
            eprintln!("记不住保存位置：{e}");
        }
    }
}

// ---- API Key 在系统的凭据存储里（Windows 凭据管理器、mac 钥匙串），按这两个名字存 ----

pub const TRANSLATE_KEY: &str = "Jie/translate-api-key";
/// 大模型取字单独填了服务地址时用的 Key
pub const OCR_KEY: &str = "Jie/ocr-api-key";

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        // 放在 target 下：项目目录是低完整性的，从这里跑的测试写不了 %TEMP%
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp").join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_file_gives_defaults() {
        let p = dir("missing").join("settings.json");
        assert!(read(&p) == Settings::default());
    }

    #[test]
    fn broken_file_is_backed_up() {
        let d = dir("broken");
        let p = d.join("settings.json");
        std::fs::write(&p, "{ not json").unwrap();
        assert!(read(&p) == Settings::default());
        assert!(!p.exists());
        assert_eq!(std::fs::read_to_string(d.join("settings.json.bad")).unwrap(), "{ not json");
    }

    #[test]
    fn partial_and_invalid_values() {
        let p = dir("partial").join("settings.json");
        // 缺的字段用默认值，不认识的字段（包括 4.8 删掉的 ocr_lang、ocr_llm）忽略，不合规的快捷键换回默认
        std::fs::write(&p, r#"{"sel_color": 15022139, "model": "m", "extra": 1, "ocr_lang": "zh-Hans-CN", "ocr_llm": false, "hotkey": {"mods": 4, "vk": 65}, "record_fps": 7}"#).unwrap();
        let s = read(&p);
        assert_eq!((s.sel_color, s.model.as_str(), s.record_fps), (0xE5383B, "m", 15));
        assert!(s.hotkey == Settings::default().hotkey);
        assert!(s.save_format == Format::Png && s.target == Lang::Zh);
    }

    /// 录屏快捷键和截图快捷键撞了：换回录屏的默认值；截图快捷键正好是录屏的默认值时，录屏用截图的默认值
    #[test]
    fn record_hotkey_never_equals_capture() {
        let d = Settings::default();
        let same = Settings { record_hotkey: d.hotkey, ..d.clone() }.sanitized();
        assert!(same.record_hotkey == d.record_hotkey);
        let swapped = Settings { hotkey: d.record_hotkey, record_hotkey: d.record_hotkey, ..d.clone() }.sanitized();
        assert!(swapped.hotkey == d.record_hotkey && swapped.record_hotkey == d.hotkey);
    }

    #[test]
    fn round_trip_without_key() {
        let p = dir("round").join("settings.json");
        let s = Settings { endpoint: "https://x.com/v1".into(), save_format: Format::Jpeg, target: Lang::Ja, ..Settings::default() };
        write(&p, &s).unwrap();
        assert!(read(&p) == s);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("\"jpeg\"") && text.contains("\"ja\""), "{text}");
        assert!(!text.contains("api_key") && !text.contains("\"key\""), "{text}");
    }

    #[test]
    #[cfg(windows)]
    fn hotkey_names() {
        assert_eq!(Settings::default().hotkey.name(" + "), "Alt + Shift + A");
        assert_eq!(Settings::default().record_hotkey.name("+"), "Alt+Shift+S");
        assert_eq!(Hotkey { mods: MOD_CONTROL | MOD_SHIFT, vk: 'A' as u32 }.name("+"), "Ctrl+Shift+A");
        assert_eq!(Hotkey { mods: MOD_ALT, vk: 0x71 }.name("+"), "Alt+F2");
        assert_eq!(Hotkey { mods: MOD_CONTROL | MOD_ALT, vk: '1' as u32 }.name("+"), "Ctrl+Alt+1");
    }
    #[test]
    #[cfg(target_os="macos")]
    fn hotkey_names_mac() {
        assert_eq!(Settings::default().hotkey.name(" + "),"⌥⇧A");
        assert_eq!(Hotkey {mods:MOD_CONTROL|MOD_SHIFT,vk:65}.name("+"),"⌃⇧A");
        assert_eq!(Hotkey {mods:MOD_ALT,vk:0x71}.name("+"),"⌥F2");
        assert_eq!(Hotkey {mods:MOD_WIN,vk:65}.name("+"),"⌘A");
        assert!(Hotkey {mods:MOD_WIN,vk:65}.valid());
        assert!(!Hotkey {mods:MOD_SHIFT,vk:65}.valid());
    }

}
