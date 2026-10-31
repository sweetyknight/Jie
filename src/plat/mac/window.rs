//! AppKit windows and the event bridge used by the platform-neutral handlers.

use super::context_canvas;
use crate::plat::{Button, Cursor, Edge, Event, FieldKind, Handler, Hit, Key, Kind, MenuItem, Mods, Monitor, Point, Rect, Spec};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject, Sel},
};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSBitmapImageRep, NSColor, NSControl, NSControlTextEditingDelegate, NSCursor, NSEvent, NSEventModifierFlags,
    NSEventType, NSGraphicsContext, NSImage, NSSecureTextField, NSTextField, NSTextFieldDelegate, NSTextInputClient, NSTextView, NSTrackingArea,
    NSTrackingAreaOptions, NSView, NSWindow, NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowDelegate, NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSNotFound, NSNotification, NSObjectProtocol, NSPoint, NSRange, NSRect, NSSize, NSString,
};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Default)]
struct ViewIvars {
    state: Cell<usize>,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    caret: Cell<Option<Rect>>,
    marked: RefCell<String>,
    selection: Cell<NSRange>,
}
#[derive(Default)]
struct WindowIvars {
    id: usize,
    focusable: bool,
}
struct FieldIvars {
    win: Cell<usize>,
    id: Cell<usize>,
}

define_class!(
    #[unsafe(super = objc2_foundation::NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = FieldIvars]
    struct FieldTarget;
    unsafe impl NSObjectProtocol for FieldTarget {}
    unsafe impl NSTextFieldDelegate for FieldTarget {}
    unsafe impl NSControlTextEditingDelegate for FieldTarget {
        #[unsafe(method(controlTextDidBeginEditing:))]
        fn began_editing(&self, _n: &NSNotification) { Win(self.ivars().win.get()).invalidate_all(); }
        #[unsafe(method(controlTextDidEndEditing:))]
        fn ended_editing(&self, _n: &NSNotification) { Win(self.ivars().win.get()).invalidate_all(); }

        #[unsafe(method(controlTextDidChange:))]
        fn text_changed(&self, _n: &NSNotification) {
            let w = Win(self.ivars().win.get()); let id = self.ivars().id.get();
            if let Some(s) = state_for(w) {
                let control = s.fields.borrow().get(&id).filter(|f| f.kind == FieldKind::Hex).map(|f| f.control.clone());
                if let Some(c) = control {
                    let value = c.stringValue().to_string();
                    if value.chars().count() > 7 { c.setStringValue(&NSString::from_str(&value.chars().take(7).collect::<String>())); }
                }
            }
            deliver(w, Event::FieldChanged(id));
        }
        #[unsafe(method(control:textView:doCommandBySelector:))]
        unsafe fn command(&self, _c: &NSControl, _v: &NSTextView, selector: Sel) -> bool {
            let key = if selector == Sel::register(c"insertTab:") || selector == Sel::register(c"insertBacktab:") { Some(Key::Tab) }
                else if selector == Sel::register(c"insertNewline:") { Some(Key::Enter) }
                else if selector == Sel::register(c"cancelOperation:") { Some(Key::Esc) } else { None };
            let shift = selector == Sel::register(c"insertBacktab:");
            if let Some(key) = key { deliver(Win(self.ivars().win.get()), Event::FieldKey {key,shift}); true } else { false }
        }
    }
    impl FieldTarget {
        #[unsafe(method(changed:))]
        fn changed(&self, _sender: Option<&AnyObject>) { deliver(Win(self.ivars().win.get()), Event::FieldChanged(self.ivars().id.get())); }
    }
);

define_class!(
    #[unsafe(super = NSTextField)]
    #[thread_kind = MainThreadOnly]
    #[ivars = FieldIvars]
    struct HotkeyField;
    impl HotkeyField {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts(&self) -> bool { true }
        #[unsafe(method(becomeFirstResponder))]
        fn become_first(&self) -> bool { true }
        #[unsafe(method(resignFirstResponder))]
        fn resign_first(&self) -> bool { true }
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _e: &NSEvent) { if let Some(w)=self.window() { w.makeFirstResponder(Some(self)); } }
        #[unsafe(method(keyDown:))]
        fn key_down(&self, e: &NSEvent) {
            let w=Win(self.ivars().win.get()); let id=self.ivars().id.get(); let k=key(e);
            if matches!(k,Key::Tab|Key::Enter|Key::Esc) { deliver(w,Event::FieldKey {key:k,shift:mods(e).shift}); }
            else if let Some(vk)=(1..256).find(|vk| super::app::carbon_key(*vk)==Some(e.keyCode() as u32)) {
                let flags=e.modifierFlags();
                let m = if flags.contains(NSEventModifierFlags::Control) {2} else {0}
                    | if flags.contains(NSEventModifierFlags::Option) {1} else {0}
                    | if flags.contains(NSEventModifierFlags::Shift) {4} else {0}
                    | if flags.contains(NSEventModifierFlags::Command) {8} else {0};
                w.set_hotkey_field(id,m,vk); deliver(w,Event::FieldChanged(id));
            }
        }
        #[unsafe(method(performKeyEquivalent:))]
        fn key_equivalent(&self, e: &NSEvent) -> bool {
            let focused=self.window().and_then(|w| w.firstResponder()).is_some_and(|r| Retained::as_ptr(&r).cast::<AnyObject>()==self as *const _ as *const AnyObject);
            if focused { self.keyDown(e); } focused
        }
    }
);

define_class!(
    #[unsafe(super = NSWindow)]
    #[thread_kind = MainThreadOnly]
    #[ivars = WindowIvars]
    struct JieWindow;
    unsafe impl NSObjectProtocol for JieWindow {}
    unsafe impl NSWindowDelegate for JieWindow {
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, _sender: &NSWindow) -> bool { deliver(Win(self.ivars().id), Event::Close); false }
        #[unsafe(method(windowDidBecomeKey:))]
        fn became_key(&self, _n: &NSNotification) { deliver(Win(self.ivars().id), Event::Focus(true)); }
        #[unsafe(method(windowDidResignKey:))]
        fn resigned_key(&self, _n: &NSNotification) {
            let w = Win(self.ivars().id);
            if let Some(s) = state_for(w) { s.resize.set(None); }
            deliver(w, Event::CaptureLost); deliver(w, Event::Focus(false));
            if w.enabled() { deliver(w, Event::Deactivate); }
        }
        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn backing_changed(&self, _n: &NSNotification) { let w = Win(self.ivars().id); deliver(w, Event::Dpi { scale:w.scale(),rect:w.rect() }); }
    }
    impl JieWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool { self.ivars().focusable }
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool { self.ivars().focusable }
    }
);

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ViewIvars]
    struct JieView;
    unsafe impl NSObjectProtocol for JieView {}
    unsafe impl NSTextInputClient for JieView {
        #[unsafe(method(insertText:replacementRange:))]
        unsafe fn insert(&self, string: &AnyObject, _range: NSRange) {
            let marked=!self.ivars().marked.borrow().is_empty();self.ivars().marked.borrow_mut().clear();
            if let Some(w)=win_for(self) {let s=input_string(string);
                if marked {deliver(w,Event::Ime {result:Some(s),comp:String::new(),cursor:0});}
                else {for c in s.chars().filter(|c|!c.is_control()) {deliver(w,Event::Char(c));}}
            }
        }
        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        unsafe fn set_marked(&self, string: &AnyObject, selection: NSRange, _range: NSRange) {
            let comp = input_string(string);
            self.ivars().marked.replace(comp.clone()); self.ivars().selection.set(selection);
            if let Some(w) = win_for(self) { deliver(w, Event::Ime {result:None,comp,cursor:selection.location}); }
        }
        #[unsafe(method(unmarkText))]
        fn unmark(&self) {
            self.ivars().marked.borrow_mut().clear();
            if let Some(w) = win_for(self) { deliver(w, Event::Ime {result:None,comp:String::new(),cursor:0}); }
        }
        #[unsafe(method(hasMarkedText))]
        fn has_marked(&self) -> bool { !self.ivars().marked.borrow().is_empty() }
        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange { let n = self.ivars().marked.borrow().encode_utf16().count(); NSRange::new(if n==0 {NSNotFound as usize} else {0},n) }
        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange { self.ivars().selection.get() }
        #[unsafe(method_id(validAttributesForMarkedText))]
        fn attributes(&self) -> Retained<NSArray<NSAttributedStringKey>> { NSArray::new() }
        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        unsafe fn substring(&self, _range: NSRange, actual: *mut NSRange) -> Option<Retained<NSAttributedString>> {
            if !actual.is_null() { unsafe { *actual=NSRange::new(NSNotFound as usize,0); } } None
        }
        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        unsafe fn first_rect(&self, range: NSRange, actual: *mut NSRange) -> NSRect {
            if !actual.is_null() { unsafe { *actual = range; } }
            let Some(w) = win_for(self) else { return NSRect::default() };
            let Some(n) = native(w) else { return NSRect::default() };
            let r = self.ivars().caret.get().unwrap_or_default(); let k=w.scale() as f64;
            let rect = NSRect::new(NSPoint::new(r.left as f64/k,r.top as f64/k),NSSize::new((r.right-r.left) as f64/k,(r.bottom-r.top) as f64/k));
            n.convertRectToScreen(self.convertRect_toView(rect,None))
        }
        #[unsafe(method(characterIndexForPoint:))]
        fn character_index(&self, _p: NSPoint) -> usize { NSNotFound as usize }
        #[unsafe(method(doCommandBySelector:))]
        unsafe fn command(&self, selector: Sel) {
            let name=selector.name().to_string_lossy();
            let key=match name.as_ref() {
                "insertNewline:"=>Key::Enter,"insertTab:"|"insertBacktab:"=>Key::Tab,"cancelOperation:"=>Key::Esc,
                "deleteBackward:"=>Key::Back,"deleteForward:"=>Key::Delete,
                "moveLeft:"|"moveLeftAndModifySelection:"=>Key::Left,"moveRight:"|"moveRightAndModifySelection:"=>Key::Right,
                "moveUp:"|"moveUpAndModifySelection:"=>Key::Up,"moveDown:"|"moveDownAndModifySelection:"=>Key::Down,
                "moveToBeginningOfLine:"|"moveToBeginningOfDocument:"=>Key::Home,"moveToEndOfLine:"|"moveToEndOfDocument:"=>Key::End,_=>Key::Other
            };
            if let Some(w)=win_for(self) {
                let mut m=NSApplication::sharedApplication(self.mtm()).currentEvent().filter(|e|e.r#type()==NSEventType::KeyDown).map(|e|mods(&e)).unwrap_or_default();
                m.shift |= name.contains("ModifySelection") || name=="insertBacktab:";
                deliver(w,Event::Key {key,mods:m});
            }
        }
    }
    impl JieView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool { true }
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool { true }
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) { draw_view(self, dirty); }
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) { mouse_event(self, e, Button::Left, true); }
        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, e: &NSEvent) { mouse_event(self, e, Button::Left, false); }
        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, e: &NSEvent) { mouse_event(self, e, Button::Right, true); }
        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, e: &NSEvent) { mouse_event(self, e, Button::Right, false); }
        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, e: &NSEvent) { move_event(self, e); }
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, e: &NSEvent) { move_event(self, e); }
        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, e: &NSEvent) { move_event(self, e); }
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, e: &NSEvent) { if let Some(w) = win_for(self) { let p = super::cursor_pos(); let raw=e.scrollingDeltaY(); let delta=if e.hasPreciseScrollingDeltas(){(raw*12.0).round() as i32}else{(raw*120.0).round() as i32}; deliver(w, Event::Wheel { delta, p, mods: mods(e) }); } }
        #[unsafe(method(keyDown:))]
        fn key_down(&self, e: &NSEvent) {
            let Some(w) = win_for(self) else {return}; let m=mods(e);
            if self.ivars().caret.get().is_some() && !m.cmd { self.interpretKeyEvents(&NSArray::from_slice(&[e])); }
            else { deliver(w, Event::Key { key:key(e),mods:m }); }
        }
        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking(&self) {
            if let Some(old) = self.ivars().tracking.take() { self.removeTrackingArea(&old); }
            let flags = NSTrackingAreaOptions::MouseEnteredAndExited | NSTrackingAreaOptions::MouseMoved | NSTrackingAreaOptions::ActiveAlways | NSTrackingAreaOptions::InVisibleRect;
            let area = unsafe { NSTrackingArea::initWithRect_options_owner_userInfo(NSTrackingArea::alloc(), self.bounds(), flags, Some(self), None) };
            self.addTrackingArea(&area); self.ivars().tracking.replace(Some(area));
        }
        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _e: &NSEvent) { if let Some(w) = win_for(self) { deliver(w, Event::Leave); } NSCursor::arrowCursor().set(); }

    }
);

struct Field {
    kind: FieldKind,
    hotkey: (u32, u32),
    shown: bool,
    control: Retained<NSTextField>,
    _target: Retained<FieldTarget>,
}
struct State {
    kind: Kind,
    owner: Option<Win>,
    modal: Cell<bool>,
    resize: Cell<Option<(Edge, Point, Rect)>>,
    control_click: Cell<bool>,
    cursor: RefCell<Option<(Cursor, Retained<NSCursor>)>>,
    handler: RefCell<Box<dyn Handler>>,
    alpha: Cell<f32>,
    timers: RefCell<HashMap<usize, u64>>,
    fields: RefCell<HashMap<usize, Field>>,
}
struct Entry {
    id: usize,
    win: Retained<NSWindow>,
    view: Retained<JieView>,
    state: Rc<State>,
}
thread_local! {
    static WINDOWS: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    static LATER: RefCell<VecDeque<Box<dyn FnOnce()>>> = const { RefCell::new(VecDeque::new()) };
    static PENDING: RefCell<VecDeque<(Win, Event)>> = const { RefCell::new(VecDeque::new()) };
    static SETTLING: Cell<bool> = const { Cell::new(false) };
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    static NEXT_ID: Cell<usize> = const { Cell::new(1) };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Win(pub(crate) usize);

fn frame_for(r: Rect) -> NSRect {
    super::cocoa_frame(r)
}
fn rect_from_frame(r: NSRect) -> Rect {
    super::physical_rect_for_cocoa(r)
}
fn state_for(w: Win) -> Option<Rc<State>> {
    WINDOWS.with(|v| v.borrow().iter().find(|e| e.id == w.0).map(|e| e.state.clone()))
}
fn native(w: Win) -> Option<Retained<NSWindow>> {
    WINDOWS.with(|v| v.borrow().iter().find(|e| e.id == w.0).map(|e| e.win.clone()))
}

fn view_for(w: Win) -> Option<Retained<JieView>> {
    WINDOWS.with(|v| v.borrow().iter().find(|e| e.id == w.0).map(|e| e.view.clone()))
}
fn win_for(v: &JieView) -> Option<Win> {
    let id = v.ivars().state.get();
    (id != 0).then_some(Win(id)).filter(|w| WINDOWS.with(|ws| ws.borrow().iter().any(|e| e.id == w.0)))
}
fn settle() {
    if DEPTH.with(|d| d.get()) > 0 || SETTLING.with(|s| s.replace(true)) {
        return;
    }
    loop {
        if let Some((w, e)) = PENDING.with_borrow_mut(|q| q.pop_front()) {
            deliver(w, e);
            continue;
        }
        if let Some(f) = LATER.with_borrow_mut(|q| q.pop_front()) {
            f();
            continue;
        }
        break;
    }
    SETTLING.with(|s| s.set(false));
}
fn call<R>(w: Win, f: impl FnOnce(&mut dyn Handler, Win) -> R) -> Option<R> {
    let st = state_for(w)?;
    let mut h = st.handler.try_borrow_mut().ok()?;
    DEPTH.with(|d| d.set(d.get() + 1));
    let result = f(&mut **h, w);
    DEPTH.with(|d| d.set(d.get() - 1));
    drop(h);
    settle();
    Some(result)
}
fn deliver(w: Win, e: Event) {
    if state_for(w).is_some_and(|s| s.handler.try_borrow().is_err()) {
        PENDING.with_borrow_mut(|q| q.push_back((w, e)));
        return;
    }
    call(w, |h, w| h.event(w, e));
    settle();
}
fn draw_view(v: &JieView, dirty: NSRect) {
    let Some(w) = win_for(v) else { return };
    let Some(gc) = NSGraphicsContext::currentContext() else {
        return;
    };
    let cg = gc.CGContext();
    let scale = WINDOWS.with(|ws| ws.borrow().iter().find(|e| e.id == w.0).and_then(|e| e.win.screen()).map(|s| s.backingScaleFactor()).unwrap_or(1.0));
    let r = Rect {
        left: (dirty.origin.x * scale).floor() as i32,
        top: (dirty.origin.y * scale).floor() as i32,
        right: ((dirty.origin.x + dirty.size.width) * scale).ceil() as i32,
        bottom: ((dirty.origin.y + dirty.size.height) * scale).ceil() as i32,
    };
    // AppKit gives the view a point-sized context; platform drawing is in backing pixels.
    objc2_core_graphics::CGContext::save_g_state(Some(&cg));
    objc2_core_graphics::CGContext::scale_ctm(Some(&cg), 1.0 / scale, 1.0 / scale);
    call(w, |h, w| {
        let canvas = context_canvas(&cg, r);
        if state_for(w).is_some_and(|s| s.kind == Kind::Pin) {
            canvas.clear(super::Color::default());
        }
        h.paint(w, &canvas);
    });
    objc2_core_graphics::CGContext::restore_g_state(Some(&cg));
}
fn event_point(e: &NSEvent) -> Point {
    let p = e.locationInWindow();
    let Some(mtm) = MainThreadMarker::new() else {
        return Point::default();
    };
    let win = e.window(mtm);
    let p = win.clone().and_then(|w| w.contentView()).map(|v| v.convertPoint_fromView(p, None)).unwrap_or(p);
    let scale = win.and_then(|w| w.screen()).map(|s| s.backingScaleFactor()).unwrap_or(1.0);
    Point { x: (p.x * scale).round() as i32, y: (p.y * scale).round() as i32 }
}
fn mods(e: &NSEvent) -> Mods {
    let f = e.modifierFlags();
    Mods { cmd: f.contains(NSEventModifierFlags::Command), shift: f.contains(NSEventModifierFlags::Shift), alt: f.contains(NSEventModifierFlags::Option) }
}
fn mouse_event(v: &JieView, e: &NSEvent, mut b: Button, down: bool) {
    let Some(w) = win_for(v) else { return };
    let Some(s) = state_for(w) else { return };
    let p = event_point(e);
    if b == Button::Left {
        if down {
            s.control_click.set(e.modifierFlags().contains(NSEventModifierFlags::Control));
        }
        if s.control_click.get() {
            b = Button::Right;
        }
    }
    if down && b == Button::Left {
        match call(w, |h, w| h.hit(w, p)).unwrap_or(Hit::Client) {
            Hit::Caption => {
                if let Some(n) = native(w) {
                    n.performWindowDragWithEvent(e);
                }
                return;
            }
            Hit::Edge(edge) => {
                s.resize.set(Some((edge, super::cursor_pos(), w.rect())));
                return;
            }
            Hit::Client => {}
        }
    }
    if down {
        deliver(w, Event::Down { p, button: b, double: e.clickCount() > 1, mods: mods(e) });
    } else {
        s.resize.set(None);
        deliver(w, Event::Up { button: b });
    }
}
#[allow(deprecated)] // These system resize cursors also support macOS 14.
fn move_event(v: &JieView, e: &NSEvent) {
    let Some(w) = win_for(v) else { return };
    let Some(s) = state_for(w) else { return };
    if let Some((edge, down, mut r)) = s.resize.get() {
        let now = super::cursor_pos();
        let (dx, dy) = (now.x - down.x, now.y - down.y);
        let (l, rr, t, b) = edge.sides();
        if l {
            r.left += dx;
        }
        if rr {
            r.right += dx;
        }
        if t {
            r.top += dy;
        }
        if b {
            r.bottom += dy;
        }
        if let Some(r) = call(w, |h, w| h.sizing(w, edge, r)) {
            w.set_rect(r);
            w.invalidate_all();
        }
        return;
    }
    let p = event_point(e);
    deliver(w, Event::Move { p, mods: mods(e) });
    let cursor = call(w, |h, w| h.cursor(w, p)).unwrap_or(Cursor::Arrow);
    if s.cursor.borrow().as_ref().is_some_and(|(c, _)| *c == cursor) {
        s.cursor.borrow().as_ref().unwrap().1.set();
        return;
    }
    let c = match cursor {
        Cursor::Arrow => NSCursor::arrowCursor(),
        Cursor::Cross => NSCursor::crosshairCursor(),
        Cursor::IBeam => NSCursor::IBeamCursor(),
        Cursor::Move => NSCursor::openHandCursor(),
        Cursor::SizeNS => NSCursor::resizeUpDownCursor(),
        Cursor::SizeWE => NSCursor::resizeLeftRightCursor(),
        Cursor::SizeNWSE | Cursor::SizeNESW => {
            let Some(b) = super::Bitmap::new(48, 48) else { return };
            {
                let c = super::Canvas::new(&b, b.full(), true);
                c.clear(super::Color::default());
                for (width, color) in [(6.0, 0xFFFFFF), (3.0, 0)] {
                    for (a, z) in [
                        ((9.0, 9.0), (39.0, 39.0)),
                        ((9.0, 22.0), (9.0, 9.0)),
                        ((9.0, 9.0), (22.0, 9.0)),
                        ((26.0, 39.0), (39.0, 39.0)),
                        ((39.0, 39.0), (39.0, 26.0)),
                    ] {
                        let map = |(x, y)| if cursor == Cursor::SizeNESW { (48.0 - x, y) } else { (x, y) };
                        c.line(
                            map(a),
                            map(z),
                            super::Color {
                                r: if color == 0 { 0.0 } else { 1.0 },
                                g: if color == 0 { 0.0 } else { 1.0 },
                                b: if color == 0 { 0.0 } else { 1.0 },
                                a: 1.0,
                            },
                            width,
                            super::Stroke::Round,
                        );
                    }
                }
            }
            let Some(cursor) = image_cursor(&b, 2.0) else { return };
            cursor
        }
        Cursor::Ring(d, scale) => {
            let (n, px) = crate::plat::ring_pixels(d, scale);
            let Some(mut b) = super::Bitmap::new(n, n) else { return };
            for (dst, p) in b.pixels_mut().chunks_exact_mut(4).zip(px) {
                dst.copy_from_slice(&p.to_le_bytes());
            }
            let Some(cursor) = image_cursor(&b, scale) else { return };
            cursor
        }
    };
    c.set();
    s.cursor.replace(Some((cursor, c)));
}

fn key(e: &NSEvent) -> Key {
    match e.keyCode() {
        0x35 => Key::Esc,
        0x24 => Key::Enter,
        0x30 => Key::Tab,
        0x31 => Key::Space,
        0x33 => Key::Back,
        0x75 => Key::Delete,
        0x7B => Key::Left,
        0x7C => Key::Right,
        0x7E => Key::Up,
        0x7D => Key::Down,
        0x73 => Key::Home,
        0x77 => Key::End,
        _ => e
            .charactersIgnoringModifiers()
            .and_then(|s| s.to_string().bytes().next())
            .map_or(Key::Other, |c| if c.is_ascii_alphanumeric() { Key::Char(c.to_ascii_uppercase()) } else { Key::Other }),
    }
}

impl Win {
    pub fn open(spec: Spec, h: impl Handler) -> Option<Win> {
        let mtm = MainThreadMarker::new()?;
        let style = match spec.kind {
            Kind::Settings => NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            _ => NSWindowStyleMask::Borderless,
        };
        let id = NEXT_ID.with(|n| {
            let v = n.get();
            n.set(v.wrapping_add(1).max(1));
            v
        });
        let base = JieWindow::alloc(mtm).set_ivars(WindowIvars { id, focusable: !matches!(spec.kind, Kind::Toast | Kind::PinBar) });
        let custom: Retained<JieWindow> =
            unsafe { msg_send![super(base), initWithContentRect: frame_for(spec.rect), styleMask: style, backing: NSBackingStoreType::Buffered, defer: false] };
        custom.setDelegate(Some(ProtocolObject::from_ref(&*custom)));
        let w: Retained<NSWindow> = custom.into_super();
        unsafe { w.setReleasedWhenClosed(false) };
        w.setTitle(&NSString::from_str(spec.title));
        let state = Rc::new(State {
            kind: spec.kind,
            owner: spec.owner,
            modal: Cell::new(false),
            resize: Cell::new(None),
            control_click: Cell::new(false),
            cursor: RefCell::new(None),
            handler: RefCell::new(Box::new(h)),
            alpha: Cell::new(1.0),
            timers: RefCell::new(HashMap::new()),
            fields: RefCell::new(HashMap::new()),
        });
        let wf = frame_for(spec.rect);
        let view: Retained<JieView> = unsafe {
            let a = JieView::alloc(mtm).set_ivars(ViewIvars { state: Cell::new(id), ..Default::default() });
            msg_send![super(a), initWithFrame: NSRect::new(NSPoint::new(0.,0.), wf.size)]
        };
        w.setContentView(Some(&view));
        w.makeFirstResponder(Some(&*view));
        w.setHasShadow(matches!(spec.kind, Kind::Pin | Kind::Card));
        w.setLevel(match spec.kind {
            Kind::Overlay => objc2_app_kit::NSScreenSaverWindowLevel,
            Kind::Settings => objc2_app_kit::NSNormalWindowLevel,
            _ => objc2_app_kit::NSFloatingWindowLevel,
        });
        w.setAnimationBehavior(NSWindowAnimationBehavior::None);
        w.setAcceptsMouseMovedEvents(true);
        if spec.kind != Kind::Settings {
            w.setCollectionBehavior(NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::FullScreenAuxiliary);
        }
        // AppKit rounds frames to whole points; odd-sized Retina pins leave spare backing pixels.
        w.setOpaque(!matches!(spec.kind, Kind::Pin | Kind::PinBar | Kind::Toast));
        if matches!(spec.kind, Kind::Pin | Kind::PinBar | Kind::Toast) {
            w.setBackgroundColor(Some(&NSColor::clearColor()));
        }
        if spec.kind == Kind::Toast {
            w.setIgnoresMouseEvents(true);
        }
        if let Some(owner) = spec.owner {
            if let Some(parent) = WINDOWS.with(|v| v.borrow().iter().find(|e| e.id == owner.0).map(|e| e.win.clone())) {
                unsafe {
                    parent.addChildWindow_ordered(&w, NSWindowOrderingMode::Above);
                }
            }
        }
        let result = Win(id);
        WINDOWS.with_borrow_mut(|v| v.push(Entry { id, win: w, view, state }));
        Some(result)
    }
    pub fn show(self) {
        if let Some(w) = WINDOWS.with(|v| v.borrow().iter().find(|e| e.id == self.0).map(|e| e.win.clone())) {
            if state_for(self).is_some_and(|s| matches!(s.kind, Kind::PinBar | Kind::Toast)) {
                w.orderFrontRegardless();
            } else {
                w.makeKeyAndOrderFront(None);
                NSApplication::sharedApplication(w.mtm()).activate();
            }
        }
    }
    pub fn hide(self) {
        if let Some(w) = WINDOWS.with(|v| v.borrow().iter().find(|e| e.id == self.0).map(|e| e.win.clone())) {
            w.orderOut(None);
        }
    }
    pub fn close(self) {
        Self::later(move || {
            let children: Vec<_> = WINDOWS.with(|v| v.borrow().iter().filter(|e| e.state.owner == Some(self)).map(|e| Win(e.id)).collect());
            for child in children {
                child.close();
            }
            let entry = WINDOWS.with_borrow_mut(|v| v.iter().position(|e| e.id == self.0).map(|i| v.remove(i)));
            if let Some(entry) = entry {
                entry.view.ivars().state.set(0);
                entry.win.setDelegate(None);
                entry.win.close();
            }
        });
    }
    pub fn all(kind: Kind) -> Vec<Win> {
        WINDOWS.with(|v| v.borrow().iter().filter(|e| e.state.kind == kind).map(|e| Win(e.id)).collect())
    }
    pub fn rect(self) -> Rect {
        WINDOWS.with(|v| v.borrow().iter().find(|e| e.id == self.0).map(|e| rect_from_frame(e.win.frame())).unwrap_or_default())
    }
    pub fn set_rect(self, r: Rect) {
        if let Some(w) = WINDOWS.with(|v| v.borrow().iter().find(|e| e.id == self.0).map(|e| e.win.clone())) {
            w.setFrame_display(frame_for(r), true);
        }
    }
    pub fn move_to(self, x: i32, y: i32) {
        let r = self.rect();
        self.set_rect(Rect { left: x, top: y, right: x + r.right - r.left, bottom: y + r.bottom - r.top });
    }
    pub fn invalidate(self, r: Rect) {
        if let Some(v) = view_for(self) {
            if state_for(self).is_some_and(|s| matches!(s.kind, Kind::Toast | Kind::PinBar)) {
                v.setNeedsDisplay(true);
            } else {
                let k = self.scale() as f64;
                v.setNeedsDisplayInRect(NSRect::new(
                    NSPoint::new(r.left as f64 / k, r.top as f64 / k),
                    NSSize::new((r.right - r.left) as f64 / k, (r.bottom - r.top) as f64 / k),
                ));
            }
        }
    }
    pub fn invalidate_all(self) {
        if let Some(v) = view_for(self) {
            v.setNeedsDisplay(true);
        }
    }

    pub fn set_alpha(self, a: f32) {
        if let Some(w) = WINDOWS.with(|v| v.borrow().iter().find(|e| e.id == self.0).map(|e| e.win.clone())) {
            w.setAlphaValue(a as f64);
            if let Some(st) = state_for(self) {
                st.alpha.set(a);
            }
        }
    }
    pub fn timer(self, id: usize, ms: u32) {
        let generation = state_for(self)
            .map(|s| {
                let mut t = s.timers.borrow_mut();
                let g = t.get(&id).copied().unwrap_or(0).wrapping_add(1);
                t.insert(id, g);
                g
            })
            .unwrap_or(0);
        let w = self;
        let delay = ms.max(1);
        let _ = dispatch2::DispatchQueue::main().after(dispatch2::DispatchTime::NOW.time(delay as i64 * 1_000_000), move || {
            if state_for(w).is_some_and(|s| s.timers.borrow().get(&id) == Some(&generation)) {
                deliver(w, Event::Timer(id));
                if let Some(s) = state_for(w) {
                    let next = s.timers.borrow().get(&id).copied();
                    if next == Some(generation) {
                        w.timer(id, delay);
                    }
                }
            }
        });
    }
    pub fn kill_timer(self, id: usize) {
        if let Some(s) = state_for(self) {
            let mut t = s.timers.borrow_mut();
            let g = t.get(&id).copied().unwrap_or(0).wrapping_add(1);
            t.insert(id, g);
        }
    }
    pub fn cursor_pos(self) -> Point {
        let Some(w) = native(self) else { return Point::default() };
        let Some(v) = w.contentView() else { return Point::default() };
        // Use the same content coordinates as mouseDown, excluding the title bar.
        let p = v.convertPoint_fromView(w.mouseLocationOutsideOfEventStream(), None);
        let scale = w.backingScaleFactor();
        Point { x: (p.x * scale).round() as i32, y: (p.y * scale).round() as i32 }
    }
    pub fn scale(self) -> f32 {
        WINDOWS.with(|v| v.borrow().iter().find(|e| e.id == self.0).and_then(|e| e.win.screen()).map(|s| s.backingScaleFactor() as f32).unwrap_or(1.0))
    }
    pub fn monitor(self) -> Monitor {
        super::monitor_at(self.rect().into())
    }
    pub fn foreground(self) {
        self.show();
    }
    pub fn enabled(self) -> bool {
        state_for(self).is_some_and(|s| !s.modal.get())
    }
    pub fn ime(self, caret: Option<Rect>) {
        if let Some(v) = view_for(self) {
            let old = v.ivars().caret.replace(caret);
            if let Some(context) = v.inputContext() {
                if old.is_some() && caret.is_none() {
                    v.ivars().marked.borrow_mut().clear();
                    context.discardMarkedText();
                }
                context.invalidateCharacterCoordinates();
            }
        }
    }
    pub fn later(f: impl FnOnce() + 'static) {
        if DEPTH.with(|d| d.get()) > 0 { LATER.with_borrow_mut(|q| q.push_back(Box::new(f))) } else { f() }
    }
    pub fn menu(self, items: &[MenuItem], at: Point, then: impl FnOnce(usize) + 'static) {
        let own: Vec<_> = items.iter().map(|i| (i.id, i.text.to_owned(), i.key.to_owned(), i.enabled, i.default, i.checked)).collect();
        let Some(w) = native(self) else { return };
        let Some(v) = w.contentView() else { return };
        let scale = w.backingScaleFactor();
        let p = w.convertPointToScreen(v.convertPoint_toView(NSPoint::new(at.x as f64 / scale, at.y as f64 / scale), None));
        let r = rect_from_frame(NSRect::new(p, NSSize::new(0.0, 0.0)));
        let at = Point { x: r.left, y: r.top };
        Self::later(move || {
            let items: Vec<_> = own
                .iter()
                .map(|(id, text, key, enabled, default, checked)| MenuItem { id: *id, text, key, enabled: *enabled, default: *default, checked: *checked })
                .collect();
            then(super::tray_menu(&items, at));
        });
    }
    pub(crate) fn dialog<R>(self, f: impl FnOnce() -> R) -> R {
        let state = state_for(self);
        if let Some(s) = &state {
            s.modal.set(true);
        }
        let window = native(self);
        let level = window.as_ref().map(|w| w.level()).unwrap_or(0);
        // NSSavePanel resets its level in runModal; keep the capture overlay below it.
        if let Some(w) = &window {
            w.setLevel(objc2_app_kit::NSNormalWindowLevel);
        }
        let result = f();
        if let Some(w) = window {
            w.setLevel(level);
            w.makeKeyAndOrderFront(None);
        }
        if let Some(s) = state {
            s.modal.set(false);
        }
        result
    }
    pub fn ask_save(self, dir: String, name: String, jpeg: bool, then: impl FnOnce(Option<PathBuf>) + 'static) {
        Self::later(move || then(super::ask_save_path(self, &dir, &name, jpeg)));
    }
    pub fn ask_folder(self, start: String, then: impl FnOnce(Option<String>) + 'static) {
        Self::later(move || then(super::ask_folder(self, &start)));
    }
    pub fn post<H: Handler>(self, f: impl FnOnce(&mut H, Win) + Send + 'static) {
        super::post(move || {
            let _ = self.with(|h| f(h, self));
        });
    }
    pub fn with<H: Handler, R>(self, f: impl FnOnce(&mut H) -> R) -> Option<R> {
        call(self, |h, _| (h as &mut dyn Any).downcast_mut::<H>().map(f)).flatten()
    }

    pub fn field(self, id: usize, kind: FieldKind, cue: &str) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(view) = view_for(self) else { return };
        let partial = FieldTarget::alloc(mtm).set_ivars(FieldIvars { win: Cell::new(self.0), id: Cell::new(id) });
        let target: Retained<FieldTarget> = unsafe { msg_send![super(partial), init] };
        let control: Retained<NSTextField> = if kind == FieldKind::Password {
            let secure: Retained<NSSecureTextField> =
                unsafe { msg_send![NSSecureTextField::alloc(mtm), initWithFrame: NSRect::new(NSPoint::new(0.,0.), NSSize::new(1.,1.))] };
            secure.into_super()
        } else if kind == FieldKind::Hotkey {
            let partial = HotkeyField::alloc(mtm).set_ivars(FieldIvars { win: Cell::new(self.0), id: Cell::new(id) });
            let c: Retained<HotkeyField> = unsafe { msg_send![super(partial), initWithFrame:NSRect::default()] };
            c.into_super()
        } else {
            unsafe { msg_send![NSTextField::alloc(mtm), initWithFrame: NSRect::new(NSPoint::new(0.,0.), NSSize::new(1.,1.))] }
        };
        let placeholder = (!cue.is_empty()).then(|| NSString::from_str(cue));
        control.setPlaceholderString(placeholder.as_ref().map(|v| &**v));
        control.setBordered(false);
        control.setBezeled(false);
        control.setDrawsBackground(false);
        control.setEditable(!matches!(kind, FieldKind::ReadOnly | FieldKind::Hotkey));
        control.setSelectable(kind != FieldKind::Hotkey);
        unsafe {
            control.setTarget(Some(&*target as &AnyObject));
            control.setAction(Some(Sel::register(c"changed:")));
        }
        unsafe {
            control.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        }
        control.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(13.0)));
        control.setHidden(true);
        view.addSubview(&control);
        if let Some(s) = state_for(self) {
            s.fields.borrow_mut().insert(id, Field { kind, hotkey: (0, 0), shown: false, control, _target: target });
        }
    }
    pub fn place_field(self, id: usize, frame: Rect, scale: f32) {
        if let Some(s) = state_for(self) {
            if let Some(f) = s.fields.borrow().get(&id) {
                let k = scale.max(1.0) as f64;
                f.control.setFrame(NSRect::new(
                    NSPoint::new(frame.left as f64 / k + 7.0, frame.top as f64 / k + ((frame.bottom - frame.top) as f64 / k - 18.0) / 2.0),
                    NSSize::new(((frame.right - frame.left) as f64 / k - 14.0).max(1.0), 18.0),
                ));
            }
        }
    }
    pub fn show_field(self, id: usize, on: bool) {
        if let Some(s) = state_for(self) {
            if let Some(f) = s.fields.borrow_mut().get_mut(&id) {
                f.shown = on;
                f.control.setHidden(!on);
            }
        }
    }
    pub fn field_text(self, id: usize) -> String {
        state_for(self).and_then(|s| s.fields.borrow().get(&id).map(|v| v.control.stringValue().to_string())).unwrap_or_default()
    }
    pub fn set_field_text(self, id: usize, t: &str) {
        if let Some(s) = state_for(self) {
            if let Some(v) = s.fields.borrow().get(&id) {
                v.control.setStringValue(&NSString::from_str(t));
            }
        }
    }
    pub fn hotkey_field(self, id: usize) -> (u32, u32) {
        state_for(self).and_then(|s| s.fields.borrow().get(&id).map(|v| v.hotkey)).unwrap_or_default()
    }
    pub fn set_hotkey_field(self, id: usize, m: u32, vk: u32) {
        if let Some(s) = state_for(self) {
            if let Some(v) = s.fields.borrow_mut().get_mut(&id) {
                v.hotkey = (m, vk);
                v.control.setStringValue(&NSString::from_str(&crate::settings::Hotkey { mods: m, vk }.name("")));
            }
        }
    }
    pub fn focus_field(self, id: Option<usize>) {
        let Some(w) = WINDOWS.with(|v| v.borrow().iter().find(|e| e.id == self.0).map(|e| e.win.clone())) else {
            return;
        };
        if let Some(id) = id {
            if let Some(s) = state_for(self) {
                if let Some(f) = s.fields.borrow().get(&id) {
                    w.makeFirstResponder(Some(&*f.control));
                }
            }
        } else {
            if let Some(v) = view_for(self) {
                w.makeFirstResponder(Some(&*v));
            }
        }
    }
    pub fn focused_field(self) -> Option<usize> {
        let w = WINDOWS.with(|v| v.borrow().iter().find(|e| e.id == self.0).map(|e| e.win.clone()))?;
        let first = w.firstResponder()?;
        let p = Retained::as_ptr(&first) as usize;
        state_for(self)?
            .fields
            .borrow()
            .iter()
            .find(|(_, f)| {
                Retained::as_ptr(&f.control) as usize == p || f.control.currentEditor().is_some_and(|editor| Retained::as_ptr(&editor) as usize == p)
            })
            .map(|(id, _)| *id)
    }
    pub fn focused(self) -> bool {
        WINDOWS.with(|v| v.borrow().iter().find(|e| e.id == self.0).is_some_and(|e| e.win.isKeyWindow()))
    }
}

#[cfg(test)]
impl Win {
    pub fn alive(self) -> bool {
        WINDOWS.with(|v| v.borrow().iter().any(|e| e.id == self.0))
    }
    pub fn send(self, e: Event) {
        deliver(self, e);
    }
    pub fn repaint(self) {
        if let Some(v) = view_for(self) {
            v.display();
        }
    }
    pub fn peek<'a, H: Handler>(self) -> &'a mut H {
        let ptr = WINDOWS.with(|v| v.borrow().iter().find(|e| e.id == self.0).map(|e| &*e.state as *const State)).expect("窗口没了");
        let st = unsafe { &*ptr };
        let h = unsafe { &mut **st.handler.as_ptr() };
        (h as &mut dyn Any).downcast_mut::<H>().expect("不是这种窗口")
    }
}

#[cfg(test)]
pub fn pump() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let mode = unsafe { objc2_foundation::NSDefaultRunLoopMode };
    let Some(e) =
        app.nextEventMatchingMask_untilDate_inMode_dequeue(objc2_app_kit::NSEventMask::Any, Some(&objc2_foundation::NSDate::distantPast()), mode, true)
    else {
        return false;
    };
    app.sendEvent(&e);
    true
}

impl From<Rect> for Point {
    fn from(r: Rect) -> Point {
        Point { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 }
    }
}

fn input_string(s: &AnyObject) -> String {
    if let Some(s) = s.downcast_ref::<NSString>() {
        s.to_string()
    } else if let Some(s) = s.downcast_ref::<NSAttributedString>() {
        s.string().to_string()
    } else {
        String::new()
    }
}

fn image_cursor(bitmap: &super::Bitmap, scale: f32) -> Option<Retained<NSCursor>> {
    let cg = bitmap.image()?;
    let size = bitmap.w as f64 / scale as f64;
    let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(size, size));
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
    image.addRepresentation(&rep);
    Some(NSCursor::initWithImage_hotSpot(NSCursor::alloc(), &image, NSPoint::new(size / 2.0, size / 2.0)))
}
