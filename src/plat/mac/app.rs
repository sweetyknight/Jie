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
static HOTKEY_OK: AtomicBool = AtomicBool::new(false);
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
            if let Some(a) = APP.get() { if right { (a.tray_menu)(crate::plat::cursor_pos()); } else { (a.capture)(); } }
        }
        #[unsafe(method(menuAction:))]
        fn menu_action(&self, sender: &NSMenuItem) { if let Some(a) = APP.get() { let _ = sender; let id = sender.tag(); if id != 0 { menu_result(id as usize); } else { (a.capture)(); } } }
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
}

const SHIFT_KEY: u32 = 1 << 9;
const CONTROL_KEY: u32 = 1 << 12;
const COMMAND_KEY: u32 = 1 << 8;
const OPTION_KEY: u32 = 1 << 11;
static HOTKEY_REF: std::sync::atomic::AtomicPtr<std::ffi::c_void> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
static EVENT_INSTALLED: AtomicBool = AtomicBool::new(false);

extern "C" fn carbon_hotkey(_: *mut std::ffi::c_void, _: *mut std::ffi::c_void, _: *mut std::ffi::c_void) -> i32 {
    if let Some(app) = APP.get() {
        (app.capture)();
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

pub fn set_hotkey(mods: u32, vk: u32) -> bool {
    let old = HOTKEY_REF.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if !old.is_null() {
        unsafe {
            let _ = UnregisterEventHotKey(old);
        }
    }
    let Some(code) = carbon_key(vk) else {
        HOTKEY_OK.store(false, Ordering::Release);
        return false;
    };
    let carbon_mods = (if mods & crate::settings::MOD_ALT != 0 { OPTION_KEY } else { 0 })
        | (if mods & crate::settings::MOD_SHIFT != 0 { SHIFT_KEY } else { 0 })
        | (if mods & crate::settings::MOD_CONTROL != 0 { CONTROL_KEY } else { 0 })
        | (if mods & crate::settings::MOD_WIN != 0 { COMMAND_KEY } else { 0 });
    if carbon_mods == 0 {
        HOTKEY_OK.store(false, Ordering::Release);
        return false;
    }
    unsafe {
        if !EVENT_INSTALLED.swap(true, Ordering::AcqRel) {
            let spec = EventTypeSpec { class: u32::from_be_bytes(*b"keyb"), kind: 5 };
            let mut handler = std::ptr::null_mut();
            if InstallEventHandler(GetApplicationEventTarget(), carbon_hotkey, 1, &spec, std::ptr::null_mut(), &mut handler) != 0 {
                EVENT_INSTALLED.store(false, Ordering::Release);
                HOTKEY_OK.store(false, Ordering::Release);
                return false;
            }
        }
        let mut ref_ = std::ptr::null_mut();
        let result =
            RegisterEventHotKey(code, carbon_mods, EventHotKeyID { signature: u32::from_be_bytes(*b"Jie "), id: 1 }, GetApplicationEventTarget(), 0, &mut ref_);
        let ok = result == 0;
        if ok {
            HOTKEY_REF.store(ref_, Ordering::Release);
        }
        HOTKEY_OK.store(ok, Ordering::Release);
        ok
    }
}

pub fn hotkey_ok() -> bool {
    HOTKEY_OK.load(Ordering::Relaxed)
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
        let color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
        for (a, b) in [
            ((5.0, 14.0), (5.0, 5.0)),
            ((5.0, 5.0), (14.0, 5.0)),
            ((22.0, 5.0), (30.0, 5.0)),
            ((30.0, 5.0), (30.0, 17.0)),
            ((5.0, 22.0), (5.0, 30.0)),
            ((5.0, 30.0), (17.0, 30.0)),
        ] {
            c.line(a, b, color, 3.0, Stroke::Round);
        }
        c.fill_rect(super::RectF { left: 23.0, top: 23.0, right: 32.0, bottom: 32.0 }, color);
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
