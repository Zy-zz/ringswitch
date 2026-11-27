use crate::geometry::{cocoa_to_ax, Layout, Rect};
use cocoa::base::{id, nil};
use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::{c_char, c_void, CStr};
use std::ptr::null;
use std::sync::OnceLock;

pub type Ref = *const c_void;

#[repr(C)]
pub struct CFRunLoopSourceContext {
    pub version: isize,
    pub info: *mut c_void,
    pub retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
    pub release: Option<extern "C" fn(*const c_void)>,
    pub copy_description: Option<extern "C" fn(*const c_void) -> Ref>,
    pub equal: Option<extern "C" fn(*const c_void, *const c_void) -> u8>,
    pub hash: Option<extern "C" fn(*const c_void) -> usize>,
    pub schedule: Option<extern "C" fn(*mut c_void, Ref, Ref)>,
    pub cancel: Option<extern "C" fn(*mut c_void, Ref, Ref)>,
    pub perform: Option<extern "C" fn(*mut c_void)>,
}

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub fn CGContextDrawImage(context: Ref, rect: NSRect, image: Ref);
    pub fn CGContextSetInterpolationQuality(context: Ref, quality: i32);
    pub fn CGContextSetAlpha(context: Ref, alpha: f64);
    pub fn CGContextSaveGState(context: Ref);
    pub fn CGContextRestoreGState(context: Ref);
}

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
    pub fn CFRunLoopSourceCreate(
        allocator: Ref,
        order: isize,
        context: *mut CFRunLoopSourceContext,
    ) -> Ref;
    pub fn CFRunLoopSourceSignal(source: Ref);
    pub fn CFRunLoopGetCurrent() -> Ref;
    pub fn CFRunLoopGetMain() -> Ref;
    pub fn CFRunLoopAddSource(runloop: Ref, source: Ref, mode: Ref);
    pub fn CFRunLoopWakeUp(runloop: Ref);
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

/// Thread-safe transfer wrapper for CoreFoundation handles such as AXUIElementRef.
pub struct SendOwned(pub Owned);
unsafe impl Send for SendOwned {}

type AxGetWindowFn = unsafe extern "C" fn(Ref, *mut u32) -> i32;

/// Resolve the system's `_AXUIElementGetWindow` symbol once so overlapping or
/// maximized windows always resolve to their exact CGWindowID for previews.
unsafe fn ax_window_number(element: Ref) -> Option<u32> {
    static FN: OnceLock<Option<AxGetWindowFn>> = OnceLock::new();
    let get_window = *FN.get_or_init(|| {
        // RTLD_DEFAULT on macOS is (void *)-2.
        let sym = dlsym((-2isize) as *mut c_void, c"_AXUIElementGetWindow".as_ptr());
        if sym.is_null() {
            None
        } else {
            Some(std::mem::transmute::<*mut c_void, AxGetWindowFn>(sym))
        }
    });
    let mut id: u32 = 0;
    if let Some(f) = get_window {
        if f(element, &mut id) == 0 && id != 0 {
            return Some(id);
        }
    }
    None
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
    copy_attr(element, name, 0.08).ok()
}

// Only retry idempotent AX reads, assignments and AXRaise; a busy app may finish later.
fn ax_request<T>(mut request: impl FnMut(f32) -> Result<T, i32>) -> Result<T, i32> {
    match request(0.08) {
        Err(-25204) => request(1.0),
        result => result,
    }
}

unsafe fn copy_attr(element: Ref, name: &str, timeout: f32) -> Result<Owned, i32> {
    let mut value = null();
    AXUIElementSetMessagingTimeout(element, timeout);
    let error = AXUIElementCopyAttributeValue(element, ns(name) as Ref, &mut value);
    if error == 0 && !value.is_null() {
        Ok(Owned(value))
    } else {
        Err(if error == 0 { -25212 } else { error })
    }
}

fn busy_error() -> String {
    "当前软件暂时未响应，请稍后重试 (-25204)".into()
}

unsafe fn query_attr(element: Ref, name: &str) -> Result<Option<Owned>, String> {
    match ax_request(|timeout| copy_attr(element, name, timeout)) {
        Ok(value) => Ok(Some(value)),
        Err(-25204) => Err(busy_error()),
        Err(_) => Ok(None),
    }
}

pub unsafe fn set(element: Ref, name: &str, value: Ref) -> Result<(), String> {
    ax_request(|timeout| {
        AXUIElementSetMessagingTimeout(element, timeout);
        let error = AXUIElementSetAttributeValue(element, ns(name) as Ref, value);
        if error == 0 {
            Ok(())
        } else {
            Err(error)
        }
    })
    .map_err(|error| format!("{name}: Accessibility error {error}"))
}
pub unsafe fn frame(element: Ref) -> Option<Rect> {
    frame_result(element).ok().flatten()
}

unsafe fn frame_result(element: Ref) -> Result<Option<Rect>, String> {
    let Some(p) = query_attr(element, "AXPosition")? else {
        return Ok(None);
    };
    let Some(s) = query_attr(element, "AXSize")? else {
        return Ok(None);
    };
    let mut position = NSPoint::new(0., 0.);
    let mut size = NSSize::new(0., 0.);
    if !AXValueGetValue(p.0, 1, &mut position as *mut _ as *mut c_void)
        || !AXValueGetValue(s.0, 2, &mut size as *mut _ as *mut c_void)
    {
        return Ok(None);
    }
    Ok(Some(Rect {
        x: position.x,
        y: position.y,
        w: size.width,
        h: size.height,
    }))
}

#[derive(Clone)]
pub struct Window {
    pub ax: Owned,
    pub pid: i32,
    pub app: String,
    pub title: String,
    pub minimized: bool,
    pub number: u32,
    pub rect: Rect,
}

unsafe fn app_ax(pid: i32) -> Owned {
    Owned(AXUIElementCreateApplication(pid))
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
    windows_for(None).unwrap_or_default()
}

pub unsafe fn current_app_windows() -> Result<Vec<Window>, String> {
    match frontmost_pid() {
        Some(pid) => windows_for(Some(pid)),
        None => Ok(vec![]),
    }
}

unsafe fn windows_for(scope: Option<i32>) -> Result<Vec<Window>, String> {
    let mut windows = Vec::new();
    let self_pid = std::process::id() as i32;
    if let Some(pid) = scope {
        if pid != self_pid {
            let app: id = msg_send![
                class!(NSRunningApplication),
                runningApplicationWithProcessIdentifier:pid
            ];
            if app != nil {
                let policy: isize = msg_send![app, activationPolicy];
                if policy == 0 {
                    let name: id = msg_send![app, localizedName];
                    windows.extend(app_windows(pid, &string(name as Ref))?);
                }
            }
        }
    } else {
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let apps: id = msg_send![workspace, runningApplications];
        let count: usize = msg_send![apps, count];
        for i in 0..count {
            let app: id = msg_send![apps, objectAtIndex:i];
            let policy: isize = msg_send![app, activationPolicy];
            let pid: i32 = msg_send![app, processIdentifier];
            if policy != 0 || pid == self_pid {
                continue;
            }
            let name: id = msg_send![app, localizedName];
            if let Ok(found) = app_windows(pid, &string(name as Ref)) {
                windows.extend(found);
            }
        }
    }
    // Seed front-to-back z-order and fallback CGWindowIDs from WindowServer.
    if !windows.is_empty() {
        if let Some(list) = std::ptr::NonNull::new(CGWindowListCopyWindowInfo(0, 0) as *mut c_void)
        {
            let list = Owned(list.as_ptr());
            let mut ranks = Vec::new();
            for i in 0..CFArrayGetCount(list.0) {
                let info = CFArrayGetValueAtIndex(list.0, i);
                let mut pid = 0i32;
                let p = CFDictionaryGetValue(info, kCGWindowOwnerPID);
                if p.is_null() {
                    continue;
                }
                CFNumberGetValue(p, 3, &mut pid as *mut _ as *mut c_void);
                if scope.is_some_and(|target| pid != target) {
                    continue;
                }
                let mut layer = 0i32;
                let l = CFDictionaryGetValue(info, kCGWindowLayer);
                if l.is_null() {
                    continue;
                }
                CFNumberGetValue(l, 3, &mut layer as *mut _ as *mut c_void);
                if layer != 0 {
                    continue;
                }
                let mut r = NSRect::new(NSPoint::new(0., 0.), NSSize::new(0., 0.));
                if CGRectMakeWithDictionaryRepresentation(
                    CFDictionaryGetValue(info, kCGWindowBounds),
                    &mut r,
                ) {
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
                if window.number == 0 {
                    if let Some(index) =
                        match_window(window.pid, window.rect, &window.title, &ranks)
                    {
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
    }
    Ok(windows)
}

unsafe fn app_windows(pid: i32, name: &str) -> Result<Vec<Window>, String> {
    let ax = app_ax(pid);
    let mut elements = Vec::new();
    if let Some(list) = query_attr(ax.0, "AXWindows")? {
        for j in 0..CFArrayGetCount(list.0) {
            elements.push(Owned(CFRetain(CFArrayGetValueAtIndex(list.0, j))));
        }
    }
    // Some apps omit the focused/main window from AXWindows on another Space.
    for key in ["AXFocusedWindow", "AXMainWindow"] {
        if let Some(w) = query_attr(ax.0, key)? {
            if !elements.contains(&w) {
                elements.push(w);
            }
        }
    }
    let mut windows = Vec::new();
    for element in elements {
        if query_attr(element.0, "AXRole")?
            .map(|v| string(v.0))
            .as_deref()
            != Some("AXWindow")
        {
            continue;
        }
        let Some(rect) = frame_result(element.0)? else {
            continue;
        };
        if rect.w < 80. || rect.h < 40. {
            continue;
        }
        let title = query_attr(element.0, "AXTitle")?
            .map(|v| string(v.0))
            .unwrap_or_default();
        let minimized =
            query_attr(element.0, "AXMinimized")?.is_some_and(|v| CFEqual(v.0, kCFBooleanTrue));
        let number = ax_window_number(element.0).unwrap_or(0);
        windows.push(Window {
            ax: element,
            pid,
            app: name.into(),
            title,
            minimized,
            number,
            rect,
        });
    }
    Ok(windows)
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
    let result = ax_request(|timeout| {
        AXUIElementSetMessagingTimeout(window.ax.0, timeout);
        let error = AXUIElementPerformAction(window.ax.0, ns("AXRaise") as Ref);
        if error == 0 {
            Ok(())
        } else {
            Err(error)
        }
    });
    result.map_err(|error| {
        if error == -25204 {
            busy_error()
        } else {
            format!("无法激活窗口 ({error})")
        }
    })?;
    let _ = set(window.ax.0, "AXMain", kCFBooleanTrue);
    let _ = set(app_ax(window.pid).0, "AXFocusedWindow", window.ax.0);
    Ok(())
}

pub unsafe fn resize(window: Ref, rect: Rect) -> Result<(), String> {
    resize_to(window, || Ok(rect), || false)
}

pub unsafe fn tile(window: Ref, layout: Layout, x: f64, y: f64) -> Result<(), String> {
    tile_with_cancel(window, layout, x, y, || false)
}

pub unsafe fn tile_with_cancel(
    window: Ref,
    layout: Layout,
    x: f64,
    y: f64,
    cancelled: impl Fn() -> bool,
) -> Result<(), String> {
    let pool = NSAutoreleasePool::new(nil);
    let result = resize_to(
        window,
        || {
            let screen = screen_at(&screens(), x, y).ok_or("没有可用显示器")?;
            Ok(layout.rect(screen))
        },
        cancelled,
    );
    pool.drain();
    result
}

unsafe fn resize_to(
    window: Ref,
    target: impl Fn() -> Result<Rect, String>,
    cancelled: impl Fn() -> bool,
) -> Result<(), String> {
    if cancelled() {
        return Ok(());
    }
    if query_attr(window, "AXFullScreen")?.is_some_and(|v| CFEqual(v.0, kCFBooleanTrue)) {
        set(window, "AXFullScreen", kCFBooleanFalse)
            .map_err(|e| format!("无法退出 macOS 全屏：{e}"))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        loop {
            if cancelled() {
                return Ok(());
            }
            // AXFullScreen can clear before the Space animation allows resizing.
            std::thread::sleep(std::time::Duration::from_millis(100));
            if cancelled() {
                return Ok(());
            }
            let state =
                query_attr(window, "AXFullScreen")?.ok_or("无法确认窗口是否已退出 macOS 全屏")?;
            if CFEqual(state.0, kCFBooleanFalse)
                && resize_frame(window, target()?, &cancelled).is_ok()
            {
                if cancelled() {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
                if cancelled() {
                    return Ok(());
                }
                if resize_frame(window, target()?, &cancelled).is_ok() {
                    return Ok(());
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err("退出全屏后未能完成缩放，请稍后重试（或检查窗口最小尺寸限制）".into());
            }
        }
    }
    resize_frame(window, target()?, &cancelled)
}

unsafe fn resize_frame(
    window: Ref,
    rect: Rect,
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    let position = NSPoint::new(rect.x, rect.y);
    let size = NSSize::new(rect.w, rect.h);
    let p = Owned(AXValueCreate(1, &position as *const _ as *const c_void));
    let s = Owned(AXValueCreate(2, &size as *const _ as *const c_void));
    settle_frame_cancellable(
        rect,
        |apply| {
            if apply {
                // Moving or growing can clamp the other attribute, especially across displays.
                set(window, "AXSize", s.0)?;
                set(window, "AXPosition", p.0)?;
                set(window, "AXSize", s.0)?;
                set(window, "AXPosition", p.0)?;
            }
            frame_result(window)?.ok_or("无法读取调整后的窗口".into())
        },
        || std::thread::sleep(std::time::Duration::from_millis(50)),
        cancelled,
    )
}

fn frame_matches(actual: Rect, target: Rect) -> bool {
    (actual.x - target.x).abs() <= 4.
        && (actual.y - target.y).abs() <= 4.
        && (actual.w - target.w).abs() <= 4.
        && (actual.h - target.h).abs() <= 4.
}

#[cfg(test)]
fn settle_frame(
    target: Rect,
    step: impl FnMut(bool) -> Result<Rect, String>,
    wait: impl FnMut(),
) -> Result<(), String> {
    settle_frame_cancellable(target, step, wait, &|| false)
}

fn settle_frame_cancellable(
    target: Rect,
    mut step: impl FnMut(bool) -> Result<Rect, String>,
    mut wait: impl FnMut(),
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    let mut stable = 0;
    for poll in 0..30 {
        if cancelled() {
            return Ok(());
        }
        let actual = step(stable == 0 && poll % 3 == 0)?;
        stable = if frame_matches(actual, target) {
            stable + 1
        } else {
            0
        };
        // AX success can precede an animation or a delayed app layout that restores the old frame.
        if stable == 5 {
            return Ok(());
        }
        if poll < 29 {
            wait();
        }
    }
    Err("窗口未能保持目标位置和尺寸，请检查软件的窗口尺寸限制或是否仍在切换全屏".into())
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

pub unsafe fn max_refresh_rate() -> f64 {
    let array: id = msg_send![class!(NSScreen), screens];
    let count: usize = msg_send![array, count];
    let mut max_fps = 60.0f64;
    for i in 0..count {
        let screen: id = msg_send![array, objectAtIndex:i];
        let responds: bool = msg_send![screen, respondsToSelector:sel!(maximumFramesPerSecond)];
        if responds {
            let fps: isize = msg_send![screen, maximumFramesPerSecond];
            if fps > 0 {
                max_fps = max_fps.max((fps as f64).clamp(60.0, 120.0));
            }
        }
    }
    max_fps
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
            let rect = frame(ax.0).unwrap_or(Rect {
                x: 0.,
                y: 0.,
                w: 480.,
                h: 360.,
            });
            fixtures.push(Window {
                ax,
                pid,
                title,
                app: "RingSwitch".into(),
                minimized: false,
                number: 0,
                rect,
            });
        }
    }
    if fixtures.len() != 2 {
        return Err("需要两个 RingSwitch QA 测试窗口".into());
    }
    let displays = screens();
    let screen = displays.first().ok_or("没有显示器")?.1;
    let window = &fixtures[0];
    let original = frame(window.ax.0).ok_or("无法读取 QA 窗口")?;
    let result = (|| {
        let mut transitions = 0;
        for (_, source) in &displays {
            for (full, _) in &displays {
                let anchor = (full.x + full.w / 2., full.y + full.h / 2.);
                for layout in Layout::DIRECTIONS.into_iter().chain([Layout::Maximize]) {
                    resize(window.ax.0, *source).map_err(|e| {
                        format!(
                            "准备起始布局 {source:?} 失败: {e}, 实际: {:?}",
                            frame(window.ax.0)
                        )
                    })?;
                    tile(window.ax.0, layout, anchor.0, anchor.1).map_err(|e| {
                        format!(
                            "{} 至显示器 {full:?} 失败: {e}, 实际: {:?}",
                            layout.label(),
                            frame(window.ax.0)
                        )
                    })?;
                    let target = layout.rect(screen_at(&screens(), anchor.0, anchor.1).unwrap());
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    let actual = frame(window.ax.0).ok_or("没有缩放回读")?;
                    if !frame_matches(actual, target) {
                        return Err(format!(
                            "{} 回读不匹配: {actual:?}, 目标: {target:?}",
                            layout.label()
                        ));
                    }
                    transitions += 1;
                }
            }
        }
        println!("PASS: {transitions} same/cross-display transitions -> all 9 layouts stable after one call");
        set(window.ax.0, "AXFullScreen", kCFBooleanTrue)?;
        std::thread::sleep(std::time::Duration::from_secs(2));
        if !query_attr(window.ax.0, "AXFullScreen")?.is_some_and(|v| CFEqual(v.0, kCFBooleanTrue)) {
            return Err("QA 窗口未进入原生全屏".into());
        }
        let anchor = (screen.x + screen.w / 2., screen.y + screen.h / 2.);
        tile(window.ax.0, Layout::Right, anchor.0, anchor.1)?;
        let target = Layout::Right.rect(screen_at(&screens(), anchor.0, anchor.1).unwrap());
        std::thread::sleep(std::time::Duration::from_secs(1));
        let actual = frame(window.ax.0).ok_or("没有退出全屏后的窗口回读")?;
        if !query_attr(window.ax.0, "AXFullScreen")?.is_some_and(|v| CFEqual(v.0, kCFBooleanFalse))
            || (actual.x - target.x).abs() > 4.
            || (actual.y - target.y).abs() > 4.
            || (actual.w - target.w).abs() > 4.
            || (actual.h - target.h).abs() > 4.
        {
            return Err(format!("退出全屏后布局未保持: {actual:?}"));
        }
        println!("PASS: native fullscreen -> exit -> tiled frame survives Space animation");
        for w in &fixtures {
            activate(w)?;
            let actual = attr(app.0, "AXFocusedWindow").ok_or("没有焦点窗口")?;
            if actual != w.ax {
                return Err("激活窗口回读不匹配".into());
            }
        }
        println!("PASS: two windows in the same app -> exact focused window readback");
        let scoped = current_app_windows()?;
        if scoped.len() != 2 || scoped.iter().any(|w| w.pid != pid) {
            return Err("当前软件窗口集合包含其它软件，或缺少 QA 窗口".into());
        }
        println!("PASS: current app scope -> only the two QA windows, other apps excluded");
        let mut w = fixtures[1].clone();
        set(w.ax.0, "AXMinimized", kCFBooleanTrue)?;
        std::thread::sleep(std::time::Duration::from_millis(500));
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
    fn resize_waits_for_delayed_layout_and_reapplies_clamped_frame() {
        let target = Rect {
            x: 500.,
            y: 30.,
            w: 500.,
            h: 700.,
        };
        let clamped = Rect {
            x: 0.,
            w: 400.,
            ..target
        };
        let mut polls = 0;
        let mut writes = 0;
        let mut waits = 0;
        settle_frame(
            target,
            |apply| {
                polls += 1;
                writes += usize::from(apply);
                // The app applies the size later; only the next assignment can place it correctly.
                Ok(if writes >= 2 && polls >= 6 {
                    target
                } else {
                    clamped
                })
            },
            || waits += 1,
        )
        .unwrap();
        assert_eq!(writes, 2);
        assert_eq!(polls, 10);
        assert_eq!(waits, 9);
    }

    #[test]
    fn resize_corrects_a_frame_that_bounces_back_after_initial_success() {
        let target = Rect {
            x: -1000.,
            y: -800.,
            w: 1000.,
            h: 800.,
        };
        let old = Rect {
            x: 0.,
            y: 0.,
            w: 500.,
            h: 400.,
        };
        let mut polls = 0;
        let mut writes = 0;
        settle_frame(
            target,
            |apply| {
                polls += 1;
                writes += usize::from(apply);
                Ok(if polls < 3 || writes >= 2 {
                    target
                } else {
                    old
                })
            },
            || {},
        )
        .unwrap();
        assert_eq!(writes, 2);
        assert_eq!(polls, 8);
    }

    #[test]
    fn resize_bounds_retries_for_size_limits_and_propagates_ax_errors() {
        let target = Rect {
            x: 0.,
            y: 0.,
            w: 500.,
            h: 400.,
        };
        let mut writes = 0;
        let mut waits = 0;
        assert!(settle_frame(
            target,
            |apply| {
                writes += usize::from(apply);
                Ok(Rect { w: 600., ..target })
            },
            || waits += 1
        )
        .is_err());
        assert_eq!(writes, 10);
        assert_eq!(waits, 29);
        let error = settle_frame(
            target,
            |_| Err("AXPosition: Accessibility error -25205".into()),
            || panic!("must not retry unsupported attributes"),
        );
        assert_eq!(error.unwrap_err(), "AXPosition: Accessibility error -25205");
    }

    #[test]
    fn ax_retries_only_transient_messaging_failures_once() {
        for (errors, expected, timeouts) in [
            (vec![0], 0, vec![0.08]),
            (vec![-25202], -25202, vec![0.08]),
            (vec![-25212], -25212, vec![0.08]),
            (vec![-25204, 0], 0, vec![0.08, 1.0]),
            (vec![-25204, -25204], -25204, vec![0.08, 1.0]),
        ] {
            let mut seen = Vec::new();
            let mut errors = errors.into_iter();
            let result = ax_request(|timeout| {
                seen.push(timeout);
                match errors.next().expect("unexpected AX request") {
                    0 => Ok(()),
                    error => Err(error),
                }
            });
            assert_eq!(result.err().unwrap_or(0), expected);
            assert_eq!(seen, timeouts);
        }
    }

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

    #[test]
    fn settle_frame_stops_immediately_when_superseded() {
        let target = Rect {
            x: 0.,
            y: 0.,
            w: 500.,
            h: 400.,
        };
        let polls = std::cell::Cell::new(0);
        let res = settle_frame_cancellable(
            target,
            |_| {
                polls.set(polls.get() + 1);
                Ok(Rect { w: 300., ..target })
            },
            || {},
            &|| polls.get() >= 2,
        );
        assert!(res.is_ok());
        assert_eq!(polls.get(), 2);
    }

    #[test]
    fn ax_window_number_symbol_is_available_on_macos() {
        let sym = unsafe { dlsym((-2isize) as *mut c_void, c"_AXUIElementGetWindow".as_ptr()) };
        assert!(!sym.is_null());
    }
}
