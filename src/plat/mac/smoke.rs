//! Main-thread AppKit integration checks. Run with sh scripts/macos/test.sh to provide an app bundle.
use crate::*;
use objc2::{MainThreadMarker, msg_send};
use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
use objc2_foundation::{NSPoint, NSRange, NSString};
use plat::{Canvas, Event, Handler, Kind, Point, Rect, Spec, Win};
#[derive(Default)]
struct Probe {
    ticks: usize,
    committed: String,
    marked: String,
}
impl Handler for Probe {
    fn event(&mut self, w: Win, e: Event) {
        match e {
            Event::Timer(id) => {
                self.ticks += 1;
                w.kill_timer(id);
            }
            Event::Ime { result, comp, .. } => {
                if let Some(s) = result {
                    self.committed.push_str(&s);
                }
                self.marked = comp;
            }
            Event::Char(c) => self.committed.push(c),
            Event::Close => w.close(),
            _ => {}
        }
    }
    fn paint(&mut self, _: Win, c: &Canvas) {
        c.fill_rect(ui::rectf(0.0, 0.0, 400.0, 240.0), ui::rgb(0xFFFFFF));
        if let Some(t) = plat::Text::new(plat::Face::Ui, 24.0, false, "Jie · 中文 English 😀") {
            c.text(&t, 12.0, 16.0, ui::rgb(0x9C44EC));
        }
    }
}
pub fn run() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(30));
        eprintln!("mac smoke timed out");
        std::process::exit(1);
    });
    let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
    app.setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Accessory);
    app.finishLaunching();
    start();
    app.run();
}
fn start() {
    settings::set(settings::Settings { hotkey: settings::Hotkey { mods: 11, vk: 0x83 }, ..Default::default() }).unwrap();
    let autostart = plat::autostart();
    let enabled = plat::set_autostart(true);
    let registered = plat::autostart();
    let repeated = plat::set_autostart(true);
    let restored = plat::set_autostart(autostart);
    assert!(restored.is_ok(), "restore login item: {restored:?}");
    assert!(enabled.is_ok() && registered, "register login item: {enabled:?}");
    assert!(repeated.is_ok(), "register an existing login item: {repeated:?}");
    eprintln!("login item registration and cleanup passed");
    plat::warm_up();
    let mon = plat::monitor_at(Point::default());
    let r = Rect { left: mon.work.left + 100, top: mon.work.top + 100, right: mon.work.left + 500, bottom: mon.work.top + 340 };
    let w = Win::open(Spec { kind: Kind::Settings, rect: r, title: "Jie smoke", owner: None }, Probe::default()).unwrap();
    w.field(1, plat::FieldKind::Text, "输入");
    w.field(2, plat::FieldKind::Password, "密钥");
    w.field(3, plat::FieldKind::Hotkey, "");
    for i in 1..=3 {
        w.place_field(i, Rect { left: 12, top: 60 + i as i32 * 40, right: 380, bottom: 92 + i as i32 * 40 }, w.scale());
        w.show_field(i, true);
    }
    w.set_field_text(1, "中文 and English");
    assert_eq!(w.field_text(1), "中文 and English");
    w.set_hotkey_field(3, 5, 65);
    assert_eq!(w.hotkey_field(3), (5, 65));
    w.show();
    w.focus_field(Some(1));
    assert_eq!(w.focused_field(), Some(1));
    w.focus_field(None);
    w.ime(Some(Rect { left: 20, top: 20, right: 22, bottom: 50 }));
    let ns = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
    let window = ns.windows().iter().find(|n| n.title().to_string() == "Jie smoke").unwrap();
    let v = window.contentView().unwrap();
    unsafe {
        let _: () = msg_send![&*v,setMarkedText:&*NSString::from_str("pin"),selectedRange:NSRange::new(3,0),replacementRange:NSRange::new(usize::MAX>>1,0)];
    }
    assert_eq!(w.with(|s: &mut Probe| s.marked.clone()).unwrap(), "pin");
    unsafe {
        let _: () = msg_send![&*v,insertText:&*NSString::from_str("拼"),replacementRange:NSRange::new(usize::MAX>>1,0)];
    }
    assert_eq!(w.with(|s: &mut Probe| s.committed.clone()).unwrap(), "拼");
    w.ime(None);
    w.focus_field(Some(3));
    w.timer(1, 20);
    w.timer(2, 30);
    w.timer(2, 100);
    let child = Win::open(Spec { kind: Kind::PinBar, rect: r, title: "child", owner: Some(w) }, Probe::default()).unwrap();
    child.show();
    let disposable = Win::open(Spec { kind: Kind::Pin, rect: r, title: "disposable", owner: None }, Probe::default()).unwrap();
    disposable.with(|_: &mut Probe| disposable.close());
    assert!(!Win::all(Kind::Pin).contains(&disposable));
    let _ = dispatch2::DispatchQueue::main().after(dispatch2::DispatchTime::NOW.time(300_000_000), move || {
        let ns = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
        let native = ns.windows().iter().find(|n| n.title().to_string() == "Jie smoke").unwrap();
        assert_eq!(w.focused_field(), Some(3));
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown,
            NSPoint::default(),
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
            0.0,
            native.windowNumber(),
            None,
            &NSString::from_str("b"),
            &NSString::from_str("b"),
            false,
            11,
        )
        .unwrap();
        ns.sendEvent(&event);
        assert_eq!(w.hotkey_field(3), (12, 66));
        assert_eq!(w.with(|s: &mut Probe| s.ticks), Some(2));
        w.close();
        assert!(!Win::all(Kind::PinBar).contains(&child));
        settings_ui::show(false);
        let settings = *Win::all(Kind::Settings).first().unwrap();
        let native = ns.windows().iter().find(|n| n.isVisible() && n.title().to_string().contains("设置")).unwrap();
        check_settings(settings, &native);
        let view = native.contentView().unwrap();
        let rep = view.bitmapImageRepForCachingDisplayInRect(view.bounds()).unwrap();
        view.cacheDisplayInRect_toBitmapImageRep(view.bounds(), &rep);
        let props = objc2_foundation::NSDictionary::<objc2::runtime::AnyObject, objc2::runtime::AnyObject>::dictionary();
        let data = unsafe { rep.representationUsingType_properties(objc2_app_kit::NSBitmapImageFileType::PNG, props.cast_unchecked()) }.unwrap();
        std::fs::create_dir_all("target/test-tmp").unwrap();
        std::fs::write("target/test-tmp/mac-settings.png", data.to_vec()).unwrap();
        click(&native, 588.0, 454.0);
        assert!(Win::all(Kind::Settings).is_empty(), "cancel must close settings");
        assert_eq!(settings::sel_color(), settings::DEFAULT_COLOR, "cancel must discard color changes");
        settings_ui::show(false);
        let native = ns.windows().iter().find(|n| n.isVisible() && n.title().to_string().contains("设置")).unwrap();
        click(&native, 302.0, 172.0);
        click(&native, 508.0, 454.0);
        assert!(Win::all(Kind::Settings).is_empty(), "OK must apply settings and close");
        settings::load();
        assert_eq!(settings::sel_color(), 0xE0389A, "OK must persist the selected color");
        check_overlay_escape();
        check_pin();
        check_clipboard();
        check_save_dialog(0);
    });
}

fn check_overlay_escape() {
    let mon = plat::monitor_at(Point::default());
    let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
    for phase in ["selected", "dragging", "hover"] {
        let frame = plat::Bitmap::new(mon.rect.right - mon.rect.left, mon.rect.bottom - mon.rect.top).unwrap();
        Canvas::new(&frame, frame.full(), false).clear(ui::rgb(0xF0F0F0));
        overlay::show_frame(overlay::Mode::Shot, mon, frame, std::time::Instant::now());
        let w = Win::all(Kind::Overlay)[0];
        let native = app.windows().iter().find(|n| n.isVisible() && n.title().to_string() == "Jie 截图").unwrap();
        let view = native.contentView().unwrap();
        if phase != "hover" {
            for (typ, x, y) in [
                (NSEventType::LeftMouseDown, 50.0, 50.0),
                (NSEventType::LeftMouseDragged, 300.0, 200.0),
                (NSEventType::LeftMouseUp, 300.0, 200.0),
            ] {
                if phase == "dragging" && typ == NSEventType::LeftMouseUp {
                    break;
                }
                let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                    typ, view.convertPoint_toView(NSPoint::new(x, y), None), NSEventModifierFlags::empty(),
                    0.0, native.windowNumber(), None, 0, 1, 1.0,
                ).unwrap();
                match typ {
                    NSEventType::LeftMouseDown => view.mouseDown(&event),
                    NSEventType::LeftMouseDragged => view.mouseDragged(&event),
                    _ => view.mouseUp(&event),
                }
            }
        }
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown, NSPoint::default(), NSEventModifierFlags::empty(),
            0.0, native.windowNumber(), None, &NSString::from_str("\u{1b}"), &NSString::from_str("\u{1b}"), false, 0x35,
        ).unwrap();
        app.sendEvent(&event);
        assert!(!Win::all(Kind::Overlay).contains(&w), "one Esc must close the screenshot while {phase}");
    }
    eprintln!("overlay Esc passed: selected region, active drag and no selection");
}

fn check_pin() {
    let mon = plat::monitor_at(Point::default());
    let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
    for (w, h) in [(320, 180), (321, 180), (320, 181), (321, 181)] {
        let at = Point { x: mon.work.left + 400, y: mon.work.top + 200 };
        let image = plat::Bitmap::new(w, h).unwrap();
        plat::Canvas::new(&image, image.full(), false).clear(ui::rgb(0x121212));
        pin::create(image, at, annotate::Editor::new(annotate::Doc::default(), mon.scale()));
        let pin = Win::all(Kind::Pin)[0];
        let window = app.windows().iter().find(|w| w.isVisible() && w.title().to_string() == "Jie 钉图").unwrap();
        check_pin_pixels(&window, w, h);
        pin.move_to(at.x + 1, at.y + 1);
        check_pin_pixels(&window, w, h);
        for (delta, zoom) in [(1, 110), (-3, 80), (2, 100)] {
            let event =
                objc2_core_graphics::CGEvent::new_scroll_wheel_event2(None, objc2_core_graphics::CGScrollEventUnit::Line, 1, delta, 0, 0)
                    .unwrap();
            window.contentView().unwrap().scrollWheel(&NSEvent::eventWithCGEvent(&event).unwrap());
            check_pin_pixels(&window, (w * zoom + 50) / 100, (h * zoom + 50) / 100);
        }
        pin.close();
    }
    eprintln!("pin edges passed: odd/even dimensions, moving and 80/100/110% zoom");
}

fn check_pin_pixels(window: &objc2_app_kit::NSWindow, w: i32, h: i32) {
    assert!(!window.isOpaque());
    assert_eq!(window.backgroundColor().alphaComponent(), 0.0);
    let view = window.contentView().unwrap();
    let rep = view.bitmapImageRepForCachingDisplayInRect(view.bounds()).unwrap();
    view.cacheDisplayInRect_toBitmapImageRep(view.bounds(), &rep);
    let (w, h) = (w as isize, h as isize);
    let scale = window.backingScaleFactor() as isize;
    assert!((w..=w + scale).contains(&rep.pixelsWide()));
    assert!((h..=h + scale).contains(&rep.pixelsHigh()));
    for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
        let color = rep.colorAtX_y(x, y).unwrap();
        assert_eq!(color.alphaComponent(), 1.0);
        assert!((color.greenComponent() - 18.0 / 255.0).abs() < 0.02, "pin must reach its edges without a white frame");
    }
    for y in 0..rep.pixelsHigh() {
        for x in if y < h { w } else { 0 }..rep.pixelsWide() {
            assert_eq!(rep.colorAtX_y(x, y).unwrap().alphaComponent(), 0.0, "rounded window padding at ({x}, {y}) must be transparent");
        }
    }
}

fn check_clipboard() {
    let pb = objc2_app_kit::NSPasteboard::generalPasteboard();
    let original: objc2::rc::Retained<objc2_foundation::NSArray<objc2::runtime::AnyObject>> = unsafe {
        let items: objc2::rc::Retained<objc2_foundation::NSArray<objc2::runtime::AnyObject>> = msg_send![&*pb, pasteboardItems];
        let copies: Vec<objc2::rc::Retained<objc2::runtime::AnyObject>> = items
            .iter()
            .map(|item| {
                let copy: objc2::rc::Retained<objc2::runtime::AnyObject> = msg_send![objc2::class!(NSPasteboardItem), new];
                let types: objc2::rc::Retained<objc2_foundation::NSArray<NSString>> = msg_send![&*item, types];
                for typ in types.iter() {
                    let data: Option<objc2::rc::Retained<objc2_foundation::NSData>> = msg_send![&*item, dataForType: &*typ];
                    if let Some(data) = data {
                        let _: bool = msg_send![&*copy, setData: &*data, forType: &*typ];
                    }
                }
                copy
            })
            .collect();
        objc2_foundation::NSArray::from_retained_slice(&copies)
    };
    let text_result = plat::copy_text("Jie 中文 😀");
    let pasted = plat::paste_text();
    let img = plat::Image { w: 1, h: 1, px: vec![90, 165, 35, 255] };
    let image_result = plat::copy_image(&img);
    let png = pb.dataForType(unsafe { objc2_app_kit::NSPasteboardTypePNG });
    let tiff = pb.dataForType(unsafe { objc2_app_kit::NSPasteboardTypeTIFF });
    pb.clearContents();
    let restored: bool = unsafe { msg_send![&*pb, writeObjects: &*original] };
    assert!(restored || original.is_empty());
    assert!(text_result.is_ok() && image_result.is_ok());
    assert_eq!(pasted.as_deref(), Some("Jie 中文 😀"));
    assert!(png.is_some() && tiff.is_some());
}

fn click(window: &objc2_app_kit::NSWindow, x: f64, y: f64) {
    let view = window.contentView().unwrap();
    let location = view.convertPoint_toView(NSPoint::new(x, y), None);
    let screen = window.convertPointToScreen(location);
    let top = objc2_app_kit::NSScreen::screens(MainThreadMarker::new().unwrap()).firstObject().unwrap().frame().size.height;
    let original = NSEvent::mouseLocation();
    unsafe {
        CGWarpMouseCursorPosition(NSPoint::new(screen.x, top - screen.y));
    }
    for typ in [NSEventType::LeftMouseDown, NSEventType::LeftMouseUp] {
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            typ,
            location,
            NSEventModifierFlags::empty(),
            0.0,
            window.windowNumber(),
            None,
            0,
            1,
            1.0,
        )
        .unwrap();
        if typ == NSEventType::LeftMouseDown {
            view.mouseDown(&event);
        } else {
            view.mouseUp(&event);
        }
    }
    unsafe {
        CGWarpMouseCursorPosition(NSPoint::new(original.x, top - original.y));
    }
}

fn check_settings(settings: Win, window: &objc2_app_kit::NSWindow) {
    let view = window.contentView().unwrap();
    for (page, fields) in [(1, vec![1]), (2, vec![6, 7, 8]), (3, vec![3, 4, 5]), (0, vec![0])] {
        click(window, 50.0, 29.0 + 36.0 * page as f64);
        let visible: Vec<_> = view.subviews().iter().enumerate().filter(|(_, v)| !v.isHidden()).map(|(i, _)| i).collect();
        assert_eq!(visible, fields, "sidebar page {page}");
    }
    let switch = objc2_foundation::NSRect::new(NSPoint::new(260.0, 124.0), objc2_foundation::NSSize::new(40.0, 20.0));
    let snapshot = || {
        let rep = view.bitmapImageRepForCachingDisplayInRect(switch).unwrap();
        view.cacheDisplayInRect_toBitmapImageRep(switch, &rep);
        unsafe { std::slice::from_raw_parts(rep.bitmapData(), (rep.bytesPerRow() * rep.pixelsHigh()) as usize) }.to_vec()
    };
    let off = snapshot();
    click(window, 280.0, 134.0);
    assert_ne!(snapshot(), off, "autostart switch must change visibly");
    click(window, 280.0, 134.0);
    assert_eq!(snapshot(), off, "autostart switch must toggle back");
    for (x, expected) in [
        (272.0, "#9C44EC"),
        (302.0, "#E0389A"),
        (332.0, "#E5383B"),
        (362.0, "#F27A1A"),
        (392.0, "#F5B700"),
        (422.0, "#23A55A"),
        (452.0, "#FFFFFF"),
    ] {
        click(window, x, 172.0);
        assert_eq!(settings.field_text(2), expected, "color swatch");
    }
    click(window, 482.0, 172.0);
    assert!(!view.subviews().objectAtIndex(2).isHidden(), "custom color must show the hex field");
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWarpMouseCursorPosition(point: NSPoint) -> i32;
}

fn check_save_dialog(step: usize) {
    eprintln!("checking save dialog {step}");
    let mon = plat::monitor_at(Point::default());
    let owner = Win::open(Spec { kind: Kind::Overlay, rect: mon.rect, title: "save smoke", owner: None }, Probe::default()).unwrap();
    owner.show();
    let dir = plat::config_dir().unwrap();
    let jpeg = step == 2;
    let path = dir.join(if jpeg { "saved.jpg" } else { "saved.png" });
    let _ = std::fs::remove_file(&path);
    after(0.3, move || {
        let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
        let panel = app.modalWindow().expect("save panel must be modal");
        assert!(panel.downcast_ref::<objc2_app_kit::NSSavePanel>().is_some(), "must open the file picker, not an error alert");
        let overlay = app.windows().iter().find(|w| w.isVisible() && w.title().to_string() == "save smoke").unwrap();
        assert!(panel.isVisible());
        assert!(panel.level() > overlay.level(), "save panel must be above the capture overlay");
        assert!(!owner.enabled());
        if step == 0 {
            unsafe {
                let _: () = msg_send![&*panel, cancel: None::<&objc2::runtime::AnyObject>];
            }
        } else {
            // The system file-picker UI lives in an XPC service; exercise our OK response path here.
            app.stopModalWithCode(objc2_app_kit::NSModalResponseOK);
        }
    });
    let kind = if jpeg { plat::SaveAs::Jpeg } else { plat::SaveAs::Png };
    owner.ask_save(dir.to_string_lossy().into_owned(), "saved".into(), kind, move |p| {
        assert!(owner.enabled());
        let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
        let overlay = app.windows().iter().find(|w| w.isVisible() && w.title().to_string() == "save smoke").unwrap();
        assert_eq!(overlay.level(), objc2_app_kit::NSScreenSaverWindowLevel);
        owner.close();
        if step == 0 {
            assert!(p.is_none(), "cancel must preserve the screenshot");
            after(0.1, || check_save_dialog(1));
        } else {
            assert_eq!(p, Some(path.clone()));
            let img = plat::Image { w: 2, h: 2, px: vec![0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255] };
            image_io::save_async(img, path.clone(), mon);
            after(0.5, move || {
                let data = std::fs::read(&path).expect("save must write the image");
                assert!(if jpeg { data.starts_with(&[0xFF, 0xD8]) } else { data.starts_with(b"\x89PNG\r\n\x1a\n") });
                if step == 1 { check_save_dialog(2); }
                else {
                    eprintln!("mac smoke passed: settings navigation, toggles, colors, OK/cancel, PNG/JPEG save/cancel, IME, hotkeys, timers, owner cleanup");
                    plat::quit();
                }
            });
        }
    });
}

fn after(seconds: f64, f: impl Fn() + 'static) {
    let block = block2::RcBlock::new(move |_: *mut objc2::runtime::AnyObject| f());
    unsafe {
        let timer: objc2::rc::Retained<objc2::runtime::AnyObject> =
            msg_send![objc2::class!(NSTimer), timerWithTimeInterval: seconds, repeats: false, block: &*block];
        let run_loop = objc2_foundation::NSRunLoop::mainRunLoop();
        for mode in [objc2_foundation::NSDefaultRunLoopMode, objc2_app_kit::NSModalPanelRunLoopMode] {
            let _: () = msg_send![&*run_loop, addTimer: &*timer, forMode: mode];
        }
    }
}
