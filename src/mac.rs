use crate::geometry::{cocoa_to_ax, Rect};
use cocoa::base::{id, nil};
use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::{c_char, c_void, CStr};
use std::ptr::null;

pub type Ref = *const c_void;
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> Ref;
    fn AXUIElementCopyAttributeValue(element: Ref, attr: Ref, value: *mut Ref) -> i32;
    fn AXUIElementSetAttributeValue(element: Ref, attr: Ref, value: Ref) -> i32;
    fn AXUIElementPerformAction(element: Ref, action: Ref) -> i32;
    fn AXUIElementSetMessagingTimeout(element: Ref, timeout: f32) -> i32;
    fn AXValueCreate(kind: u32, value: *const c_void) -> Ref;
    fn AXValueGetValue(value: Ref, kind: u32, output: *mut c_void) -> bool;
    pub fn CGEventTapCreate(
        location: u32,
        placement: u32,
        options: u32,
        mask: u64,
        callback: extern "C" fn(Ref, u32, Ref, *mut c_void) -> Ref,
        user: *mut c_void,
    ) -> Ref;
    pub fn CGEventTapEnable(tap: Ref, enabled: bool);
    pub fn CGEventGetIntegerValueField(event: Ref, field: u32) -> i64;
    pub fn CGEventGetFlags(event: Ref) -> u64;
    pub fn CGEventGetLocation(event: Ref) -> NSPoint;
    fn CGWindowListCopyWindowInfo(options: u32, relative: u32) -> Ref;
    pub fn CGPreflightListenEventAccess() -> bool;
    pub fn CGRequestListenEventAccess() -> bool;
    pub fn CGPreflightScreenCaptureAccess() -> bool;
    pub fn CGRequestScreenCaptureAccess() -> bool;
    pub fn CGImageGetWidth(image: Ref) -> usize;
    pub fn CGImageGetHeight(image: Ref) -> usize;
    pub static kCGWindowOwnerPID: Ref;
    pub static kCGWindowBounds: Ref;
    pub static kCGWindowLayer: Ref;
    pub static kCGWindowNumber: Ref;
    pub static kCGWindowName: Ref;
    fn CGRectMakeWithDictionaryRepresentation(dictionary: Ref, rect: *mut NSRect) -> bool;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub fn CFRetain(value: Ref) -> Ref;
    pub fn CFRelease(value: Ref);
    pub fn CFEqual(a: Ref, b: Ref) -> bool;
    pub fn CFArrayGetCount(array: Ref) -> isize;
    pub fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
    fn CFDictionaryGetValue(dictionary: Ref, key: Ref) -> Ref;
    fn CFNumberGetValue(number: Ref, kind: u32, output: *mut c_void) -> bool;
    pub fn CFMachPortCreateRunLoopSource(allocator: Ref, port: Ref, order: isize) -> Ref;
    pub fn CFRunLoopGetCurrent() -> Ref;
    pub fn CFRunLoopAddSource(runloop: Ref, source: Ref, mode: Ref);
    pub fn CFRunLoopRun();
    pub static kCFRunLoopCommonModes: Ref;
    pub static kCFBooleanFalse: Ref;
    pub static kCFBooleanTrue: Ref;
}

pub struct Owned(pub Ref);
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.0);
        }
    }
}
impl Clone for Owned {
    fn clone(&self) -> Self {
        unsafe { Self(CFRetain(self.0)) }
    }
}
impl PartialEq for Owned {
    fn eq(&self, other: &Self) -> bool {
        unsafe { CFEqual(self.0, other.0) }
    }
}

pub unsafe fn ns(s: &str) -> id {
    NSString::alloc(nil).init_str(s).autorelease()
}
pub unsafe fn string(s: Ref) -> String {
    if s.is_null() {
        return String::new();
    }
    let bytes: *const c_char = msg_send![s as id, UTF8String];
    if bytes.is_null() {
        String::new()
    } else {
        CStr::from_ptr(bytes).to_string_lossy().into_owned()
    }
}
pub unsafe fn attr(element: Ref, name: &str) -> Option<Owned> {
    let mut value = null();
    if AXUIElementCopyAttributeValue(element, ns(name) as Ref, &mut value) == 0 && !value.is_null()
    {
        Some(Owned(value))
    } else {
        None
    }
}
pub unsafe fn set(element: Ref, name: &str, value: Ref) -> Result<(), String> {
    let error = AXUIElementSetAttributeValue(element, ns(name) as Ref, value);
    if error == 0 {
        Ok(())
    } else {
        Err(format!("{name}: Accessibility error {error}"))
    }
}
pub unsafe fn frame(element: Ref) -> Option<Rect> {
    let p = attr(element, "AXPosition")?;
    let s = attr(element, "AXSize")?;
    let mut position = NSPoint::new(0., 0.);
    let mut size = NSSize::new(0., 0.);
    if !AXValueGetValue(p.0, 1, &mut position as *mut _ as *mut c_void)
        || !AXValueGetValue(s.0, 2, &mut size as *mut _ as *mut c_void)
    {
        return None;
    }
    Some(Rect {
        x: position.x,
        y: position.y,
        w: size.width,
        h: size.height,
    })
}

#[derive(Clone)]
pub struct Window {
    pub ax: Owned,
    pub pid: i32,
    pub app: String,
    pub title: String,
    pub minimized: bool,
    pub number: u32,
}

unsafe fn app_ax(pid: i32) -> Owned {
    let ax = Owned(AXUIElementCreateApplication(pid));
    AXUIElementSetMessagingTimeout(ax.0, 0.08);
    ax
}

pub unsafe fn focused() -> Option<Owned> {
    attr(app_ax(frontmost_pid()?).0, "AXFocusedWindow")
}

pub unsafe fn frontmost_pid() -> Option<i32> {
    let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
    let app: id = msg_send![workspace, frontmostApplication];
    if app == nil {
        return None;
    }
    let pid: i32 = msg_send![app, processIdentifier];
    Some(pid)
}

struct WindowInfo {
    pid: i32,
    rect: Rect,
    number: u32,
    title: String,
}
fn match_window(pid: i32, rect: Rect, title: &str, infos: &[WindowInfo]) -> Option<usize> {
    let candidates: Vec<_> = infos
        .iter()
        .enumerate()
        .filter(|(_, i)| {
            i.pid == pid
                && (i.rect.x - rect.x).abs() < 3.
                && (i.rect.y - rect.y).abs() < 3.
                && (i.rect.w - rect.w).abs() < 3.
                && (i.rect.h - rect.h).abs() < 3.
        })
        .map(|(index, _)| index)
        .collect();
    if !title.is_empty() {
        if let Some(index) = candidates.iter().find(|i| infos[**i].title == title) {
            return Some(*index);
        }
    }
    if candidates.len() == 1 {
        Some(candidates[0])
    } else {
        None
    }
}

pub unsafe fn windows() -> Vec<Window> {
    windows_for(None)
}

pub unsafe fn current_app_windows() -> Vec<Window> {
    frontmost_pid()
        .map(|pid| windows_for(Some(pid)))
        .unwrap_or_default()
}

unsafe fn windows_for(scope: Option<i32>) -> Vec<Window> {
    // shortcut: public AX only; full enumeration across Spaces would require private macOS APIs.
    let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
    let apps: id = msg_send![workspace, runningApplications];
    let count: usize = msg_send![apps, count];
    let mut windows = Vec::new();
    for i in 0..count {
        let app: id = msg_send![apps, objectAtIndex:i];
        let policy: isize = msg_send![app, activationPolicy];
        let pid: i32 = msg_send![app, processIdentifier];
        if policy != 0
            || pid == std::process::id() as i32
            || scope.is_some_and(|target| pid != target)
        {
            continue;
        }
        let name: id = msg_send![app, localizedName];
        let ax = app_ax(pid);
        let mut elements = Vec::new();
        if let Some(list) = attr(ax.0, "AXWindows") {
            for j in 0..CFArrayGetCount(list.0) {
                elements.push(Owned(CFRetain(CFArrayGetValueAtIndex(list.0, j))));
            }
        }
        // Some apps omit the focused/main window from AXWindows on another Space.
        for key in ["AXFocusedWindow", "AXMainWindow"] {
            if let Some(w) = attr(ax.0, key) {
                if !elements.contains(&w) {
                    elements.push(w);
                }
            }
        }
        for element in elements {
            AXUIElementSetMessagingTimeout(element.0, 0.08);
            if attr(element.0, "AXRole").map(|v| string(v.0)).as_deref() != Some("AXWindow") {
                continue;
            }
            let Some(rect) = frame(element.0) else {
                continue;
            };
            if rect.w < 80. || rect.h < 40. {
                continue;
            }
            let title = attr(element.0, "AXTitle")
                .map(|v| string(v.0))
                .unwrap_or_default();
            let minimized =
                attr(element.0, "AXMinimized").is_some_and(|v| CFEqual(v.0, kCFBooleanTrue));
            windows.push(Window {
                ax: element,
                pid,
                app: string(name as Ref),
                title,
                minimized,
                number: 0,
            });
        }
    }
    // Use public WindowServer bounds to seed front-to-back ordering without private window IDs.
    if let Some(list) = std::ptr::NonNull::new(CGWindowListCopyWindowInfo(0, 0) as *mut c_void) {
        let list = Owned(list.as_ptr());
        let mut ranks = Vec::new();
        for i in 0..CFArrayGetCount(list.0) {
            let info = CFArrayGetValueAtIndex(list.0, i);
            let mut pid = 0i32;
            let mut layer = 0i32;
            let p = CFDictionaryGetValue(info, kCGWindowOwnerPID);
            let l = CFDictionaryGetValue(info, kCGWindowLayer);
            if p.is_null() || l.is_null() {
                continue;
            }
            CFNumberGetValue(p, 3, &mut pid as *mut _ as *mut c_void);
            CFNumberGetValue(l, 3, &mut layer as *mut _ as *mut c_void);
            let mut r = NSRect::new(NSPoint::new(0., 0.), NSSize::new(0., 0.));
            if layer == 0
                && CGRectMakeWithDictionaryRepresentation(
                    CFDictionaryGetValue(info, kCGWindowBounds),
                    &mut r,
                )
            {
                let mut number = 0i32;
                let n = CFDictionaryGetValue(info, kCGWindowNumber);
                if !n.is_null() {
                    CFNumberGetValue(n, 3, &mut number as *mut _ as *mut c_void);
                }
                ranks.push(WindowInfo {
                    pid,
                    rect: Rect {
                        x: r.origin.x,
                        y: r.origin.y,
                        w: r.size.width,
                        h: r.size.height,
                    },
                    number: number as u32,
                    title: string(CFDictionaryGetValue(info, kCGWindowName)),
                });
            }
        }
        for window in &mut windows {
            if let Some(rect) = frame(window.ax.0) {
                if let Some(index) = match_window(window.pid, rect, &window.title, &ranks) {
                    window.number = ranks[index].number;
                }
            }
        }
        windows.sort_by_key(|w| {
            ranks
                .iter()
                .position(|i| i.number == w.number)
                .unwrap_or(usize::MAX)
        });
    }
    windows
}

pub unsafe fn activate(window: &Window) -> Result<(), String> {
    let app: id =
        msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier:window.pid];
    if app == nil {
        return Err("窗口所属程序已经退出".into());
    }
    let _: bool = msg_send![app, unhide];
    if window.minimized {
        set(window.ax.0, "AXMinimized", kCFBooleanFalse)?;
    }
    let _: bool = msg_send![app, activateWithOptions:2usize];
    let error = AXUIElementPerformAction(window.ax.0, ns("AXRaise") as Ref);
    if error != 0 {
        return Err(format!("无法激活窗口 ({error})"));
    }
    let _ = set(window.ax.0, "AXMain", kCFBooleanTrue);
    let _ = set(app_ax(window.pid).0, "AXFocusedWindow", window.ax.0);
    Ok(())
}

pub unsafe fn resize(window: Ref, rect: Rect) -> Result<(), String> {
    if attr(window, "AXFullScreen").is_some_and(|v| CFEqual(v.0, kCFBooleanTrue)) {
        return Err("请先退出 macOS 全屏模式".into());
    }
    let position = NSPoint::new(rect.x, rect.y);
    let size = NSSize::new(rect.w, rect.h);
    let p = Owned(AXValueCreate(1, &position as *const _ as *const c_void));
    let s = Owned(AXValueCreate(2, &size as *const _ as *const c_void));
    // Shrink first so a large window can move into a corner, then reapply size after moving.
    set(window, "AXSize", s.0)?;
    set(window, "AXPosition", p.0)?;
    set(window, "AXSize", s.0)?;
    let actual = frame(window).ok_or("无法读取调整后的窗口")?;
    if (actual.w - rect.w).abs() > 4.
        || (actual.h - rect.h).abs() > 4.
        || (actual.x - rect.x).abs() > 4.
        || (actual.y - rect.y).abs() > 4.
    {
        return Err("程序的最小窗口尺寸限制了缩放".into());
    }
    Ok(())
}

pub unsafe fn screens() -> Vec<(Rect, Rect)> {
    let array: id = msg_send![class!(NSScreen), screens];
    let count: usize = msg_send![array, count];
    if count == 0 {
        return vec![];
    }
    let first: id = msg_send![array, objectAtIndex:0usize];
    let primary: NSRect = msg_send![first, frame];
    (0..count)
        .map(|i| {
            let screen: id = msg_send![array, objectAtIndex:i];
            let full: NSRect = msg_send![screen, frame];
            let visible: NSRect = msg_send![screen, visibleFrame];
            let convert = |r: NSRect| {
                cocoa_to_ax(
                    Rect {
                        x: r.origin.x,
                        y: r.origin.y,
                        w: r.size.width,
                        h: r.size.height,
                    },
                    primary.size.height,
                )
            };
            (convert(full), convert(visible))
        })
        .collect()
}

pub unsafe fn primary_height() -> f64 {
    let array: id = msg_send![class!(NSScreen), screens];
    let first: id = msg_send![array, objectAtIndex:0usize];
    let frame: NSRect = msg_send![first, frame];
    frame.size.height
}

pub fn screen_at(screens: &[(Rect, Rect)], x: f64, y: f64) -> Option<Rect> {
    screens
        .iter()
        .find(|(r, _)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
        .or_else(|| screens.first())
        .map(|(_, visible)| *visible)
}

// A disposable native fixture: verification never resizes the user's real windows.
pub unsafe fn qa_windows() {
    let app: id = msg_send![class!(NSApplication), sharedApplication];
    let _: () = msg_send![app,setActivationPolicy:0isize];
    let _: () = msg_send![app, finishLaunching];
    for (i, title) in ["RingSwitch QA A", "RingSwitch QA B"].iter().enumerate() {
        let window: id = msg_send![class!(NSWindow), alloc];
        let r = NSRect::new(
            NSPoint::new(180. + i as f64 * 60., 180. + i as f64 * 60.),
            NSSize::new(480., 360.),
        );
        let window: id =
            msg_send![window,initWithContentRect:r styleMask:15usize backing:2usize defer:0i8];
        let _: () = msg_send![window,setTitle:ns(title)];
        let _: () = msg_send![window,makeKeyAndOrderFront:nil];
    }
}

pub unsafe fn native_check(pid: i32) -> Result<(), String> {
    let app = app_ax(pid);
    let list = attr(app.0, "AXWindows").ok_or("QA 窗口未公开")?;
    let mut fixtures = Vec::new();
    for i in 0..CFArrayGetCount(list.0) {
        let ax = Owned(CFRetain(CFArrayGetValueAtIndex(list.0, i)));
        let title = attr(ax.0, "AXTitle")
            .map(|v| string(v.0))
            .unwrap_or_default();
        if title.starts_with("RingSwitch QA ") {
            fixtures.push(Window {
                ax,
                pid,
                title,
                app: "RingSwitch".into(),
                minimized: false,
                number: 0,
            });
        }
    }
    if fixtures.len() != 2 {
        return Err("需要两个 RingSwitch QA 测试窗口".into());
    }
    let screen = screens().first().ok_or("没有显示器")?.1;
    let window = &fixtures[0];
    let original = frame(window.ax.0).ok_or("无法读取 QA 窗口")?;
    let result = (|| {
        for layout in crate::geometry::Layout::DIRECTIONS
            .into_iter()
            .chain([crate::geometry::Layout::Maximize])
        {
            let target = layout.rect(screen);
            resize(window.ax.0, target)?;
            let actual = frame(window.ax.0).ok_or("没有缩放回读")?;
            if (actual.x - target.x).abs() > 4.
                || (actual.y - target.y).abs() > 4.
                || (actual.w - target.w).abs() > 4.
                || (actual.h - target.h).abs() > 4.
            {
                return Err(format!("{} 回读不匹配: {actual:?}", layout.label()));
            }
        }
        println!("PASS: all 9 layouts -> real AX position/size readback");
        for w in &fixtures {
            activate(w)?;
            let actual = attr(app.0, "AXFocusedWindow").ok_or("没有焦点窗口")?;
            if actual != w.ax {
                return Err("激活窗口回读不匹配".into());
            }
        }
        println!("PASS: two windows in the same app -> exact focused window readback");
        let scoped = current_app_windows();
        if scoped.len() != 2 || scoped.iter().any(|w| w.pid != pid) {
            return Err("当前软件窗口集合包含其它软件，或缺少 QA 窗口".into());
        }
        println!("PASS: current app scope -> only the two QA windows, other apps excluded");
        let mut w = fixtures[1].clone();
        set(w.ax.0, "AXMinimized", kCFBooleanTrue)?;
        w.minimized = true;
        activate(&w)?;
        if attr(w.ax.0, "AXMinimized").is_some_and(|v| CFEqual(v.0, kCFBooleanTrue)) {
            return Err("最小化窗口未恢复".into());
        }
        println!("PASS: minimized window -> restored and raised");
        Ok(())
    })();
    let restore = resize(window.ax.0, original);
    result.and(restore)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlapping_windows_never_share_the_wrong_thumbnail() {
        let r = Rect {
            x: 10.,
            y: 10.,
            w: 500.,
            h: 400.,
        };
        let infos = vec![
            WindowInfo {
                pid: 1,
                rect: r,
                number: 11,
                title: "first".into(),
            },
            WindowInfo {
                pid: 1,
                rect: r,
                number: 12,
                title: "second".into(),
            },
        ];
        assert_eq!(match_window(1, r, "second", &infos), Some(1));
        assert_eq!(match_window(1, r, "unknown", &infos), None);
        assert_eq!(match_window(2, r, "first", &infos), None);
        assert_eq!(match_window(1, r, "unknown", &infos[..1]), Some(0));
    }
}
