//! macOS application lifetime, status item and the small amount of main-thread glue.

use crate::plat::{App, MenuItem, Point};
use objc2::{
    AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject, Sel},
};
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBitmapImageRep, NSEventMask,
    NSEventModifierFlags, NSEventType, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
};
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSNotificationSuspensionBehavior, NSObject, NSObjectProtocol, NSSize, NSString, ns_string,
};
use std::cell::OnceCell;
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

static APP: OnceLock<App> = OnceLock::new();
/// 各个全局快捷键（按编号 1–4）现在是否注册成功
static HOTKEY_OK: [AtomicBool; 4] = [const { AtomicBool::new(false) }; 4];
static TIP: Mutex<String> = Mutex::new(String::new());

#[derive(Default)]
struct TrayIvars;

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = TrayIvars]
    struct TrayTarget;
    unsafe impl NSObjectProtocol for TrayTarget {}
    unsafe impl NSApplicationDelegate for TrayTarget {
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, _app: &NSApplication, _visible: bool) -> bool { if let Some(a)=APP.get() { (a.again)(); } true }
    }
    impl TrayTarget {
        #[unsafe(method(again:))]
        fn again(&self, _notification: &NSNotification) { if let Some(a)=APP.get() {(a.again)();} }

        #[unsafe(method(capture:))]
        fn capture(&self, _sender: Option<&AnyObject>) {
            let right = MainThreadMarker::new().and_then(|m| NSApplication::sharedApplication(m).currentEvent()).is_some_and(|e| e.r#type() == NSEventType::RightMouseUp || e.modifierFlags().contains(NSEventModifierFlags::Control));
            if let Some(a) = APP.get() { if right { (a.tray_menu)(crate::plat::cursor_pos()); } else { (a.tray_click)(); } }
        }
        #[unsafe(method(menuAction:))]
        fn menu_action(&self, sender: &NSMenuItem) { if let Some(a) = APP.get() { let id = sender.tag(); if id != 0 { menu_result(id as usize); } else { (a.tray_click)(); } } }
    }
);

thread_local! {
    static TARGET: OnceCell<Retained<TrayTarget>> = const { OnceCell::new() };
    static STATUS: OnceCell<Retained<NSStatusItem>> = const { OnceCell::new() };
}

fn target(mtm: MainThreadMarker) -> Retained<TrayTarget> {
    TARGET.with(|c| {
        c.get_or_init(|| {
            let t = TrayTarget::alloc(mtm).set_ivars(TrayIvars);
            unsafe { msg_send![super(t), init] }
        })
        .clone()
    })
}

pub fn run(app: App) {
    let mtm = MainThreadMarker::new().expect("AppKit must run on the main thread");
    let ns = NSApplication::sharedApplication(mtm);
    let _ = APP.set(app);
    let Some(dir) = super::config_dir() else {
        eprintln!("找不到用户配置目录");
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("无法打开配置目录：{e}");
        return;
    }
    let instance = match std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join("instance.lock")) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("无法检查运行状态：{e}");
            return;
        }
    };
    let center = NSDistributedNotificationCenter::defaultCenter();
    match instance.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            unsafe {
                center.postNotificationName_object_userInfo_deliverImmediately(ns_string!("com.jie.app.reopen"), None, None, true);
            }
            return;
        }
        Err(e) => {
            eprintln!("无法锁定运行状态：{e}");
            return;
        }
    }
    let t = target(mtm);
    unsafe {
        center.addObserver_selector_name_object_suspensionBehavior(
            &t,
            Sel::register(c"again:"),
            Some(ns_string!("com.jie.app.reopen")),
            None,
            NSNotificationSuspensionBehavior::DeliverImmediately,
        );
    }
    ns.setDelegate(Some(ProtocolObject::from_ref(&*t)));
    ns.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    ns.setAppearance(NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }).as_deref());
    install_edit_menu(&ns, mtm);
    ns.finishLaunching();
    (APP.get().expect("app").start)();
    ns.run();
}
fn install_edit_menu(app: &NSApplication, mtm: MainThreadMarker) {
    let main = NSMenu::new(mtm);
    let item = NSMenuItem::new(mtm);
    let edit = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("编辑"));
    for (title, key, action) in [
        ("撤销", "z", c"undo:"),
        ("重做", "Z", c"redo:"),
        ("剪切", "x", c"cut:"),
        ("复制", "c", c"copy:"),
        ("粘贴", "v", c"paste:"),
        ("全选", "a", c"selectAll:"),
    ] {
        let i = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(Sel::register(action)),
                &NSString::from_str(key),
            )
        };
        if key == "Z" {
            i.setKeyEquivalentModifierMask(NSEventModifierFlags::Command | NSEventModifierFlags::Shift);
        }
        edit.addItem(&i);
    }
    item.setSubmenu(Some(&edit));
    main.addItem(&item);
    app.setMainMenu(Some(&main));
}
pub fn quit() {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).terminate(None);
    }
}

pub fn post(f: impl FnOnce() + Send + 'static) {
    dispatch2::DispatchQueue::main().exec_async(f);
}

#[repr(C)]
struct EventTypeSpec {
    class: u32,
    kind: u32,
}
#[repr(C)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}
type CarbonHandler = extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, *mut std::ffi::c_void) -> i32;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut std::ffi::c_void;
    fn InstallEventHandler(
        target: *mut std::ffi::c_void,
        handler: CarbonHandler,
        count: usize,
        types: *const EventTypeSpec,
        user: *mut std::ffi::c_void,
        out: *mut *mut std::ffi::c_void,
    ) -> i32;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: *mut std::ffi::c_void,
        options: u32,
        out: *mut *mut std::ffi::c_void,
    ) -> i32;
    fn UnregisterEventHotKey(hot_key: *mut std::ffi::c_void) -> i32;
    fn GetEventParameter(
        event: *mut std::ffi::c_void,
        name: u32,
        desired: u32,
        actual: *mut u32,
        size: usize,
        actual_size: *mut usize,
        data: *mut std::ffi::c_void,
    ) -> i32;
}

const SHIFT_KEY: u32 = 1 << 9;
const CONTROL_KEY: u32 = 1 << 12;
const COMMAND_KEY: u32 = 1 << 8;
const OPTION_KEY: u32 = 1 << 11;
static HOTKEY_REF: [std::sync::atomic::AtomicPtr<std::ffi::c_void>; 4] = [const { std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()) }; 4];
static EVENT_INSTALLED: AtomicBool = AtomicBool::new(false);

/// 按下了注册过的快捷键：从事件里读出注册时给的编号
extern "C" fn carbon_hotkey(_: *mut std::ffi::c_void, event: *mut std::ffi::c_void, _: *mut std::ffi::c_void) -> i32 {
    let mut id = EventHotKeyID { signature: 0, id: 0 };
    // kEventParamDirectObject（'----'）、typeEventHotKeyID（'hkid'）
    let got = unsafe {
        GetEventParameter(
            event,
            u32::from_be_bytes(*b"----"),
            u32::from_be_bytes(*b"hkid"),
            std::ptr::null_mut(),
            size_of::<EventHotKeyID>(),
            std::ptr::null_mut(),
            &mut id as *mut _ as *mut std::ffi::c_void,
        )
    };
    if got == 0
        && let Some(app) = APP.get()
    {
        (app.hotkey)(id.id as usize);
    }
    0
}

pub(crate) fn carbon_key(vk: u32) -> Option<u32> {
    // Carbon uses the hardware key code while settings persist Windows VKs.
    Some(match vk {
        0x41 => 0,
        0x42 => 11,
        0x43 => 8,
        0x44 => 2,
        0x45 => 14,
        0x46 => 3,
        0x47 => 5,
        0x48 => 4,
        0x49 => 34,
        0x4A => 38,
        0x4B => 40,
        0x4C => 37,
        0x4D => 46,
        0x4E => 45,
        0x4F => 31,
        0x50 => 35,
        0x51 => 12,
        0x52 => 15,
        0x53 => 1,
        0x54 => 17,
        0x55 => 32,
        0x56 => 9,
        0x57 => 13,
        0x58 => 7,
        0x59 => 16,
        0x5A => 6,
        0x30 => 29,
        0x31 => 18,
        0x32 => 19,
        0x33 => 20,
        0x34 => 21,
        0x35 => 23,
        0x36 => 22,
        0x37 => 26,
        0x38 => 28,
        0x39 => 25,
        0x70..=0x7B => [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111][(vk - 0x70) as usize],
        0x7C..=0x83 => [105, 107, 113, 106, 64, 79, 80, 90][(vk - 0x7C) as usize],
        0x20 => 49,
        0x21 => 116,
        0x22 => 121,
        0x23 => 119,
        0x24 => 115,
        0x25 => 123,
        0x26 => 126,
        0x27 => 124,
        0x28 => 125,
        0x2E => 117,
        _ => return None,
    })
}

/// 换编号 `id`（1–4）的全局快捷键（先注销旧的）；返回是否注册成功。按下时调 `App::hotkey(id)`
pub fn set_hotkey(id: usize, mods: u32, vk: u32) -> bool {
    use crate::plat::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};
    let (Some(slot), Some(ok_slot)) = (HOTKEY_REF.get(id.wrapping_sub(1)), HOTKEY_OK.get(id.wrapping_sub(1))) else { return false };
    let old = slot.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if !old.is_null() {
        unsafe {
            let _ = UnregisterEventHotKey(old);
        }
    }
    let Some(code) = carbon_key(vk) else {
        ok_slot.store(false, Ordering::Release);
        return false;
    };
    let carbon_mods = (if mods & MOD_ALT != 0 { OPTION_KEY } else { 0 })
        | (if mods & MOD_SHIFT != 0 { SHIFT_KEY } else { 0 })
        | (if mods & MOD_CONTROL != 0 { CONTROL_KEY } else { 0 })
        | (if mods & MOD_WIN != 0 { COMMAND_KEY } else { 0 });
    if carbon_mods == 0 {
        ok_slot.store(false, Ordering::Release);
        return false;
    }
    if !install_handler() {
        ok_slot.store(false, Ordering::Release);
        return false;
    }
    unsafe {
        let mut ref_ = std::ptr::null_mut();
        let hk_id = EventHotKeyID { signature: u32::from_be_bytes(*b"Jie "), id: id as u32 };
        let ok = RegisterEventHotKey(code, carbon_mods, hk_id, GetApplicationEventTarget(), 0, &mut ref_) == 0;
        if ok {
            slot.store(ref_, Ordering::Release);
        }
        ok_slot.store(ok, Ordering::Release);
        ok
    }
}

/// 装上收快捷键事件的处理函数（只装一次）；装不上返回 false
fn install_handler() -> bool {
    if EVENT_INSTALLED.swap(true, Ordering::AcqRel) {
        return true;
    }
    let spec = EventTypeSpec { class: u32::from_be_bytes(*b"keyb"), kind: 5 };
    let mut handler = std::ptr::null_mut();
    if unsafe { InstallEventHandler(GetApplicationEventTarget(), carbon_hotkey, 1, &spec, std::ptr::null_mut(), &mut handler) } != 0 {
        EVENT_INSTALLED.store(false, Ordering::Release);
        return false;
    }
    true
}

/// 长截图临时占用的那几个键注册下来的句柄（指针存成整数才能放进静态变量）
static GRABBED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// 临时占用 ↑ ↓ Esc Return（不带修饰键），按下时调 `App::hotkey(KEY_*)`；`on` 为 false 时放掉。
/// Carbon 的快捷键不连发，按住 ↑↓ 只算一下
pub fn grab_keys(on: bool) {
    use crate::plat::{KEY_DOWN, KEY_ENTER, KEY_ESC, KEY_UP};
    let mut held = GRABBED.lock().unwrap_or_else(|e| e.into_inner());
    for r in held.drain(..) {
        unsafe {
            let _ = UnregisterEventHotKey(r as *mut std::ffi::c_void);
        }
    }
    if !on || !install_handler() {
        return;
    }
    // kVK_UpArrow、kVK_DownArrow、kVK_Escape、kVK_Return
    for (id, code) in [(KEY_UP, 126), (KEY_DOWN, 125), (KEY_ESC, 53), (KEY_ENTER, 36)] {
        let mut r = std::ptr::null_mut();
        let hk = EventHotKeyID { signature: u32::from_be_bytes(*b"Jie "), id: id as u32 };
        if unsafe { RegisterEventHotKey(code, 0, hk, GetApplicationEventTarget(), 0, &mut r) } == 0 {
            held.push(r as usize);
        }
    }
}

pub fn hotkey_ok(id: usize) -> bool {
    HOTKEY_OK.get(id.wrapping_sub(1)).is_some_and(|s| s.load(Ordering::Relaxed))
}

pub fn add_tray(tip: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    *TIP.lock().unwrap() = tip.to_owned();
    let status = STATUS.with(|c| c.get_or_init(|| NSStatusBar::systemStatusBar().statusItemWithLength(-1.0)).clone());
    if let Some(button) = status.button(mtm) {
        if let Some(icon) = tray_icon() {
            button.setImage(Some(&icon));
        } else {
            button.setTitle(ns_string!("Jie"));
        }
        button.setToolTip(Some(&NSString::from_str(tip)));
        let t = target(mtm);
        let obj: &AnyObject = &*t;
        unsafe {
            button.setTarget(Some(obj));
            button.setAction(Some(Sel::register(c"capture:")));
        }
        button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
    }
}

pub fn set_tray_tip(tip: &str) {
    *TIP.lock().unwrap() = tip.to_owned();
    if let Some(mtm) = MainThreadMarker::new() {
        STATUS.with(|s| {
            if let Some(status) = s.get() {
                if let Some(button) = status.button(mtm) {
                    button.setToolTip(Some(&NSString::from_str(tip)));
                }
            }
        });
    }
}

pub fn notify(title: &str, text: &str, click: Option<fn()>) {
    eprintln!("{title}: {text}");
    if let Some(app) = APP.get() {
        (app.notify)(title, text, click);
    }
}
fn tray_icon() -> Option<Retained<NSImage>> {
    use super::{Bitmap, Canvas, Color, Stroke};
    let bitmap = Bitmap::new(36, 36)?;
    {
        let c = Canvas::new(&bitmap, bitmap.full(), true);
        c.clear(Color::default());
        // 应用图标「取景框」的单色版（scripts/macos/icon.swift）：三个角 + 右下角圆角方块，按 100 网格 × 0.36 取整
        let color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
        for (a, b) in [
            ((5.5, 15.0), (5.5, 5.5)),
            ((5.5, 5.5), (15.0, 5.5)),
            ((21.0, 5.5), (29.5, 5.5)),
            ((29.5, 5.5), (29.5, 14.5)),
            ((5.5, 21.0), (5.5, 29.5)),
            ((5.5, 29.5), (14.5, 29.5)),
        ] {
            c.line(a, b, color, 3.5, Stroke::Round);
        }
        c.fill_round(super::RectF { left: 19.5, top: 19.5, right: 30.5, bottom: 30.5 }, 2.0, color);
    }
    let cg = bitmap.image()?;
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
    let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(18.0, 18.0));
    image.addRepresentation(&rep);
    image.setTemplate(true);
    Some(image)
}

thread_local! { static MENU_COMMAND: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
fn menu_result(id: usize) {
    MENU_COMMAND.with(|v| v.set(id));
}

pub fn tray_menu(items: &[MenuItem], at: Point) -> usize {
    let Some(mtm) = MainThreadMarker::new() else {
        return 0;
    };
    MENU_COMMAND.with(|v| v.set(0));
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Jie"));
    menu.setAutoenablesItems(false);
    let t = target(mtm);
    for item in items {
        if item.id == 0 {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            continue;
        }
        let title = if item.key.is_empty() { item.text.to_owned() } else { format!("{}\t{}", item.text, item.key) };
        let mi = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(&title),
                Some(Sel::register(c"menuAction:")),
                ns_string!(""),
            )
        };
        mi.setTag(item.id as isize);
        mi.setEnabled(item.enabled);
        mi.setState(if item.checked { 1 } else { 0 });
        if item.default {
            let font = objc2_app_kit::NSFont::boldSystemFontOfSize(13.0);
            let attrs = objc2_foundation::NSDictionary::from_slices(&[unsafe { objc2_app_kit::NSFontAttributeName }], &[&*font]);
            let title = unsafe {
                objc2_foundation::NSAttributedString::initWithString_attributes(
                    objc2_foundation::NSAttributedString::alloc(),
                    &NSString::from_str(&title),
                    Some(attrs.cast_unchecked()),
                )
            };
            mi.setAttributedTitle(Some(&title));
        }
        let obj: &AnyObject = &*t;
        unsafe {
            mi.setTarget(Some(obj));
        }
        menu.addItem(&mi);
    }
    menu.popUpMenuPositioningItem_atLocation_inView(None, super::screen::cocoa_point(at), None);
    MENU_COMMAND.with(|v| v.get())
}
