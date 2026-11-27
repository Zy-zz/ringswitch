use crate::{
    anim::{Spring, SpringAngle, SpringRect, SPRING_FLUID, SPRING_GLOW, SPRING_POP, SPRING_SNAPPY},
    capture,
    config::Config,
    geometry::{cycle, selection, Layout, Rect},
    input::{self, Command, Controls, Dispatcher},
    mac::*,
};
use cocoa::base::{id, nil, NO, YES};
use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize};
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{Class, Object, Sel, BOOL},
    sel, sel_impl,
};
use std::f64::consts::PI;
use std::ffi::c_void;
use std::os::fd::AsRawFd;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[link(name = "QuartzCore", kind = "framework")]
extern "C" {}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPathCreateMutable() -> Ref;
    fn CGPathAddEllipseInRect(path: Ref, transform: Ref, rect: NSRect);
}

extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}

const CARD_W: f64 = 244.0;
const CARD_H: f64 = 184.0;
const CARD_GAP_X: f64 = 14.0;
const CARD_GAP_Y: f64 = 14.0;
const SWITCHER_PAD_X: f64 = 20.0;
const SWITCHER_HEADER_H: f64 = 52.0;
const SWITCHER_PAD_BOTTOM: f64 = 18.0;
const SWITCHER_DOTS_H: f64 = 18.0;

const RADIAL_PANEL_SIZE: f64 = 210.0;
const RADIAL_CENTER: f64 = RADIAL_PANEL_SIZE * 0.5;
const RADIAL_OUTER_R: f64 = 75.0;
const RADIAL_INNER_R: f64 = 36.0;

const MAX_CACHED_THUMBS: usize = 48;
const THUMB_REFRESH_AGE: Duration = Duration::from_millis(2000);

fn instance_lock() -> Result<Option<std::fs::File>, String> {
    let directory = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/RingSwitch");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("instance.lock"))
        .map_err(|e| e.to_string())?;
    if unsafe { flock(file.as_raw_fd(), 6) } == 0 {
        Ok(Some(file))
    } else {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            Ok(None)
        } else {
            Err(error.to_string())
        }
    }
}

struct CachedThumb {
    image: Owned,
    captured_at: Instant,
    fade: f64,
}

struct TileJob {
    window: SendOwned,
    layout: Layout,
    origin: (f64, f64),
    gen: u64,
}

struct Radial {
    window: Owned,
    window_rect: Option<Rect>,
    origin: (f64, f64),
    screen: Rect,
    selected: Option<Layout>,
    armed: bool,
    started: Instant,
    cursor_dist: f64,
    has_angle: bool,
}

struct WakeSource {
    source: usize,
    runloop: usize,
}
unsafe impl Send for WakeSource {}
unsafe impl Sync for WakeSource {}

impl WakeSource {
    fn wake(&self) {
        unsafe {
            CFRunLoopSourceSignal(self.source as Ref);
            CFRunLoopWakeUp(self.runloop as Ref);
        }
    }
}

thread_local! { static APP: RefCell<Option<App>> = const { RefCell::new(None) }; }

struct App {
    // Dedicated panels for each surface (zero cross-mode mask/radius glitches).
    switcher_panel: id,
    switcher_view: id,
    help: id,
    help_popover: id,

    radial_panel: id,
    radial_glass: id,
    radial_view: id,

    preview_panel: id,
    preview_view: id,

    toast_panel: id,
    toast_view: id,

    status: id,
    menu: id,
    target: id,
    anim_timer: id,
    last_anim: Instant,

    controls: Arc<Controls>,
    rx: mpsc::Receiver<Command>,
    tx: Dispatcher,
    tile_tx: mpsc::Sender<TileJob>,
    tile_gen: Arc<AtomicU64>,

    hooked: bool,
    demo: bool,
    instant_mode: bool,

    // Switcher state & animations
    windows: Vec<Window>,
    selected: usize,
    switching: bool,
    switcher_visible: bool,
    switcher_alpha: Spring,
    switcher_sel_rect: SpringRect,
    switcher_has_sel: bool,
    switcher_open_mouse: Option<(f64, f64)>,
    switcher_mouse_armed: bool,
    grid: (usize, usize),
    session: u64,
    thumbnails: HashMap<u32, CachedThumb>,
    requested: HashSet<u32>,
    in_flight: HashSet<u32>,
    app_icon: Option<(i32, id)>,
    shimmer_phase: f64,

    // Radial state & animations
    radial: Option<Radial>,
    radial_visible: bool,
    radial_alpha: Spring,
    radial_scale: Spring,
    radial_angle: SpringAngle,
    radial_sector_glow: [Spring; 8],
    radial_center_glow: Spring,
    radial_last_mask_scale: f64,

    // Target preview state & animations
    preview_visible: bool,
    preview_alpha: Spring,
    preview_rect: SpringRect,
    preview_layout: Option<Layout>,

    // Toast state & animations
    message: String,
    message_until: Instant,
    toast_visible: bool,
    toast_alpha: Spring,

    // MRU tracking
    mru: Vec<Owned>,
}

unsafe fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

unsafe fn color(r: f64, g: f64, b: f64, a: f64) -> id {
    msg_send![class!(NSColor), colorWithSRGBRed:r green:g blue:b alpha:a]
}

unsafe fn accent_rgb() -> (f64, f64, f64) {
    let accent: id = msg_send![class!(NSColor), controlAccentColor];
    let space: id = msg_send![class!(NSColorSpace), sRGBColorSpace];
    let srgb: id = msg_send![accent, colorUsingColorSpace:space];
    if srgb != nil {
        let r: f64 = msg_send![srgb, redComponent];
        let g: f64 = msg_send![srgb, greenComponent];
        let b: f64 = msg_send![srgb, blueComponent];
        // Blend slightly toward luminous sky-cyan for crisp glass readability.
        return (
            r * 0.72 + 0.28 * 0.26,
            g * 0.72 + 0.28 * 0.64,
            b * 0.72 + 0.28 * 1.0,
        );
    }
    (0.26, 0.64, 1.0)
}

unsafe fn is_dark_view(view: id) -> bool {
    let appearance: id = msg_send![view, effectiveAppearance];
    if appearance == nil {
        return true;
    }
    let dark = ns("NSAppearanceNameDarkAqua");
    let light = ns("NSAppearanceNameAqua");
    let names = [dark, light];
    let arr: id = msg_send![class!(NSArray), arrayWithObjects:names.as_ptr() count:2usize];
    let best: id = msg_send![appearance, bestMatchFromAppearancesWithNames:arr];
    if best == nil {
        return true;
    }
    let eq: bool = msg_send![best, isEqualToString:dark];
    eq
}

unsafe fn fill(r: NSRect, c: id, radius: f64) {
    let _: () = msg_send![c, set];
    let path: id =
        msg_send![class!(NSBezierPath), bezierPathWithRoundedRect:r xRadius:radius yRadius:radius];
    let _: () = msg_send![path, fill];
}

unsafe fn outline(r: NSRect, c: id, radius: f64, width: f64) {
    let _: () = msg_send![c, set];
    let path: id =
        msg_send![class!(NSBezierPath), bezierPathWithRoundedRect:r xRadius:radius yRadius:radius];
    let _: () = msg_send![path, setLineWidth:width];
    let _: () = msg_send![path, stroke];
}

unsafe fn font_weight(size: f64, weight: f64) -> id {
    msg_send![class!(NSFont), systemFontOfSize:size weight:weight]
}

unsafe fn mono_digit_font(size: f64, weight: f64) -> id {
    msg_send![class!(NSFont), monospacedDigitSystemFontOfSize:size weight:weight]
}

unsafe fn measure_text_size(s: &str, font: id) -> NSSize {
    let attrs: id = msg_send![class!(NSDictionary), dictionaryWithObject:font forKey:ns("NSFont")];
    msg_send![ns(s), sizeWithAttributes:attrs]
}

unsafe fn measure_text(s: &str, font: id) -> f64 {
    measure_text_size(s, font).width.ceil()
}

/// Draw single-line text centered both horizontally and vertically inside `box_r`.
/// Uses `drawAtPoint:` so it is completely immune to arm64 vs x86_64 NSTextAlignment ABI differences.
unsafe fn draw_text_centered(s: &str, box_r: NSRect, font: id, c: id) {
    let sz = measure_text_size(s, font);
    let keys = [ns("NSFont"), ns("NSColor")];
    let values = [font, c];
    let attrs: id = msg_send![
        class!(NSDictionary),
        dictionaryWithObjects:values.as_ptr()
        forKeys:keys.as_ptr()
        count:2usize
    ];
    let pt = NSPoint::new(
        (box_r.origin.x + (box_r.size.width - sz.width) * 0.5).round(),
        (box_r.origin.y + (box_r.size.height - sz.height) * 0.5).round(),
    );
    let _: () = msg_send![ns(s), drawAtPoint:pt withAttributes:attrs];
}

/// Draw single-line left-aligned text vertically centered on `mid_y`, truncating with ellipsis at `max_w`.
unsafe fn draw_text_left(s: &str, x: f64, mid_y: f64, max_w: f64, font: id, c: id) {
    let sz = measure_text_size(s, font);
    let h = sz.height.ceil();
    let style: id = msg_send![class!(NSMutableParagraphStyle), new];
    let _: () = msg_send![style, setLineBreakMode:4usize]; // NSLineBreakByTruncatingTail
    let _: () = msg_send![style, setAlignment:0usize]; // NSTextAlignmentLeft == 0 on all architectures
    let keys = [ns("NSFont"), ns("NSColor"), ns("NSParagraphStyle")];
    let values = [font, c, style];
    let attrs: id = msg_send![
        class!(NSDictionary),
        dictionaryWithObjects:values.as_ptr()
        forKeys:keys.as_ptr()
        count:3usize
    ];
    let r = rect(x, (mid_y - h * 0.5).round(), max_w.max(1.0), h);
    let _: () = msg_send![ns(s), drawWithRect:r options:33usize attributes:attrs];
    let _: () = msg_send![style, release];
}

unsafe fn draw_cached_icon(icon_img: id, r: NSRect) {
    if icon_img != nil {
        let _: () = msg_send![icon_img, drawInRect:r];
    }
}

unsafe fn draw_cgimage_rounded(image: Ref, r: NSRect, radius: f64, alpha: f64) {
    let gc: id = msg_send![class!(NSGraphicsContext), currentContext];
    if gc == nil {
        return;
    }
    let ctx: Ref = msg_send![gc, CGContext];
    if ctx.is_null() {
        return;
    }
    let _: () = msg_send![class!(NSGraphicsContext), saveGraphicsState];
    let clip: id =
        msg_send![class!(NSBezierPath), bezierPathWithRoundedRect:r xRadius:radius yRadius:radius];
    let _: () = msg_send![clip, addClip];
    CGContextSaveGState(ctx);
    CGContextSetInterpolationQuality(ctx, 3); // kCGInterpolationHigh
    CGContextSetAlpha(ctx, alpha.clamp(0.0, 1.0));
    CGContextDrawImage(ctx, r, image);
    CGContextRestoreGState(ctx);
    let _: () = msg_send![class!(NSGraphicsContext), restoreGraphicsState];
}

/// Draw a tiny minimalist geometric window layout glyph inside `r`.
unsafe fn draw_layout_glyph(layout: Layout, r: NSRect, frame_color: id, fill_color: id) {
    let radius = (r.size.height * 0.22).clamp(1.5, 3.0);
    outline(r, frame_color, radius, 1.1);
    let pad = 1.6;
    let ix = r.origin.x + pad;
    let iy = r.origin.y + pad;
    let iw = (r.size.width - pad * 2.0).max(2.0);
    let ih = (r.size.height - pad * 2.0).max(2.0);
    let (fx, fy, fw, fh) = match layout {
        // Note: Cocoa NSView has bottom-left origin!
        Layout::Maximize => (0.0, 0.0, 1.0, 1.0),
        Layout::Left => (0.0, 0.0, 0.48, 1.0),
        Layout::Right => (0.52, 0.0, 0.48, 1.0),
        Layout::Top => (0.0, 0.52, 1.0, 0.48),
        Layout::Bottom => (0.0, 0.0, 1.0, 0.48),
        Layout::TopLeft => (0.0, 0.52, 0.48, 0.48),
        Layout::TopRight => (0.52, 0.52, 0.48, 0.48),
        Layout::BottomLeft => (0.0, 0.0, 0.48, 0.48),
        Layout::BottomRight => (0.52, 0.0, 0.48, 0.48),
    };
    let inner = rect(ix + iw * fx, iy + ih * fy, iw * fw, ih * fh);
    fill(inner, fill_color, (radius - 0.8).max(1.0));
}

unsafe fn set_donut_mask(material: id, scale: f64) {
    let outer = RADIAL_OUTER_R * scale;
    let inner = RADIAL_INNER_R * scale;
    let d = outer * 2.0;
    // Match material bounds exactly to the outer ring so macOS 26 NSGlassEffectView
    // renders its native specular Liquid Glass lens refraction right on the ring's outer edge.
    let _: () = msg_send![
        material,
        setFrame:rect(RADIAL_CENTER - outer, RADIAL_CENTER - outer, d, d)
    ];
    if Class::get("NSGlassEffectView").is_some() {
        let _: () = msg_send![material, setCornerRadius:outer];
    } else {
        let _: () = msg_send![material, setWantsLayer:YES];
        let layer: id = msg_send![material, layer];
        let _: () = msg_send![layer, setCornerRadius:outer];
    }
    let _: () = msg_send![material, setWantsLayer:YES];
    let layer: id = msg_send![material, layer];
    let path = Owned(CGPathCreateMutable());
    CGPathAddEllipseInRect(path.0, std::ptr::null(), rect(0.0, 0.0, d, d));
    CGPathAddEllipseInRect(
        path.0,
        std::ptr::null(),
        rect(outer - inner, outer - inner, inner * 2.0, inner * 2.0),
    );
    let mask: id = msg_send![class!(CAShapeLayer), layer];
    let _: () = msg_send![mask, setPath:path.0];
    let _: () = msg_send![mask, setFillRule:ns("even-odd")];
    let _: () = msg_send![layer, setMask:mask];
}

fn card_frame_in_switcher(position: usize, cols: usize, view_h: f64) -> Rect {
    let col = position % cols;
    let row = position / cols;
    let x = SWITCHER_PAD_X + col as f64 * (CARD_W + CARD_GAP_X);
    let y = view_h - SWITCHER_HEADER_H - (row + 1) as f64 * CARD_H - row as f64 * CARD_GAP_Y;
    Rect {
        x,
        y,
        w: CARD_W,
        h: CARD_H,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SwitchHit {
    Card(usize),
    Page(usize),
}

fn switcher_hit_test(
    pt: (f64, f64),
    view_w: f64,
    view_h: f64,
    grid: (usize, usize),
    selected: usize,
    total_windows: usize,
) -> Option<SwitchHit> {
    let capacity = (grid.0 * grid.1).max(1);
    let start = selected / capacity * capacity;
    let visible = total_windows.saturating_sub(start).min(capacity);
    for position in 0..visible {
        let cf = card_frame_in_switcher(position, grid.0, view_h);
        if pt.0 >= cf.x && pt.0 <= cf.x + cf.w && pt.1 >= cf.y && pt.1 <= cf.y + cf.h {
            return Some(SwitchHit::Card(start + position));
        }
    }
    let pages = total_windows.div_ceil(capacity);
    if pages > 1 && pt.1 >= 2.0 && pt.1 <= 26.0 {
        let current_page = selected / capacity;
        let total_w = (pages as f64 - 1.0) * 12.0 + 16.0;
        let mut dx = (view_w - total_w) * 0.5;
        for p in 0..pages {
            let dw = if p == current_page { 16.0 } else { 5.5 };
            if pt.0 >= dx - 4.0 && pt.0 <= dx + dw + 4.0 {
                return Some(SwitchHit::Page(p));
            }
            dx += dw + 6.5;
        }
    }
    None
}

unsafe fn draw_switcher(app: &App, view: id, b: NSRect) {
    let dark = is_dark_view(view);
    let (ar, ag, ab) = accent_rgb();
    let label_c: id = msg_send![class!(NSColor), labelColor];
    let secondary_c: id = msg_send![class!(NSColor), secondaryLabelColor];
    let tertiary_c: id = msg_send![class!(NSColor), tertiaryLabelColor];

    // Outer subtle specular rim around the Liquid Glass panel.
    let rim_c = if dark {
        color(1.0, 1.0, 1.0, 0.22)
    } else {
        color(1.0, 1.0, 1.0, 0.65)
    };
    outline(
        rect(0.75, 0.75, b.size.width - 1.5, b.size.height - 1.5),
        rim_c,
        27.25,
        1.0,
    );

    if app.windows.is_empty() {
        return;
    }

    // Header: App icon + Semibold app name + perfectly centered count pill + subtle shortcut hint.
    let heading = app
        .windows
        .first()
        .map(|w| w.app.as_str())
        .unwrap_or("当前软件");
    let header_mid_y = b.size.height - SWITCHER_HEADER_H * 0.5;
    if let Some((_, icon_img)) = app.app_icon {
        draw_cached_icon(icon_img, rect(20.0, header_mid_y - 12.0, 24.0, 24.0));
    }

    // Measure count pill first so app heading never overlaps it at any panel width.
    let count_str = format!("{} / {}", app.selected + 1, app.windows.len());
    let count_font = mono_digit_font(11.5, 0.15);
    let count_sz = measure_text_size(&count_str, count_font);
    let pill_w = (count_sz.width.ceil() + 16.0).max(38.0);
    let pill_h = 20.0;

    let heading_font = font_weight(15.0, 0.23); // Semibold
    let max_heading_w = (b.size.width - 116.0 - pill_w).max(48.0);
    let heading_w = measure_text(heading, heading_font).min(max_heading_w);
    draw_text_left(
        heading,
        52.0,
        header_mid_y,
        heading_w + 2.0,
        heading_font,
        label_c,
    );

    let pill_x = (52.0 + heading_w + 10.0).round();
    let pill_r = rect(
        pill_x,
        (header_mid_y - pill_h * 0.5).round(),
        pill_w,
        pill_h,
    );
    let pill_bg = if dark {
        color(1.0, 1.0, 1.0, 0.11)
    } else {
        color(0.0, 0.0, 0.0, 0.065)
    };
    fill(pill_r, pill_bg, pill_h * 0.5);
    draw_text_centered(&count_str, pill_r, count_font, secondary_c);

    // Subtle shortcut hint on the right side (only when there is clean breathing room).
    let hold_label = app.controls.config.lock().unwrap().switch.label();
    let hint = format!("{hold_label} 切换 · 鼠标可点选");
    let hint_font = font_weight(11.0, 0.0);
    let hint_sz = measure_text_size(&hint, hint_font);
    let hint_w = hint_sz.width.ceil();
    let hint_x = b.size.width - 46.0 - hint_w;
    if hint_x >= pill_x + pill_w + 18.0 {
        draw_text_centered(
            &hint,
            rect(hint_x, (header_mid_y - 9.0).round(), hint_w, 18.0),
            hint_font,
            tertiary_c,
        );
    }

    let capacity = app.grid.0 * app.grid.1;
    let start = app.selected / capacity * capacity;
    let has_capture_perm = CGPreflightScreenCaptureAccess();

    // Draw unselected card wells first, then the spring-gliding selection highlight behind card contents.
    for (position, _) in app.windows.iter().skip(start).take(capacity).enumerate() {
        let cf = card_frame_in_switcher(position, app.grid.0, b.size.height);
        let card_r = rect(cf.x, cf.y, cf.w, cf.h);
        let base_bg = if dark {
            color(1.0, 1.0, 1.0, 0.055)
        } else {
            color(0.0, 0.0, 0.0, 0.032)
        };
        let base_border = if dark {
            color(1.0, 1.0, 1.0, 0.10)
        } else {
            color(0.0, 0.0, 0.0, 0.07)
        };
        fill(card_r, base_bg, 16.0);
        outline(
            rect(cf.x + 0.5, cf.y + 0.5, cf.w - 1.0, cf.h - 1.0),
            base_border,
            15.5,
            0.75,
        );
    }

    // Animated spring-gliding selection highlight.
    if app.switcher_has_sel {
        let sr = app.switcher_sel_rect.current();
        let sel_r = rect(sr.x, sr.y, sr.w, sr.h);
        // Soft luminous accent fill
        fill(
            sel_r,
            color(ar, ag, ab, if dark { 0.18 } else { 0.14 }),
            16.0,
        );
        // Inner specular glass highlight
        outline(
            rect(sr.x + 1.5, sr.y + 1.5, sr.w - 3.0, sr.h - 3.0),
            color(1.0, 1.0, 1.0, if dark { 0.24 } else { 0.55 }),
            14.5,
            0.75,
        );
        // Crisp luminous accent rim
        outline(
            rect(sr.x + 0.85, sr.y + 0.85, sr.w - 1.7, sr.h - 1.7),
            color(ar, ag, ab, if dark { 0.86 } else { 0.78 }),
            15.15,
            1.7,
        );
    }

    // Card contents (previews + titles).
    let title_sel_font = font_weight(12.5, 0.18); // Medium
    let title_font = font_weight(12.0, 0.0);
    let badge_font = font_weight(10.5, 0.15);

    for (position, window) in app.windows.iter().skip(start).take(capacity).enumerate() {
        let index = start + position;
        let is_selected = index == app.selected;
        let cf = card_frame_in_switcher(position, app.grid.0, b.size.height);

        // Preview well: concentric 10pt radius, 8pt inset from card edges.
        let prev_x = cf.x + 8.0;
        let prev_y = cf.y + 38.0;
        let prev_w = cf.w - 16.0;
        let prev_h = cf.h - 46.0;
        let preview_r = rect(prev_x, prev_y, prev_w, prev_h);

        let well_bg = if dark {
            color(0.0, 0.0, 0.0, 0.22)
        } else {
            color(0.0, 0.0, 0.0, 0.055)
        };
        fill(preview_r, well_bg, 10.0);
        outline(
            rect(prev_x + 0.5, prev_y + 0.5, prev_w - 1.0, prev_h - 1.0),
            if dark {
                color(1.0, 1.0, 1.0, 0.07)
            } else {
                color(0.0, 0.0, 0.0, 0.06)
            },
            9.5,
            0.5,
        );

        if let Some(thumb) = app.thumbnails.get(&window.number) {
            let iw = CGImageGetWidth(thumb.image.0) as f64;
            let ih = CGImageGetHeight(thumb.image.0) as f64;
            if iw > 0.0 && ih > 0.0 {
                let pad = 4.0;
                let max_w = prev_w - pad * 2.0;
                let max_h = prev_h - pad * 2.0;
                let scale = (max_w / iw).min(max_h / ih);
                let w = (iw * scale).round();
                let h = (ih * scale).round();
                let img_r = rect(
                    prev_x + (prev_w - w) * 0.5,
                    prev_y + (prev_h - h) * 0.5,
                    w,
                    h,
                );
                draw_cgimage_rounded(thumb.image.0, img_r, 7.0, thumb.fade);
                outline(
                    rect(
                        img_r.origin.x + 0.25,
                        img_r.origin.y + 0.25,
                        w - 0.5,
                        h - 0.5,
                    ),
                    color(1.0, 1.0, 1.0, (if dark { 0.18 } else { 0.35 }) * thumb.fade),
                    6.75,
                    0.5,
                );
            }
        } else {
            // Animated subtle shimmer while thumbnail capture is in-flight.
            if has_capture_perm && !window.minimized && app.in_flight.contains(&window.number) {
                let pulse = 0.5 + 0.5 * (app.shimmer_phase * 4.2 + position as f64 * 0.7).sin();
                fill(
                    preview_r,
                    color(
                        1.0,
                        1.0,
                        1.0,
                        if dark {
                            0.03 + 0.05 * pulse
                        } else {
                            0.08 + 0.12 * pulse
                        },
                    ),
                    10.0,
                );
            }
            if let Some((_, icon_img)) = app.app_icon {
                draw_cached_icon(
                    icon_img,
                    rect(
                        prev_x + (prev_w - 42.0) * 0.5,
                        prev_y + (prev_h - 42.0) * 0.5 + 10.0,
                        42.0,
                        42.0,
                    ),
                );
            }
            let status_text = if window.minimized {
                "已最小化"
            } else if !has_capture_perm {
                "开启录屏权限查看预览"
            } else if app.in_flight.contains(&window.number) {
                "加载预览…"
            } else {
                "窗口预览"
            };
            let sw = measure_text(status_text, badge_font) + 16.0;
            let badge_r = rect(
                (prev_x + (prev_w - sw) * 0.5).round(),
                prev_y + 14.0,
                sw,
                20.0,
            );
            let badge_bg = if dark {
                color(1.0, 1.0, 1.0, 0.09)
            } else {
                color(0.0, 0.0, 0.0, 0.06)
            };
            fill(badge_r, badge_bg, 10.0);
            draw_text_centered(status_text, badge_r, badge_font, secondary_c);
        }

        // Card footer title (with subtle current-window dot for index 0).
        let title_str = if window.title.is_empty() {
            &window.app
        } else {
            &window.title
        };
        let mut tx = cf.x + 12.0;
        let mut tw = cf.w - 24.0;
        let footer_mid_y = cf.y + 19.0;
        if index == 0 {
            let dot_c = color(ar, ag, ab, if is_selected { 0.95 } else { 0.65 });
            fill(rect(tx, footer_mid_y - 3.0, 6.0, 6.0), dot_c, 3.0);
            tx += 11.0;
            tw -= 11.0;
        }
        draw_text_left(
            title_str,
            tx,
            footer_mid_y,
            tw,
            if is_selected {
                title_sel_font
            } else {
                title_font
            },
            if is_selected { label_c } else { secondary_c },
        );
    }

    // Page indicator dots when multiple pages exist.
    let pages = app.windows.len().div_ceil(capacity);
    if pages > 1 {
        let current_page = app.selected / capacity;
        let total_w = (pages as f64 - 1.0) * 12.0 + 16.0;
        let mut dx = (b.size.width - total_w) * 0.5;
        let dy = 11.0;
        for p in 0..pages {
            let active = p == current_page;
            let dw = if active { 16.0 } else { 5.5 };
            let dc = if active {
                color(ar, ag, ab, 0.92)
            } else if dark {
                color(1.0, 1.0, 1.0, 0.25)
            } else {
                color(0.0, 0.0, 0.0, 0.20)
            };
            fill(rect(dx, dy, dw, 5.5), dc, 2.75);
            dx += dw + 6.5;
        }
    }
}

unsafe fn draw_radial(app: &App, view: id) {
    let dark = is_dark_view(view);
    let (ar, ag, ab) = accent_rgb();
    let s = app.radial_scale.value.clamp(0.5, 1.15);
    let center = NSPoint::new(RADIAL_CENTER, RADIAL_CENTER);
    let outer = RADIAL_OUTER_R * s;
    let inner = RADIAL_INNER_R * s;
    let mid_r = (outer + inner) * 0.5;

    // Subtle frosted ring base tint for contrast over any wallpaper.
    let ring_base: id = msg_send![class!(NSBezierPath), bezierPath];
    let _: () = msg_send![ring_base, appendBezierPathWithArcWithCenter:center radius:outer startAngle:0.0f64 endAngle:360.0f64];
    let _: () = msg_send![ring_base, appendBezierPathWithArcWithCenter:center radius:inner startAngle:360.0f64 endAngle:0.0f64 clockwise:YES];
    let _: () = msg_send![ring_base, closePath];
    let base_fill = if dark {
        color(0.08, 0.10, 0.14, 0.28)
    } else {
        color(1.0, 1.0, 1.0, 0.24)
    };
    let _: () = msg_send![base_fill, set];
    let _: () = msg_send![ring_base, fill];

    // 8 sectors with smooth per-sector spring glow + miniature layout glyphs.
    for (i, layout) in Layout::DIRECTIONS.iter().enumerate() {
        let a_deg = -(i as f64) * 45.0;
        let glow = app.radial_sector_glow[i].value.clamp(0.0, 1.0);

        if glow > 0.008 {
            let wedge: id = msg_send![class!(NSBezierPath), bezierPath];
            let _: () = msg_send![wedge, appendBezierPathWithArcWithCenter:center radius:outer - 0.5 startAngle:a_deg - 22.0 endAngle:a_deg + 22.0];
            let _: () = msg_send![wedge, appendBezierPathWithArcWithCenter:center radius:inner + 0.5 startAngle:a_deg + 22.0 endAngle:a_deg - 22.0 clockwise:YES];
            let _: () = msg_send![wedge, closePath];
            let fill_c = color(ar, ag, ab, 0.44 * glow);
            let _: () = msg_send![fill_c, set];
            let _: () = msg_send![wedge, fill];

            // Outer luminous accent arc highlight.
            let arc: id = msg_send![class!(NSBezierPath), bezierPath];
            let _: () = msg_send![arc, appendBezierPathWithArcWithCenter:center radius:outer - 1.5 startAngle:a_deg - 19.5 endAngle:a_deg + 19.5];
            let _: () = msg_send![arc, setLineWidth:2.6f64 * s];
            let _: () = msg_send![arc, setLineCapStyle:1usize]; // NSRoundLineCapStyle
            let hi = color((ar + 0.38).min(1.0), (ag + 0.30).min(1.0), 1.0, 0.92 * glow);
            let _: () = msg_send![hi, set];
            let _: () = msg_send![arc, stroke];
        }

        // Delicate divider tick between sectors (at a_deg + 22.5).
        let div_rad = (a_deg + 22.5).to_radians();
        let tick: id = msg_send![class!(NSBezierPath), bezierPath];
        let _: () = msg_send![tick, moveToPoint:NSPoint::new(
            RADIAL_CENTER + div_rad.cos() * (inner + 3.0 * s),
            RADIAL_CENTER + div_rad.sin() * (inner + 3.0 * s),
        )];
        let _: () = msg_send![tick, lineToPoint:NSPoint::new(
            RADIAL_CENTER + div_rad.cos() * (outer - 3.0 * s),
            RADIAL_CENTER + div_rad.sin() * (outer - 3.0 * s),
        )];
        let _: () = msg_send![tick, setLineWidth:0.75f64];
        let tick_c = if dark {
            color(1.0, 1.0, 1.0, 0.16)
        } else {
            color(0.0, 0.0, 0.0, 0.12)
        };
        let _: () = msg_send![tick_c, set];
        let _: () = msg_send![tick, stroke];

        // Miniature geometric layout icon inside each sector.
        let rad = a_deg.to_radians();
        let gx = RADIAL_CENTER + rad.cos() * mid_r;
        let gy = RADIAL_CENTER + rad.sin() * mid_r;
        let gw = 14.5 * s;
        let gh = 10.5 * s;
        let icon_alpha = 0.42 + 0.56 * glow;
        let frame_c = if dark || glow > 0.35 {
            color(1.0, 1.0, 1.0, icon_alpha)
        } else {
            color(0.12, 0.14, 0.18, icon_alpha)
        };
        let fill_c = if glow > 0.25 {
            color(1.0, 1.0, 1.0, 0.94 * glow + 0.35 * (1.0 - glow))
        } else if dark {
            color(1.0, 1.0, 1.0, 0.34)
        } else {
            color(0.12, 0.14, 0.18, 0.34)
        };
        draw_layout_glyph(
            *layout,
            rect(gx - gw * 0.5, gy - gh * 0.5, gw, gh),
            frame_c,
            fill_c,
        );
    }

    // Crisp outer and inner Liquid Glass specular rims.
    outline(
        rect(
            RADIAL_CENTER - outer,
            RADIAL_CENTER - outer,
            outer * 2.0,
            outer * 2.0,
        ),
        color(1.0, 1.0, 1.0, if dark { 0.44 } else { 0.72 }),
        outer,
        1.1,
    );
    outline(
        rect(
            RADIAL_CENTER - inner,
            RADIAL_CENTER - inner,
            inner * 2.0,
            inner * 2.0,
        ),
        color(1.0, 1.0, 1.0, if dark { 0.30 } else { 0.52 }),
        inner,
        1.0,
    );

    // Smooth cursor-angle pointer bead gliding along the inner bezel when pointing outward.
    let show_pointer = app
        .radial
        .as_ref()
        .is_some_and(|r| r.has_angle && r.cursor_dist >= 16.0)
        || (app.demo && !app.switching);
    if show_pointer {
        // Screen/event dy is top-down; Cocoa NSView y is bottom-up -> negate angle.
        let cocoa_angle = -app.radial_angle.value;
        let px = RADIAL_CENTER + cocoa_angle.cos() * inner;
        let py = RADIAL_CENTER + cocoa_angle.sin() * inner;
        fill(
            rect(px - 4.5 * s, py - 4.5 * s, 9.0 * s, 9.0 * s),
            color(ar, ag, ab, 0.45),
            4.5 * s,
        );
        fill(
            rect(px - 2.75 * s, py - 2.75 * s, 5.5 * s, 5.5 * s),
            color(1.0, 1.0, 1.0, 0.96),
            2.75 * s,
        );
    }

    // Center hub: Maximize bloom OR subtle arming progress reticle.
    let cglow = app.radial_center_glow.value.clamp(0.0, 1.0);
    if cglow > 0.01 {
        let hub_r = (26.0 + 3.0 * cglow) * s;
        fill(
            rect(
                RADIAL_CENTER - hub_r,
                RADIAL_CENTER - hub_r,
                hub_r * 2.0,
                hub_r * 2.0,
            ),
            color(ar, ag, ab, 0.36 * cglow),
            hub_r,
        );
        outline(
            rect(
                RADIAL_CENTER - hub_r,
                RADIAL_CENTER - hub_r,
                hub_r * 2.0,
                hub_r * 2.0,
            ),
            color(1.0, 1.0, 1.0, 0.72 * cglow),
            hub_r,
            1.2,
        );
        let gw = 17.0 * s;
        let gh = 12.5 * s;
        draw_layout_glyph(
            Layout::Maximize,
            rect(RADIAL_CENTER - gw * 0.5, RADIAL_CENTER - gh * 0.5, gw, gh),
            color(1.0, 1.0, 1.0, 0.96 * cglow),
            color(1.0, 1.0, 1.0, 0.85 * cglow),
        );
    } else {
        // Subtle anchor dot + arming progress ring if holding in dead zone.
        fill(
            rect(
                RADIAL_CENTER - 3.0 * s,
                RADIAL_CENTER - 3.0 * s,
                6.0 * s,
                6.0 * s,
            ),
            color(1.0, 1.0, 1.0, if dark { 0.55 } else { 0.75 }),
            3.0 * s,
        );
        if let Some(r) = &app.radial {
            if !r.armed && r.cursor_dist < 28.0 {
                let prog = (r.started.elapsed().as_secs_f64() / 0.5).clamp(0.0, 1.0);
                if prog > 0.08 {
                    let arc: id = msg_send![class!(NSBezierPath), bezierPath];
                    let _: () = msg_send![arc, appendBezierPathWithArcWithCenter:center radius:14.0 * s startAngle:90.0f64 endAngle:90.0 - prog * 360.0 clockwise:YES];
                    let _: () = msg_send![arc, setLineWidth:1.8f64];
                    let _: () = msg_send![arc, setLineCapStyle:1usize];
                    let pc = color(ar, ag, ab, 0.85);
                    let _: () = msg_send![pc, set];
                    let _: () = msg_send![arc, stroke];
                }
            }
        }
    }
}

unsafe fn draw_preview(app: &App, view: id, b: NSRect) {
    let dark = is_dark_view(view);
    let (ar, ag, ab) = accent_rgb();

    // Soft Liquid Glass tint wash.
    fill(b, color(ar, ag, ab, if dark { 0.11 } else { 0.09 }), 18.0);

    // Inner specular hairline + outer luminous accent rim.
    outline(
        rect(2.5, 2.5, b.size.width - 5.0, b.size.height - 5.0),
        color(1.0, 1.0, 1.0, if dark { 0.24 } else { 0.55 }),
        15.5,
        0.75,
    );
    outline(
        rect(1.0, 1.0, b.size.width - 2.0, b.size.height - 2.0),
        color(ar, ag, ab, if dark { 0.78 } else { 0.68 }),
        17.0,
        1.8,
    );

    // Sleek centered floating layout pill.
    if let Some(layout) = app.preview_layout {
        if b.size.width >= 160.0 && b.size.height >= 80.0 {
            let label = layout.label();
            let font = font_weight(13.0, 0.23);
            let tw = measure_text(label, font);
            let pill_w = (tw + 52.0).max(108.0);
            let pill_h = 36.0;
            let px = (b.size.width - pill_w) * 0.5;
            let py = (b.size.height - pill_h) * 0.5;
            let pill_bg = if dark {
                color(0.08, 0.10, 0.14, 0.62)
            } else {
                color(1.0, 1.0, 1.0, 0.76)
            };
            fill(rect(px, py, pill_w, pill_h), pill_bg, pill_h * 0.5);
            outline(
                rect(px + 0.5, py + 0.5, pill_w - 1.0, pill_h - 1.0),
                if dark {
                    color(1.0, 1.0, 1.0, 0.24)
                } else {
                    color(0.0, 0.0, 0.0, 0.10)
                },
                (pill_h - 1.0) * 0.5,
                0.75,
            );
            let fg = if dark {
                color(1.0, 1.0, 1.0, 0.94)
            } else {
                color(0.10, 0.12, 0.16, 0.92)
            };
            draw_layout_glyph(
                layout,
                rect(px + 14.0, py + (pill_h - 13.0) * 0.5, 18.0, 13.0),
                fg,
                color(ar, ag, ab, 0.92),
            );
            draw_text_left(label, px + 38.0, py + pill_h * 0.5, tw + 4.0, font, fg);
        }
    }
}

unsafe fn draw_toast(app: &App, view: id, b: NSRect) {
    let dark = is_dark_view(view);
    let (ar, ag, ab) = accent_rgb();
    let label_c: id = msg_send![class!(NSColor), labelColor];

    outline(
        rect(0.75, 0.75, b.size.width - 1.5, b.size.height - 1.5),
        color(1.0, 1.0, 1.0, if dark { 0.24 } else { 0.60 }),
        (b.size.height - 1.5) * 0.5,
        1.0,
    );
    // Subtle accent dot on the left.
    fill(
        rect(18.0, (b.size.height - 8.0) * 0.5, 8.0, 8.0),
        color(ar, ag, ab, 0.92),
        4.0,
    );
    let font = font_weight(13.5, 0.18);
    draw_text_left(
        &app.message,
        34.0,
        b.size.height * 0.5,
        b.size.width - 50.0,
        font,
        label_c,
    );
}

extern "C" fn accepts_first_mouse(_: &Object, _: Sel, _: id) -> BOOL {
    YES
}

extern "C" fn mouse_down(view: &Object, _: Sel, event: id) {
    unsafe {
        APP.with(|cell| {
            let Ok(mut app) = cell.try_borrow_mut() else {
                return;
            };
            let Some(app) = app.as_mut() else { return };
            let view_ptr = view as *const Object as id;
            if !std::ptr::eq(view_ptr, app.switcher_view) || !app.switching || app.hooked {
                return;
            }
            let loc: NSPoint = msg_send![event, locationInWindow];
            let pt: NSPoint = msg_send![view, convertPoint:loc fromView:nil];
            app.click_switcher_local(pt.x, pt.y);
        });
    }
}

extern "C" fn draw(view: &Object, _: Sel, _: NSRect) {
    unsafe {
        APP.with(|cell| {
            let Ok(app) = cell.try_borrow() else { return };
            let Some(app) = app.as_ref() else { return };
            let b: NSRect = msg_send![view, bounds];
            let view_ptr = view as *const Object as id;
            if std::ptr::eq(view_ptr, app.preview_view) {
                draw_preview(app, view_ptr, b);
            } else if std::ptr::eq(view_ptr, app.radial_view) {
                draw_radial(app, view_ptr);
            } else if std::ptr::eq(view_ptr, app.toast_view) {
                draw_toast(app, view_ptr, b);
            } else {
                draw_switcher(app, view_ptr, b);
            }
        });
    }
}

extern "C" fn wake_perform(_: *mut c_void) {
    unsafe {
        APP.with(|cell| {
            let mut app = cell.borrow_mut();
            let Some(app) = app.as_mut() else { return };
            let pool = NSAutoreleasePool::new(nil);
            app.drain_commands();
            pool.drain();
        });
    }
}

extern "C" fn anim_tick(_: &Object, _: Sel, _: id) {
    unsafe {
        APP.with(|cell| {
            let mut app = cell.borrow_mut();
            let Some(app) = app.as_mut() else { return };
            let pool = NSAutoreleasePool::new(nil);
            app.drain_commands();
            app.step_animations();
            pool.drain();
        });
    }
}

extern "C" fn housekeep(_: &Object, _: Sel, _: id) {
    unsafe {
        APP.with(|cell| {
            let mut app = cell.borrow_mut();
            let Some(app) = app.as_mut() else { return };
            let pool = NSAutoreleasePool::new(nil);
            app.housekeep();
            pool.drain();
        });
    }
}

extern "C" fn workspace_activated(_: &Object, _: Sel, _: id) {
    unsafe {
        APP.with(|cell| {
            let mut app = cell.borrow_mut();
            let Some(app) = app.as_mut() else { return };
            if !app.switching && app.radial.is_none() && !app.demo && AXIsProcessTrusted() {
                app.record_focused_mru();
            }
        });
    }
}

extern "C" fn action(_: &Object, _: Sel, sender: id) {
    unsafe {
        APP.with(|cell| {
            let mut app = cell.borrow_mut();
            let Some(app) = app.as_mut() else { return };
            let tag: isize = msg_send![sender, tag];
            match tag {
                1 => open_url(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                ),
                2 => open_url(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
                ),
                3 | 4 => {
                    let mut c = *app.controls.config.lock().unwrap();
                    c.button = tag as u32;
                    app.save_config(c);
                }
                5 | 7 | 8 => {
                    if !app.hooked {
                        app.toast("请先允许辅助功能和输入监控权限，再录制按键");
                        return;
                    }
                    let mode = match tag {
                        7 => 1,
                        8 => 2,
                        _ => 3,
                    };
                    app.controls.recording.store(mode, Ordering::Relaxed);
                    app.hide_all();
                    app.toast(match mode {
                        1 => "按下新的窗口切换组合键（⌃ / ⌥ / ⌘ + 一个键）；Esc 取消",
                        2 => "按下圆盘触发键或组合键；单独修饰键请按下后松开；Esc 取消",
                        _ => "请按一次希望使用的鼠标侧键；Esc 取消",
                    });
                }
                9 => {
                    let mut c = *app.controls.config.lock().unwrap();
                    c.radial = None;
                    app.save_config(c);
                }
                10 => {
                    app.save_config(Config::default());
                }
                11 => {
                    CGRequestScreenCaptureAccess();
                    open_url(
                        "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
                    );
                }
                12 => {
                    let controller: id = msg_send![app.help_popover, contentViewController];
                    let label: id = msg_send![controller, view];
                    let hint: id = msg_send![sender, toolTip];
                    let _: () = msg_send![label, setStringValue:hint];
                    let bounds: NSRect = msg_send![sender, bounds];
                    let _: () = msg_send![
                        app.help_popover,
                        showRelativeToRect:bounds
                        ofView:sender
                        preferredEdge:1usize
                    ];
                }
                6 => {
                    let application: id = msg_send![class!(NSApplication), sharedApplication];
                    let _: () = msg_send![application, terminate:nil];
                }
                _ => (),
            }
        });
    }
}

unsafe fn open_url(s: &str) {
    let url: id = msg_send![class!(NSURL), URLWithString:ns(s)];
    let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
    let _: bool = msg_send![workspace, openURL:url];
}

unsafe fn make_base_panel(w: f64, h: f64, level: isize, shadow: bool) -> id {
    let p: id = msg_send![class!(NSPanel), alloc];
    let p: id = msg_send![
        p,
        initWithContentRect:rect(0., 0., w, h)
        styleMask:128usize // NSWindowStyleMaskNonactivatingPanel
        backing:2usize
        defer:NO
    ];
    let _: () = msg_send![p, setOpaque:NO];
    let clear: id = msg_send![class!(NSColor), clearColor];
    let _: () = msg_send![p, setBackgroundColor:clear];
    let _: () = msg_send![p, setLevel:level];
    let _: () = msg_send![p, setCollectionBehavior:257usize];
    let _: () = msg_send![p, setIgnoresMouseEvents:YES];
    let _: () = msg_send![p, setHidesOnDeactivate:NO];
    let _: () = msg_send![p, setReleasedWhenClosed:NO];
    let _: () = msg_send![p, setHasShadow:if shadow { YES } else { NO }];
    p
}

unsafe fn make_glass_material(frame: NSRect, radius: f64, clear_style: bool) -> (id, bool) {
    if let Some(glass_class) = Class::get("NSGlassEffectView") {
        let allocated: id = msg_send![glass_class, alloc];
        let material: id = msg_send![allocated, initWithFrame:frame];
        let style: isize = if clear_style { 1 } else { 0 };
        let _: () = msg_send![material, setStyle:style];
        let _: () = msg_send![material, setCornerRadius:radius];
        if clear_style {
            let (ar, ag, ab) = accent_rgb();
            let tint = color(ar, ag, ab, 0.14);
            let _: () = msg_send![material, setTintColor:tint];
        }
        (material, true)
    } else {
        let allocated: id = msg_send![class!(NSVisualEffectView), alloc];
        let material: id = msg_send![allocated, initWithFrame:frame];
        let _: () = msg_send![material, setMaterial:13isize]; // HUDWindow
        let _: () = msg_send![material, setBlendingMode:0isize]; // behindWindow
        let _: () = msg_send![material, setState:1isize]; // active
        let _: () = msg_send![material, setWantsLayer:YES];
        let layer: id = msg_send![material, layer];
        let _: () = msg_send![layer, setCornerRadius:radius];
        let _: () = msg_send![layer, setMasksToBounds:YES];
        (material, false)
    }
}

unsafe fn make_glass_panel(
    view_class: &Class,
    w: f64,
    h: f64,
    radius: f64,
    level: isize,
    clear_style: bool,
    shadow: bool,
) -> (id, id) {
    let p = make_base_panel(w, h, level, shadow);
    let frame = rect(0., 0., w, h);
    let view: id = msg_send![view_class, alloc];
    let view: id = msg_send![view, initWithFrame:frame];
    let _: () = msg_send![view, setAutoresizingMask:18usize];
    let (material, is_glass) = make_glass_material(frame, radius, clear_style);
    if is_glass {
        let _: () = msg_send![material, setContentView:view];
    } else {
        let _: () = msg_send![material, addSubview:view];
    }
    let _: () = msg_send![p, setContentView:material];
    let _: () = msg_send![material, release];
    (p, view)
}

unsafe fn make_radial_panel(view_class: &Class) -> (id, id, id) {
    let size = RADIAL_PANEL_SIZE;
    let frame = rect(0., 0., size, size);
    let p = make_base_panel(size, size, 26, true);

    let root: id = msg_send![class!(NSView), alloc];
    let root: id = msg_send![root, initWithFrame:frame];

    // Frosted/Liquid Glass donut behind the unclipped vector HUD overlay.
    let d = RADIAL_OUTER_R * 2.0;
    let glass_frame = rect(
        RADIAL_CENTER - RADIAL_OUTER_R,
        RADIAL_CENTER - RADIAL_OUTER_R,
        d,
        d,
    );
    let (glass, _) = make_glass_material(glass_frame, RADIAL_OUTER_R, false);
    set_donut_mask(glass, 1.0);
    let _: () = msg_send![root, addSubview:glass];

    let view: id = msg_send![view_class, alloc];
    let view: id = msg_send![view, initWithFrame:frame];
    let _: () = msg_send![view, setAutoresizingMask:18usize];
    let _: () = msg_send![root, addSubview:view];

    let _: () = msg_send![p, setContentView:root];
    let _: () = msg_send![root, release];
    (p, glass, view)
}

impl App {
    unsafe fn drain_commands(&mut self) {
        let commands: Vec<_> = self.rx.try_iter().collect();
        if commands.is_empty() {
            return;
        }
        // Coalesce high-frequency mouse motion so high-polling mice never delay releases/clicks.
        let mut motion = None;
        for command in commands {
            if matches!(command, Command::RadialMove(..) | Command::SwitchHover(..)) {
                motion = Some(command);
                continue;
            }
            if let Some(m) = motion.take() {
                self.command(m);
            }
            self.command(command);
        }
        if let Some(m) = motion {
            self.command(m);
        }
    }

    unsafe fn needs_animation(&self) -> bool {
        if self.demo {
            return false;
        }
        if self.switcher_alpha.is_active(0.005)
            || (self.switching && self.switcher_sel_rect.is_active())
        {
            return true;
        }
        if self.switching {
            if !self.in_flight.is_empty() && CGPreflightScreenCaptureAccess() {
                return true;
            }
            if self.thumbnails.values().any(|t| t.fade < 0.995) {
                return true;
            }
        }
        if self.radial.is_some()
            || self.radial_alpha.is_active(0.005)
            || self.radial_scale.is_active(0.003)
            || self.radial_center_glow.is_active(0.005)
            || self.radial_sector_glow.iter().any(|s| s.is_active(0.005))
        {
            return true;
        }
        if self.preview_alpha.is_active(0.005)
            || (self.preview_visible && self.preview_rect.is_active())
        {
            return true;
        }
        if self.toast_alpha.is_active(0.005) {
            return true;
        }
        false
    }

    unsafe fn ensure_anim_timer(&mut self) {
        if self.instant_mode || self.demo {
            return;
        }
        if self.anim_timer == nil && self.needs_animation() {
            let interval = 1.0 / max_refresh_rate();
            self.last_anim = Instant::now();
            let timer: id = msg_send![
                class!(NSTimer),
                timerWithTimeInterval:interval
                target:self.target
                selector:sel!(animTick:)
                userInfo:nil
                repeats:YES
            ];
            let _: () = msg_send![timer, setTolerance:0.0f64];
            let runloop: id = msg_send![class!(NSRunLoop), mainRunLoop];
            let _: () = msg_send![runloop, addTimer:timer forMode:ns("kCFRunLoopCommonModes")];
            self.anim_timer = timer;
        }
    }

    unsafe fn stop_anim_timer(&mut self) {
        if self.anim_timer != nil {
            let _: () = msg_send![self.anim_timer, invalidate];
            self.anim_timer = nil;
        }
    }

    unsafe fn step_animations(&mut self) {
        let now = Instant::now();
        let dt = now
            .duration_since(self.last_anim)
            .as_secs_f64()
            .clamp(1.0 / 240.0, 1.0 / 30.0);
        self.last_anim = now;

        // 1. Switcher animations
        if self.switcher_visible {
            let alpha_moved = self.switcher_alpha.step(dt, SPRING_POP, 0.005);
            if alpha_moved || (self.switcher_alpha.target - self.switcher_alpha.value).abs() < 0.01
            {
                let a = self.switcher_alpha.value.clamp(0.0, 1.0);
                let _: () = msg_send![self.switcher_panel, setAlphaValue:a];
                if !self.switching && a <= 0.01 {
                    let _: () = msg_send![self.switcher_panel, orderOut:nil];
                    self.switcher_visible = false;
                }
            }
            let mut redraw_switcher = false;
            if self.switching && self.switcher_sel_rect.step(dt, SPRING_SNAPPY) {
                redraw_switcher = true;
            }
            if self.switching {
                if !self.in_flight.is_empty() && CGPreflightScreenCaptureAccess() {
                    self.shimmer_phase += dt;
                    redraw_switcher = true;
                }
                for thumb in self.thumbnails.values_mut() {
                    if thumb.fade < 1.0 {
                        thumb.fade = (thumb.fade + dt * 7.5).min(1.0);
                        redraw_switcher = true;
                    }
                }
            }
            if redraw_switcher {
                let _: () = msg_send![self.switcher_view, setNeedsDisplay:YES];
            }
        }

        // 2. Radial auto-arm timer (holding still at center for 500ms blooms Maximize automatically)
        let mut auto_armed = false;
        if let Some(r) = self.radial.as_mut() {
            if !r.armed && r.started.elapsed() >= Duration::from_millis(500) {
                r.armed = true;
                auto_armed = true;
            }
        }
        if auto_armed {
            let origin = self.radial.as_ref().unwrap().origin;
            self.update_radial_selection(origin.0, origin.1);
        }

        // 3. Radial animations
        if self.radial_visible {
            let mut redraw_radial = false;
            if self.radial_alpha.step(dt, SPRING_POP, 0.005) {
                let a = self.radial_alpha.value.clamp(0.0, 1.0);
                let _: () = msg_send![self.radial_panel, setAlphaValue:a];
            } else if self.radial.is_none() && self.radial_alpha.value <= 0.01 {
                let _: () = msg_send![self.radial_panel, orderOut:nil];
                self.radial_visible = false;
            }
            if self.radial_scale.step(dt, SPRING_POP, 0.003) {
                let s = self.radial_scale.value.clamp(0.5, 1.15);
                if (s - self.radial_last_mask_scale).abs() > 0.004 {
                    set_donut_mask(self.radial_glass, s);
                    self.radial_last_mask_scale = s;
                }
                redraw_radial = true;
            }
            if self.radial_angle.step(dt, SPRING_SNAPPY) {
                redraw_radial = true;
            }
            for glow in &mut self.radial_sector_glow {
                if glow.step(dt, SPRING_GLOW, 0.005) {
                    redraw_radial = true;
                }
            }
            if self.radial_center_glow.step(dt, SPRING_GLOW, 0.005) {
                redraw_radial = true;
            }
            if let Some(r) = &self.radial {
                if !r.armed && r.cursor_dist < 28.0 {
                    // Animate the subtle arming progress arc.
                    redraw_radial = true;
                }
            }
            if redraw_radial && self.radial_visible {
                let _: () = msg_send![self.radial_view, setNeedsDisplay:YES];
            }
        }

        // 4. Target preview animations
        if self.preview_visible {
            let alpha_changed = self.preview_alpha.step(dt, SPRING_POP, 0.005);
            let rect_changed = self.preview_rect.step(dt, SPRING_FLUID);
            let a = self.preview_alpha.value.clamp(0.0, 1.0);
            if alpha_changed {
                let _: () = msg_send![self.preview_panel, setAlphaValue:a];
            }
            if rect_changed {
                let cur = self.preview_rect.current();
                let ph = primary_height();
                let _: () = msg_send![
                    self.preview_panel,
                    setFrame:rect(cur.x, ph - cur.y - cur.h, cur.w, cur.h)
                    display:YES
                ];
                let _: () = msg_send![self.preview_view, setNeedsDisplay:YES];
            }
            if self.preview_alpha.target <= 0.01 && a <= 0.01 {
                let _: () = msg_send![self.preview_panel, orderOut:nil];
                self.preview_visible = false;
            }
        }

        // 5. Toast fade animations
        if self.toast_visible {
            if now > self.message_until && self.toast_alpha.target > 0.0 {
                self.toast_alpha.set_target(0.0);
            }
            if self.toast_alpha.step(dt, SPRING_POP, 0.005) {
                let a = self.toast_alpha.value.clamp(0.0, 1.0);
                let _: () = msg_send![self.toast_panel, setAlphaValue:a];
            } else if self.toast_alpha.target <= 0.01 && self.toast_alpha.value <= 0.01 {
                let _: () = msg_send![self.toast_panel, orderOut:nil];
                self.toast_visible = false;
            }
        }

        if !self.needs_animation() {
            self.stop_anim_timer();
        }
    }

    unsafe fn housekeep(&mut self) {
        if self.demo {
            return;
        }
        if !self.hooked && AXIsProcessTrusted() && CGPreflightListenEventAccess() {
            match input::start(self.tx.clone(), self.controls.clone()) {
                Ok(()) => {
                    self.hooked = true;
                    eprintln!("RingSwitch: input tap ready");
                    self.update_menu();
                }
                Err(e) => eprintln!("{e}"),
            }
        }
        if !self.switching && self.radial.is_none() && AXIsProcessTrusted() {
            self.record_focused_mru();
        }
        if self.toast_visible && Instant::now() > self.message_until {
            self.toast_alpha.set_target(0.0);
            self.ensure_anim_timer();
        }
    }

    unsafe fn record_focused_mru(&mut self) {
        if let Some(focused) = focused() {
            if self.mru.first() != Some(&focused) {
                self.mru.retain(|w| w != &focused);
                self.mru.insert(0, focused);
                self.mru.truncate(100);
            }
        }
    }

    unsafe fn refresh_app_icon(&mut self, pid: i32) {
        if self
            .app_icon
            .is_some_and(|(cached_pid, _)| cached_pid == pid)
        {
            return;
        }
        if let Some((_, old)) = self.app_icon.take() {
            if old != nil {
                let _: () = msg_send![old, release];
            }
        }
        let app: id = msg_send![
            class!(NSRunningApplication),
            runningApplicationWithProcessIdentifier:pid
        ];
        if app != nil {
            let img: id = msg_send![app, icon];
            if img != nil {
                let _: id = msg_send![img, retain];
                self.app_icon = Some((pid, img));
            }
        }
    }

    unsafe fn toast(&mut self, message: &str) {
        eprintln!("RingSwitch: {message}");
        self.message = message.into();
        self.message_until = Instant::now() + Duration::from_secs(3);
        let screen = screens().first().map(|(_, v)| *v);
        if let Some(s) = screen {
            let font = font_weight(13.5, 0.18);
            let w = (measure_text(message, font) + 56.0).clamp(220.0, 620.0);
            let h = 46.0;
            let x = s.x + (s.w - w) * 0.5;
            let y = s.y + s.h * 0.18;
            let _: () = msg_send![
                self.toast_panel,
                setFrame:rect(x, primary_height() - y - h, w, h)
                display:YES
            ];
            let _: () = msg_send![self.toast_view, setNeedsDisplay:YES];
            if !self.toast_visible {
                self.toast_alpha.snap(0.0);
                let _: () = msg_send![self.toast_panel, setAlphaValue:0.0f64];
                let _: () = msg_send![self.toast_panel, orderFrontRegardless];
                self.toast_visible = true;
            }
            self.toast_alpha.set_target(1.0);
            self.ensure_anim_timer();
        }
    }

    unsafe fn hide_switcher(&mut self) {
        self.switching = false;
        self.controls
            .switching_active
            .store(false, Ordering::Relaxed);
        self.switcher_open_mouse = None;
        self.switcher_mouse_armed = false;
        self.in_flight.clear();
        let _: () = msg_send![self.help_popover, close];
        let _: () = msg_send![self.switcher_panel, setIgnoresMouseEvents:YES];
        if self.instant_mode || self.demo {
            let _: () = msg_send![self.switcher_panel, orderOut:nil];
            self.switcher_visible = false;
            self.switcher_alpha.snap(0.0);
        } else if self.switcher_visible {
            self.switcher_alpha.set_target(0.0);
            self.ensure_anim_timer();
        }
    }

    unsafe fn hide_radial(&mut self) {
        self.radial = None;
        for g in &mut self.radial_sector_glow {
            g.set_target(0.0);
        }
        self.radial_center_glow.set_target(0.0);
        if self.instant_mode || self.demo {
            let _: () = msg_send![self.radial_panel, orderOut:nil];
            let _: () = msg_send![self.preview_panel, orderOut:nil];
            self.radial_visible = false;
            self.preview_visible = false;
            self.radial_alpha.snap(0.0);
            self.preview_alpha.snap(0.0);
        } else {
            if self.radial_visible {
                self.radial_alpha.set_target(0.0);
                self.radial_scale.set_target(0.90);
            }
            if self.preview_visible {
                self.preview_alpha.set_target(0.0);
            }
            self.ensure_anim_timer();
        }
    }

    unsafe fn hide_all(&mut self) {
        self.hide_switcher();
        self.hide_radial();
    }

    unsafe fn save_config(&mut self, config: Config) {
        match config.save() {
            Ok(()) => {
                *self.controls.config.lock().unwrap() = config;
                self.update_menu();
                self.toast("按键设置已保存");
            }
            Err(e) => self.toast(&e),
        }
    }

    unsafe fn update_menu(&mut self) {
        let _: () = msg_send![self.menu, removeAllItems];
        let button: id = msg_send![self.status, button];
        let _: () = msg_send![button, setTitle:ns(if self.hooked { "RS" } else { "RS !" })];
        let c = *self.controls.config.lock().unwrap();
        let ax_ok = AXIsProcessTrusted();
        let in_ok = CGPreflightListenEventAccess();
        let sc_ok = CGPreflightScreenCaptureAccess();
        let mark = |ok: bool| if ok { "✓" } else { "○" };
        let labels = [
            (format!("窗口切换：{} · 更改…", c.switch.label()), 7),
            (
                format!(
                    "圆盘键盘：{} · 更改…",
                    c.radial.map(|s| s.label()).unwrap_or("未启用".into())
                ),
                8,
            ),
            ("圆盘仅使用鼠标侧键".into(), 9),
            (format!("圆盘侧键：{}", c.button + 1), 0),
            ("侧键 4（常见后退键）".into(), 3),
            ("侧键 5（常见前进键）".into(), 4),
            ("学习鼠标侧键…".into(), 5),
            ("恢复默认按键".into(), 10),
            (format!("{} 辅助功能权限…", mark(ax_ok)), 1),
            (format!("{} 输入监控权限…", mark(in_ok)), 2),
            (format!("{} 窗口缩略图权限…", mark(sc_ok)), 11),
            ("退出 RingSwitch".into(), 6),
        ];
        for (label, tag) in labels {
            let item: id = msg_send![class!(NSMenuItem), alloc];
            let item: id = msg_send![
                item,
                initWithTitle:ns(&label)
                action:sel!(menuAction:)
                keyEquivalent:ns("")
            ];
            let _: () = msg_send![item, setTag:tag as isize];
            let _: () = msg_send![item, setTarget:self.target];
            if tag == 0 {
                let _: () = msg_send![item, setEnabled:NO];
            }
            if (tag == 3 || tag == 4) && c.button == tag as u32 || tag == 9 && c.radial.is_none() {
                let _: () = msg_send![item, setState:1isize];
            }
            let _: () = msg_send![self.menu, addItem:item];
            let _: () = msg_send![item, release];
        }
    }

    unsafe fn show_switcher(&mut self, newly_opened: bool, page_changed: bool) {
        let mouse: NSPoint = msg_send![class!(NSEvent), mouseLocation];
        let s = screen_at(&screens(), mouse.x, primary_height() - mouse.y).unwrap();
        let max_cols = (((s.w - SWITCHER_PAD_X * 2.0 + CARD_GAP_X) / (CARD_W + CARD_GAP_X))
            as usize)
            .clamp(1, 4);
        let preferred_cols = if [5, 6, 9].contains(&self.windows.len()) {
            3
        } else {
            self.windows.len().clamp(1, 4)
        };
        let cols = max_cols.min(preferred_cols);
        let max_rows = (((s.h - SWITCHER_HEADER_H - SWITCHER_PAD_BOTTOM - SWITCHER_DOTS_H)
            / (CARD_H + CARD_GAP_Y)) as usize)
            .clamp(1, 3);
        let rows = max_rows.min(self.windows.len().div_ceil(cols).max(1));
        self.grid = (cols, rows);
        let capacity = cols * rows;
        let has_pages = self.windows.len() > capacity;
        let w = SWITCHER_PAD_X * 2.0
            + cols as f64 * CARD_W
            + (cols.saturating_sub(1)) as f64 * CARD_GAP_X;
        let h = SWITCHER_HEADER_H
            + rows as f64 * CARD_H
            + (rows.saturating_sub(1)) as f64 * CARD_GAP_Y
            + SWITCHER_PAD_BOTTOM
            + if has_pages { SWITCHER_DOTS_H } else { 0.0 };

        if let Some(first) = self.windows.first() {
            self.refresh_app_icon(first.pid);
        }

        let sel_pos = self.selected % capacity;
        let target_card = card_frame_in_switcher(sel_pos, cols, h);
        if newly_opened || page_changed || !self.switcher_has_sel || self.demo || self.instant_mode
        {
            self.switcher_sel_rect.snap(target_card);
            self.switcher_has_sel = true;
        } else {
            self.switcher_sel_rect.set_target(target_card);
        }

        if newly_opened || !self.switcher_visible || self.demo {
            let _: () = msg_send![self.help, setFrame:rect(w - 36.0, h - SWITCHER_HEADER_H * 0.5 - 9.0, 18.0, 18.0)];
            let hold = self.controls.config.lock().unwrap().switch.label();
            let _: () = msg_send![
                self.help,
                setToolTip:ns(&format!("{hold} · 松开修饰键或点击卡片切换 · 鼠标悬停/方向键选择 · Esc 取消"))
            ];
            let x = s.x + (s.w - w) * 0.5;
            let y = s.y + (s.h - h) * 0.5;
            let _: () = msg_send![
                self.switcher_panel,
                setFrame:rect(x, primary_height() - y - h, w, h)
                display:YES
            ];
            let _: () = msg_send![self.switcher_panel, setIgnoresMouseEvents:NO];
            if self.demo || self.instant_mode {
                self.switcher_alpha.snap(1.0);
                let _: () = msg_send![self.switcher_panel, setAlphaValue:1.0f64];
            } else {
                if !self.switcher_visible {
                    self.switcher_alpha.snap(0.0);
                    let _: () = msg_send![self.switcher_panel, setAlphaValue:0.0f64];
                }
                self.switcher_alpha.set_target(1.0);
            }
            let _: () = msg_send![self.switcher_panel, orderFrontRegardless];
            self.switcher_visible = true;
        }
        let _: () = msg_send![self.switcher_view, setNeedsDisplay:YES];

        // Request fresh captures for current page windows not freshly cached (selected card first).
        let start = self.selected / capacity * capacity;
        let now = Instant::now();
        let mut numbers: Vec<u32> = Vec::new();
        let selected_num = self
            .windows
            .get(self.selected)
            .filter(|w| !w.minimized && w.number != 0)
            .map(|w| w.number);
        for id in selected_num.into_iter().chain(
            self.windows
                .iter()
                .skip(start)
                .take(capacity)
                .filter(|w| !w.minimized && w.number != 0)
                .map(|w| w.number),
        ) {
            let stale_or_missing = self
                .thumbnails
                .get(&id)
                .is_none_or(|t| now.duration_since(t.captured_at) >= THUMB_REFRESH_AGE);
            if stale_or_missing && self.requested.insert(id) {
                numbers.push(id);
            }
        }
        if !numbers.is_empty() {
            for id in &numbers {
                self.in_flight.insert(*id);
            }
            capture::thumbnails(numbers, self.session, self.tx.clone());
        }
        self.ensure_anim_timer();
    }

    fn prune_thumbnails(&mut self) {
        while self.thumbnails.len() > MAX_CACHED_THUMBS {
            if let Some(oldest_key) = self
                .thumbnails
                .iter()
                .min_by_key(|(_, v)| v.captured_at)
                .map(|(k, _)| *k)
            {
                self.thumbnails.remove(&oldest_key);
            } else {
                break;
            }
        }
    }

    unsafe fn commit_switch(&mut self) {
        if self.switching {
            let selected = self.windows.get(self.selected).cloned();
            self.hide_switcher();
            if let Some(w) = selected {
                match activate(&w) {
                    Ok(()) => {
                        self.mru.retain(|m| m != &w.ax);
                        self.mru.insert(0, w.ax);
                    }
                    Err(e) => self.toast(&e),
                }
            }
        }
    }

    unsafe fn hover_switcher(&mut self, screen_x: f64, screen_y: f64) {
        if !self.switching || !self.switcher_visible || self.windows.is_empty() {
            return;
        }
        if !self.switcher_mouse_armed {
            match self.switcher_open_mouse {
                Some((ox, oy)) if (screen_x - ox).hypot(screen_y - oy) < 6.0 => return,
                None => {
                    self.switcher_open_mouse = Some((screen_x, screen_y));
                    return;
                }
                _ => {
                    self.switcher_mouse_armed = true;
                }
            }
        }
        let panel_frame: NSRect = msg_send![self.switcher_panel, frame];
        let cocoa_y = primary_height() - screen_y;
        let lx = screen_x - panel_frame.origin.x;
        let ly = cocoa_y - panel_frame.origin.y;
        if let Some(SwitchHit::Card(index)) = switcher_hit_test(
            (lx, ly),
            panel_frame.size.width,
            panel_frame.size.height,
            self.grid,
            self.selected,
            self.windows.len(),
        ) {
            if index != self.selected {
                self.selected = index;
                let capacity = (self.grid.0 * self.grid.1).max(1);
                let sel_pos = self.selected % capacity;
                let target = card_frame_in_switcher(sel_pos, self.grid.0, panel_frame.size.height);
                self.switcher_sel_rect.set_target(target);
                let _: () = msg_send![self.switcher_view, setNeedsDisplay:YES];
                self.ensure_anim_timer();
            }
        }
    }

    unsafe fn click_switcher_screen(&mut self, screen_x: f64, screen_y: f64) {
        if !self.switching || !self.switcher_visible || self.windows.is_empty() {
            return;
        }
        let panel_frame: NSRect = msg_send![self.switcher_panel, frame];
        let cocoa_y = primary_height() - screen_y;
        let lx = screen_x - panel_frame.origin.x;
        let ly = cocoa_y - panel_frame.origin.y;
        if lx < 0.0 || ly < 0.0 || lx > panel_frame.size.width || ly > panel_frame.size.height {
            // Clicking outside the switcher panel dismisses it cleanly.
            self.hide_switcher();
            return;
        }
        self.click_switcher_local(lx, ly);
    }

    unsafe fn click_switcher_local(&mut self, lx: f64, ly: f64) {
        if !self.switching || self.windows.is_empty() {
            return;
        }
        let bounds: NSRect = msg_send![self.switcher_view, bounds];
        match switcher_hit_test(
            (lx, ly),
            bounds.size.width,
            bounds.size.height,
            self.grid,
            self.selected,
            self.windows.len(),
        ) {
            Some(SwitchHit::Card(index)) => {
                self.selected = index;
                if self.demo {
                    let capacity = (self.grid.0 * self.grid.1).max(1);
                    let sel_pos = self.selected % capacity;
                    let target = card_frame_in_switcher(sel_pos, self.grid.0, bounds.size.height);
                    self.switcher_sel_rect.snap(target);
                    let _: () = msg_send![self.switcher_view, setNeedsDisplay:YES];
                } else {
                    self.commit_switch();
                }
            }
            Some(SwitchHit::Page(page)) => {
                let capacity = (self.grid.0 * self.grid.1).max(1);
                let next = (page * capacity).min(self.windows.len().saturating_sub(1));
                if next != self.selected {
                    self.selected = next;
                    self.show_switcher(false, true);
                }
            }
            None => (),
        }
    }

    unsafe fn command(&mut self, command: Command) {
        match command {
            Command::Cycle(backwards) => {
                let mut newly_opened = false;
                let prev_capacity = (self.grid.0 * self.grid.1).max(1);
                let prev_page = self.selected / prev_capacity;
                if !self.switching {
                    self.hide_radial();
                    self.session = self.session.wrapping_add(1);
                    self.requested.clear();
                    self.in_flight.clear();
                    self.windows = match current_app_windows() {
                        Ok(windows) => windows,
                        Err(error) => {
                            self.toast(&error);
                            return;
                        }
                    };
                    self.windows.sort_by_key(|w| {
                        self.mru
                            .iter()
                            .position(|m| m == &w.ax)
                            .unwrap_or(usize::MAX)
                    });
                    if let Some(f) = focused() {
                        if let Some(i) = self.windows.iter().position(|w| w.ax == f) {
                            let w = self.windows.remove(i);
                            self.windows.insert(0, w);
                        }
                    }
                    if self.windows.is_empty() {
                        self.toast("没有可切换的窗口");
                        return;
                    }
                    self.selected = 0;
                    self.switching = true;
                    self.controls
                        .switching_active
                        .store(true, Ordering::Relaxed);
                    let mouse: NSPoint = msg_send![class!(NSEvent), mouseLocation];
                    self.switcher_open_mouse = Some((mouse.x, primary_height() - mouse.y));
                    self.switcher_mouse_armed = false;
                    newly_opened = true;
                }
                self.selected = cycle(self.selected, self.windows.len(), backwards);
                let new_page = self.selected / prev_capacity;
                self.show_switcher(newly_opened, !newly_opened && new_page != prev_page);
            }
            Command::Navigate(dx, dy) => {
                if !self.switching || self.windows.is_empty() {
                    return;
                }
                let cols = self.grid.0.max(1);
                let capacity = (cols * self.grid.1.max(1)).max(1);
                let prev_page = self.selected / capacity;
                let len = self.windows.len() as isize;
                let step = if dx != 0 {
                    dx as isize
                } else {
                    (dy as isize) * (cols as isize)
                };
                self.selected = (self.selected as isize + step).rem_euclid(len) as usize;
                let new_page = self.selected / capacity;
                self.show_switcher(false, new_page != prev_page);
            }
            Command::SwitchHover(x, y) => {
                self.hover_switcher(x, y);
            }
            Command::SwitchClick(x, y) => {
                self.click_switcher_screen(x, y);
            }
            Command::CommitSwitch => {
                self.commit_switch();
            }
            Command::Cancel => {
                self.controls.recording.store(0, Ordering::Relaxed);
                self.hide_all();
            }
            Command::Learned(button) => {
                let mut c = *self.controls.config.lock().unwrap();
                c.button = button;
                self.save_config(c);
            }
            Command::LearnedKey(mode, shortcut) => {
                let mut c = *self.controls.config.lock().unwrap();
                if mode == 1 {
                    c.switch = shortcut;
                } else {
                    c.radial = Some(shortcut);
                }
                self.save_config(c);
            }
            Command::Thumbnail(session, number, image) => {
                self.in_flight.remove(&number);
                if image != 0 {
                    let image = Owned(image as Ref);
                    let was_cached = self.thumbnails.contains_key(&number);
                    self.thumbnails.insert(
                        number,
                        CachedThumb {
                            image,
                            captured_at: Instant::now(),
                            fade: if was_cached { 1.0 } else { 0.0 },
                        },
                    );
                    self.prune_thumbnails();
                }
                if self.switching && session == self.session {
                    let _: () = msg_send![self.switcher_view, setNeedsDisplay:YES];
                    self.ensure_anim_timer();
                }
            }
            Command::TileError(error) => {
                self.toast(&error);
            }
            Command::RadialStart(x, y) => {
                self.hide_switcher();
                let Some(window) = focused() else {
                    self.toast("当前没有可调整的窗口");
                    return;
                };
                let Some(screen) = screen_at(&screens(), x, y) else {
                    return;
                };
                let window_rect = frame(window.ax_or_self());
                self.radial = Some(Radial {
                    window,
                    window_rect,
                    origin: (x, y),
                    screen,
                    selected: None,
                    armed: false,
                    started: Instant::now(),
                    cursor_dist: 0.0,
                    has_angle: false,
                });
                self.preview_layout = None;
                for g in &mut self.radial_sector_glow {
                    g.snap(0.0);
                }
                self.radial_center_glow.snap(0.0);

                // Keep the ring centered at the actual cursor, even near display edges.
                let half = RADIAL_PANEL_SIZE * 0.5;
                let _: () = msg_send![
                    self.radial_panel,
                    setFrame:rect(
                        x - half,
                        primary_height() - (y - half) - RADIAL_PANEL_SIZE,
                        RADIAL_PANEL_SIZE,
                        RADIAL_PANEL_SIZE
                    )
                    display:YES
                ];
                if self.instant_mode || self.demo {
                    self.radial_alpha.snap(1.0);
                    self.radial_scale.snap(1.0);
                    set_donut_mask(self.radial_glass, 1.0);
                    self.radial_last_mask_scale = 1.0;
                    let _: () = msg_send![self.radial_panel, setAlphaValue:1.0f64];
                } else {
                    self.radial_alpha.snap(0.0);
                    self.radial_alpha.set_target(1.0);
                    self.radial_scale.snap(0.82);
                    self.radial_scale.set_target(1.0);
                    set_donut_mask(self.radial_glass, 0.82);
                    self.radial_last_mask_scale = 0.82;
                    let _: () = msg_send![self.radial_panel, setAlphaValue:0.0f64];
                }
                let _: () = msg_send![self.radial_view, setNeedsDisplay:YES];
                let _: () = msg_send![self.radial_panel, orderFrontRegardless];
                self.radial_visible = true;
                self.ensure_anim_timer();
            }
            Command::RadialMove(x, y) => self.move_radial(x, y),
            Command::RadialEnd(x, y) => {
                self.move_radial(x, y);
                let radial = self.radial.take();
                self.hide_radial();
                if let Some(r) = radial {
                    if let Some(layout) = r.selected {
                        if self.instant_mode {
                            if let Err(e) = tile(r.window.0, layout, r.origin.0, r.origin.1) {
                                self.toast(&e);
                            }
                        } else {
                            let gen = self.tile_gen.fetch_add(1, Ordering::Relaxed) + 1;
                            let _ = self.tile_tx.send(TileJob {
                                window: SendOwned(r.window),
                                layout,
                                origin: r.origin,
                                gen,
                            });
                        }
                    }
                }
            }
        }
    }

    unsafe fn move_radial(&mut self, x: f64, y: f64) {
        let Some(r) = self.radial.as_mut() else {
            return;
        };
        let dx = x - r.origin.0;
        let dy = y - r.origin.1;
        let dist = dx.hypot(dy);
        r.cursor_dist = dist;
        if dist >= 10.0 {
            let angle = dy.atan2(dx);
            if !r.has_angle || self.instant_mode {
                self.radial_angle.snap(angle);
                r.has_angle = true;
            } else {
                self.radial_angle.set_target(angle);
            }
        }
        if dist >= 28.0 || r.started.elapsed() > Duration::from_millis(500) {
            r.armed = true;
        }
        self.update_radial_selection(x, y);
        let _: () = msg_send![self.radial_view, setNeedsDisplay:YES];
        self.ensure_anim_timer();
    }

    unsafe fn update_radial_selection(&mut self, x: f64, y: f64) {
        let Some(r) = self.radial.as_mut() else {
            return;
        };
        let dx = x - r.origin.0;
        let dy = y - r.origin.1;
        let selected = selection(dx, dy, r.armed);
        if selected == r.selected {
            return;
        }
        r.selected = selected;

        // Update sector & center glow targets smoothly.
        for (i, layout) in Layout::DIRECTIONS.iter().enumerate() {
            let target = if selected == Some(*layout) { 1.0 } else { 0.0 };
            if self.instant_mode {
                self.radial_sector_glow[i].snap(target);
            } else {
                self.radial_sector_glow[i].set_target(target);
            }
        }
        let center_target = if selected == Some(Layout::Maximize) {
            1.0
        } else {
            0.0
        };
        if self.instant_mode {
            self.radial_center_glow.snap(center_target);
        } else {
            self.radial_center_glow.set_target(center_target);
        }

        let screen = r.screen;
        let window_rect = r.window_rect;

        if let Some(layout) = selected {
            let target = layout.rect(screen);
            self.preview_layout = Some(layout);
            if self.instant_mode || self.demo {
                self.preview_rect.snap(target);
                self.preview_alpha.snap(1.0);
                let ph = primary_height();
                let _: () = msg_send![
                    self.preview_panel,
                    setFrame:rect(target.x, ph - target.y - target.h, target.w, target.h)
                    display:YES
                ];
                let _: () = msg_send![self.preview_panel, setAlphaValue:1.0f64];
                let _: () = msg_send![self.preview_view, setNeedsDisplay:YES];
                let _: () = msg_send![self.preview_panel, orderFrontRegardless];
                let _: () = msg_send![self.radial_panel, orderFrontRegardless];
                self.preview_visible = true;
            } else {
                if !self.preview_visible {
                    // Seed the morph smoothly from the window's actual frame (or a slightly inset target).
                    let seed = window_rect.unwrap_or(Rect {
                        x: target.x + 24.0,
                        y: target.y + 24.0,
                        w: (target.w - 48.0).max(80.0),
                        h: (target.h - 48.0).max(60.0),
                    });
                    self.preview_rect.snap(seed);
                    self.preview_rect.set_target(target);
                    self.preview_alpha.snap(0.0);
                    self.preview_alpha.set_target(1.0);
                    let ph = primary_height();
                    let _: () = msg_send![
                        self.preview_panel,
                        setFrame:rect(seed.x, ph - seed.y - seed.h, seed.w, seed.h)
                        display:YES
                    ];
                    let _: () = msg_send![self.preview_panel, setAlphaValue:0.0f64];
                    let _: () = msg_send![self.preview_view, setNeedsDisplay:YES];
                    let _: () = msg_send![self.preview_panel, orderFrontRegardless];
                    let _: () = msg_send![self.radial_panel, orderFrontRegardless];
                    self.preview_visible = true;
                } else {
                    self.preview_rect.set_target(target);
                    self.preview_alpha.set_target(1.0);
                    let _: () = msg_send![self.preview_view, setNeedsDisplay:YES];
                }
            }
        } else if self.instant_mode {
            let _: () = msg_send![self.preview_panel, orderOut:nil];
            self.preview_visible = false;
            self.preview_alpha.snap(0.0);
        } else if self.preview_visible {
            self.preview_alpha.set_target(0.0);
        }
        let _: () = msg_send![self.radial_view, setNeedsDisplay:YES];
        self.ensure_anim_timer();
    }

    unsafe fn radial_check(&mut self, pid: i32) -> Result<(), String> {
        self.instant_mode = true;
        let window = windows()
            .into_iter()
            .find(|w| w.pid == pid && w.title.starts_with("RingSwitch QA "))
            .ok_or("需要 QA 测试窗口")?;
        activate(&window)?;
        let original = frame(window.ax.0).ok_or("无法读取测试窗口")?;
        let screen = screens().first().ok_or("没有显示器")?.1;
        let origin = (screen.x + screen.w / 2., screen.y + screen.h / 2.);
        let result = (|| {
            for (i, layout) in Layout::DIRECTIONS.into_iter().enumerate() {
                self.command(Command::RadialStart(origin.0, origin.1));
                let angle = i as f64 * std::f64::consts::FRAC_PI_4;
                self.command(Command::RadialMove(
                    origin.0 + angle.cos() * 90.,
                    origin.1 + angle.sin() * 90.,
                ));
                let radial = self.radial.as_ref().ok_or("没有光环")?;
                if radial.selected != Some(layout) {
                    return Err("光环方向不匹配".into());
                }
                let shown: bool = msg_send![self.preview_panel, isVisible];
                let preview_frame: NSRect = msg_send![self.preview_panel, frame];
                let target = layout.rect(screen);
                if !shown
                    || (preview_frame.origin.x - target.x).abs() > 1.
                    || (preview_frame.origin.y - (primary_height() - target.y - target.h)).abs()
                        > 1.
                    || (preview_frame.size.width - target.w).abs() > 1.
                    || (preview_frame.size.height - target.h).abs() > 1.
                {
                    return Err("目标区域预览不匹配".into());
                }
                if frame(window.ax.0) != Some(original) {
                    return Err("松开前窗口已被改变".into());
                }
                self.command(Command::Cancel);
                let shown: bool = msg_send![self.preview_panel, isVisible];
                if shown || frame(window.ax.0) != Some(original) {
                    return Err("取消后预览未消失或窗口被改变".into());
                }
            }
            println!("PASS: 8 directions -> visible glass target frame; window unchanged until release; cancel hides preview");
            self.command(Command::RadialStart(origin.0, origin.1));
            self.command(Command::RadialEnd(origin.0 + 90., origin.1));
            let actual = frame(window.ax.0).ok_or("无法回读松开后的窗口")?;
            let target = Layout::Right.rect(screen);
            if (actual.x - target.x).abs() > 4.
                || (actual.y - target.y).abs() > 4.
                || (actual.w - target.w).abs() > 4.
                || (actual.h - target.h).abs() > 4.
            {
                return Err("松开后窗口位置不匹配".into());
            }
            let shown: bool = msg_send![self.preview_panel, isVisible];
            if shown {
                return Err("确认后预览未消失".into());
            }
            println!("PASS: release -> actual resize readback and preview dismissed");
            Ok(())
        })();
        self.hide_all();
        result.and(resize(window.ax.0, original))
    }
}

trait OwnedExt {
    fn ax_or_self(&self) -> Ref;
}
impl OwnedExt for Owned {
    fn ax_or_self(&self) -> Ref {
        self.0
    }
}

pub unsafe fn run() {
    let pool = NSAutoreleasePool::new(nil);
    let application: id = msg_send![class!(NSApplication), sharedApplication];
    let _: () = msg_send![application, setActivationPolicy:1isize];
    let args: Vec<String> = std::env::args().collect();

    if let Some(index) = args.iter().position(|a| a == "--capture-check") {
        let pid: i32 = args
            .get(index + 1)
            .and_then(|v| v.parse().ok())
            .expect("需要 QA PID");
        let numbers: Vec<_> = windows()
            .into_iter()
            .filter(|w| w.pid == pid && w.title.starts_with("RingSwitch QA ") && w.number != 0)
            .map(|w| w.number)
            .collect();
        if numbers.len() != 2 || !CGPreflightScreenCaptureAccess() {
            eprintln!("FAIL: need two QA windows and screen capture permission");
            std::process::exit(1);
        }
        let (tx, rx) = mpsc::channel();
        let dispatcher = Dispatcher::new(tx, || {});
        capture::thumbnails(numbers, 1, dispatcher);
        let runloop: id = msg_send![class!(NSRunLoop), currentRunLoop];
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut count = 0;
        while count < 2 && Instant::now() < deadline {
            let date: id = msg_send![class!(NSDate), dateWithTimeIntervalSinceNow:0.05f64];
            let _: () = msg_send![runloop, runUntilDate:date];
            for command in rx.try_iter() {
                if let Command::Thumbnail(_, _, image) = command {
                    if image != 0 {
                        let image = Owned(image as Ref);
                        if CGImageGetWidth(image.0) > 0 && CGImageGetHeight(image.0) > 0 {
                            count += 1;
                        }
                    }
                }
            }
        }
        if count != 2 {
            eprintln!("FAIL: received {count}/2 screenshots");
            std::process::exit(1);
        }
        println!("PASS: two QA windows -> ScreenCaptureKit -> retained CGImage thumbnails");
        pool.drain();
        return;
    }

    if let Some(index) = args.iter().position(|a| a == "--native-check") {
        let _: () = msg_send![application, finishLaunching];
        let result = args
            .get(index + 1)
            .and_then(|s| s.parse().ok())
            .ok_or("需要 QA 进程 PID".into())
            .and_then(|pid| native_check(pid));
        if let Err(e) = result {
            eprintln!("FAIL: {e}");
            std::process::exit(1);
        }
        pool.drain();
        return;
    }

    if args.iter().any(|a| a == "--doctor") {
        println!(
            "Accessibility: {}\nInput Monitoring: {}\nScreen Capture: {}\nMouse button (zero based): {}",
            AXIsProcessTrusted(),
            CGPreflightListenEventAccess(),
            CGPreflightScreenCaptureAccess(),
            Config::load().button
        );
        if AXIsProcessTrusted() {
            println!("Accessible windows: {}", windows().len());
        }
        pool.drain();
        return;
    }

    let _instance_lock = if args
        .iter()
        .any(|a| a.starts_with("--demo") || a == "--qa-windows" || a == "--radial-check")
    {
        None
    } else {
        match instance_lock() {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => {
                eprintln!("RingSwitch is already running");
                return;
            }
            Err(e) => {
                eprintln!("RingSwitch: {e}");
                return;
            }
        }
    };

    let mut view_decl = ClassDecl::new("RingSwitchCanvas", class!(NSView)).unwrap();
    view_decl.add_method(sel!(drawRect:), draw as extern "C" fn(&Object, Sel, NSRect));
    view_decl.add_method(
        sel!(acceptsFirstMouse:),
        accepts_first_mouse as extern "C" fn(&Object, Sel, id) -> BOOL,
    );
    view_decl.add_method(
        sel!(mouseDown:),
        mouse_down as extern "C" fn(&Object, Sel, id),
    );
    let view_class = view_decl.register();

    let mut delegate_decl = ClassDecl::new("RingSwitchDelegate", class!(NSObject)).unwrap();
    delegate_decl.add_method(
        sel!(animTick:),
        anim_tick as extern "C" fn(&Object, Sel, id),
    );
    delegate_decl.add_method(
        sel!(housekeep:),
        housekeep as extern "C" fn(&Object, Sel, id),
    );
    delegate_decl.add_method(
        sel!(workspaceActivated:),
        workspace_activated as extern "C" fn(&Object, Sel, id),
    );
    delegate_decl.add_method(sel!(menuAction:), action as extern "C" fn(&Object, Sel, id));
    let delegate_class = delegate_decl.register();
    let target: id = msg_send![delegate_class, new];

    // 4 dedicated panels: Switcher, Radial Halo, Target Preview, and Toast Pill.
    let (switcher_panel, switcher_view) =
        make_glass_panel(view_class, 540.0, 260.0, 28.0, 25, false, true);
    let _: () = msg_send![switcher_panel, setBecomesKeyOnlyIfNeeded:YES];
    let (radial_panel, radial_glass, radial_view) = make_radial_panel(view_class);
    let (preview_panel, preview_view) =
        make_glass_panel(view_class, 400.0, 300.0, 18.0, 24, true, false);
    let (toast_panel, toast_view) =
        make_glass_panel(view_class, 360.0, 46.0, 23.0, 27, false, true);

    // Subtle circular ? help button inside the switcher header.
    let help: id = msg_send![class!(NSButton), alloc];
    let help: id = msg_send![help, initWithFrame:rect(0., 0., 18., 18.)];
    let _: () = msg_send![help, setBordered:NO];
    let _: () = msg_send![help, setTitle:ns("?")];
    let help_font = font_weight(10.5, 0.23);
    let _: () = msg_send![help, setFont:help_font];
    let help_tint: id = msg_send![class!(NSColor), secondaryLabelColor];
    let _: () = msg_send![help, setContentTintColor:help_tint];
    let _: () = msg_send![help, setWantsLayer:YES];
    let help_layer: id = msg_send![help, layer];
    let _: () = msg_send![help_layer, setCornerRadius:9.0f64];
    let help_bg: id = msg_send![color(1., 1., 1., 0.12), CGColor];
    let _: () = msg_send![help_layer, setBackgroundColor:help_bg];
    let _: () = msg_send![help, setAccessibilityLabel:ns("窗口切换帮助")];
    let _: () = msg_send![help, setTarget:target];
    let _: () = msg_send![help, setAction:sel!(menuAction:)];
    let _: () = msg_send![help, setTag:12isize];
    let _: () = msg_send![switcher_view, addSubview:help];
    let _: () = msg_send![help, release];

    let help_popover: id = msg_send![class!(NSPopover), new];
    let controller: id = msg_send![class!(NSViewController), new];
    let label: id = msg_send![class!(NSTextField), wrappingLabelWithString:ns("")];
    let _: () = msg_send![controller, setView:label];
    let _: () = msg_send![help_popover, setContentViewController:controller];
    let _: () = msg_send![controller, release];
    let _: () = msg_send![help_popover, setContentSize:NSSize::new(320., 56.)];
    let _: () = msg_send![help_popover, setBehavior:1isize];

    // Menu bar status item.
    let status_bar: id = msg_send![class!(NSStatusBar), systemStatusBar];
    let status: id = msg_send![status_bar, statusItemWithLength:-1.0f64];
    let menu: id = msg_send![class!(NSMenu), new];
    let _: () = msg_send![menu, setAutoenablesItems:NO];
    let _: () = msg_send![status, setMenu:menu];

    // Event-driven main runloop wake source (zero idle polling!).
    let mut ctx = CFRunLoopSourceContext {
        version: 0,
        info: std::ptr::null_mut(),
        retain: None,
        release: None,
        copy_description: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform: Some(wake_perform),
    };
    let wake_source_ref = CFRunLoopSourceCreate(std::ptr::null(), 0, &mut ctx);
    let main_rl = CFRunLoopGetMain();
    CFRunLoopAddSource(main_rl, wake_source_ref, kCFRunLoopCommonModes);
    let wake_handle = Arc::new(WakeSource {
        source: wake_source_ref as usize,
        runloop: main_rl as usize,
    });

    let (raw_tx, rx) = mpsc::channel();
    let wake_for_tx = wake_handle.clone();
    let tx = Dispatcher::new(raw_tx, move || wake_for_tx.wake());

    // Dedicated background window-tiling worker thread so releasing the side button never stalls UI.
    let (tile_tx, tile_rx) = mpsc::channel::<TileJob>();
    let tile_gen = Arc::new(AtomicU64::new(0));
    let worker_gen = tile_gen.clone();
    let worker_tx = tx.clone();
    std::thread::spawn(move || unsafe {
        while let Ok(job) = tile_rx.recv() {
            if worker_gen.load(Ordering::Relaxed) != job.gen {
                continue;
            }
            let res = tile_with_cancel(
                job.window.0 .0,
                job.layout,
                job.origin.0,
                job.origin.1,
                || worker_gen.load(Ordering::Relaxed) != job.gen,
            );
            if let Err(err) = res {
                if worker_gen.load(Ordering::Relaxed) == job.gen {
                    let _ = worker_tx.send(Command::TileError(err));
                }
            }
        }
    });

    let qa = args.iter().any(|a| a == "--qa-windows");
    if qa {
        qa_windows();
    }
    let demo = qa || args.iter().any(|a| a.starts_with("--demo"));
    let zero_rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 100.0,
    };

    let mut app = App {
        switcher_panel,
        switcher_view,
        help,
        help_popover,
        radial_panel,
        radial_glass,
        radial_view,
        preview_panel,
        preview_view,
        toast_panel,
        toast_view,
        status,
        menu,
        target,
        anim_timer: nil,
        last_anim: Instant::now(),
        controls: Arc::new(Controls {
            config: Mutex::new(Config::load()),
            recording: AtomicU32::new(0),
            switching_active: AtomicBool::new(false),
        }),
        rx,
        tx,
        tile_tx,
        tile_gen,
        hooked: false,
        demo,
        instant_mode: false,
        windows: vec![],
        selected: 0,
        switching: false,
        switcher_visible: false,
        switcher_alpha: Spring::new(0.0),
        switcher_sel_rect: SpringRect::new(zero_rect),
        switcher_has_sel: false,
        switcher_open_mouse: None,
        switcher_mouse_armed: false,
        grid: (1, 1),
        session: 0,
        thumbnails: HashMap::new(),
        requested: HashSet::new(),
        in_flight: HashSet::new(),
        app_icon: None,
        shimmer_phase: 0.0,
        radial: None,
        radial_visible: false,
        radial_alpha: Spring::new(0.0),
        radial_scale: Spring::new(1.0),
        radial_angle: SpringAngle::new(-PI * 0.5),
        radial_sector_glow: [Spring::new(0.0); 8],
        radial_center_glow: Spring::new(0.0),
        radial_last_mask_scale: 1.0,
        preview_visible: false,
        preview_alpha: Spring::new(0.0),
        preview_rect: SpringRect::new(zero_rect),
        preview_layout: None,
        message: String::new(),
        message_until: Instant::now(),
        toast_visible: false,
        toast_alpha: Spring::new(0.0),
        mru: vec![],
    };

    app.update_menu();

    if let Some(index) = args.iter().position(|a| a == "--radial-check") {
        let result = args
            .get(index + 1)
            .and_then(|s| s.parse().ok())
            .ok_or("需要 QA 进程 PID".into())
            .and_then(|pid| app.radial_check(pid));
        if let Err(e) = result {
            eprintln!("FAIL: {e}");
            std::process::exit(1);
        }
        pool.drain();
        return;
    }

    if args.iter().any(|a| a == "--demo-switch") {
        app.switching = true;
        app.windows = (0..5)
            .map(|i| Window {
                ax: Owned(CFRetain(kCFBooleanFalse)),
                pid: std::process::id() as i32,
                app: "PyCharm".into(),
                title: [
                    "ringswitch — src/ui.rs",
                    "agent-dashboard — App.tsx",
                    "skeletal-pipeline — train.py",
                    "crucible-brew — Cargo.toml",
                    "docs — Architecture Notes",
                ][i]
                    .into(),
                minimized: i == 4,
                number: 0,
                rect: zero_rect,
            })
            .collect();
        app.selected = 1;
        app.show_switcher(true, false);
    } else if demo && !qa {
        let s = screens()[0].1;
        let half = RADIAL_PANEL_SIZE * 0.5;
        let cx = s.x + s.w * 0.5;
        let cy = s.y + s.h * 0.5;
        app.radial_scale.snap(1.0);
        app.radial_alpha.snap(1.0);
        app.radial_angle.snap(-PI * 0.5); // Top
        app.radial_sector_glow[6].snap(1.0); // Layout::Top
        let _: () = msg_send![
            app.radial_panel,
            setFrame:rect(
                cx - half,
                primary_height() - (cy - half) - RADIAL_PANEL_SIZE,
                RADIAL_PANEL_SIZE,
                RADIAL_PANEL_SIZE
            )
            display:YES
        ];
        let _: () = msg_send![app.radial_panel, setAlphaValue:1.0f64];
        let _: () = msg_send![app.radial_view, setNeedsDisplay:YES];
        let _: () = msg_send![app.radial_panel, orderFrontRegardless];
        app.radial_visible = true;
        if args.iter().any(|a| a == "--demo-preview") {
            let t = Layout::Top.rect(s);
            app.preview_layout = Some(Layout::Top);
            app.preview_rect.snap(t);
            app.preview_alpha.snap(1.0);
            let _: () = msg_send![
                app.preview_panel,
                setFrame:rect(t.x, primary_height() - t.y - t.h, t.w, t.h)
                display:YES
            ];
            let _: () = msg_send![app.preview_panel, setAlphaValue:1.0f64];
            let _: () = msg_send![app.preview_view, setNeedsDisplay:YES];
            let _: () = msg_send![app.preview_panel, orderFrontRegardless];
            let _: () = msg_send![app.radial_panel, orderFrontRegardless];
            app.preview_visible = true;
        }
    } else if !demo {
        if AXIsProcessTrusted() && CGPreflightListenEventAccess() {
            match input::start(app.tx.clone(), app.controls.clone()) {
                Ok(()) => {
                    app.hooked = true;
                    app.update_menu();
                    eprintln!("RingSwitch: input tap ready");
                }
                Err(e) => eprintln!("{e}"),
            }
        } else {
            app.toast("请在菜单栏 RS ! 中开启辅助功能与输入监控权限");
            CGRequestListenEventAccess();
        }
    }

    eprintln!(
        "RingSwitch: startup AX={} input={} capture={} demo={}",
        AXIsProcessTrusted(),
        CGPreflightListenEventAccess(),
        CGPreflightScreenCaptureAccess(),
        demo
    );

    // Subscribe to workspace app activations for instant MRU updates with zero polling.
    if !demo {
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let nc: id = msg_send![workspace, notificationCenter];
        let _: () = msg_send![
            nc,
            addObserver:target
            selector:sel!(workspaceActivated:)
            name:ns("NSWorkspaceDidActivateApplicationNotification")
            object:nil
        ];
    }

    APP.with(|cell| *cell.borrow_mut() = Some(app));

    // Gentle low-power background timer (1.5s, 0.5s tolerance) for permission retry & intra-app MRU.
    if !demo {
        let bg_timer: id = msg_send![
            class!(NSTimer),
            timerWithTimeInterval:1.5f64
            target:target
            selector:sel!(housekeep:)
            userInfo:nil
            repeats:YES
        ];
        let _: () = msg_send![bg_timer, setTolerance:0.5f64];
        let runloop: id = msg_send![class!(NSRunLoop), mainRunLoop];
        let _: () = msg_send![runloop, addTimer:bg_timer forMode:ns("kCFRunLoopCommonModes")];
    }

    let _: () = msg_send![application, run];
    CFRelease(wake_source_ref);
    pool.drain();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switcher_hit_test_finds_cards_and_page_dots() {
        let cols = 3;
        let rows = 2;
        let w = SWITCHER_PAD_X * 2.0 + cols as f64 * CARD_W + (cols - 1) as f64 * CARD_GAP_X;
        let h = SWITCHER_HEADER_H
            + rows as f64 * CARD_H
            + (rows - 1) as f64 * CARD_GAP_Y
            + SWITCHER_PAD_BOTTOM
            + SWITCHER_DOTS_H;
        let c0 = card_frame_in_switcher(0, cols, h);
        let c4 = card_frame_in_switcher(4, cols, h);
        assert_eq!(
            switcher_hit_test((c0.x + 20.0, c0.y + 20.0), w, h, (cols, rows), 0, 8),
            Some(SwitchHit::Card(0))
        );
        assert_eq!(
            switcher_hit_test((c4.x + 50.0, c4.y + 50.0), w, h, (cols, rows), 0, 8),
            Some(SwitchHit::Card(4))
        );
        // Header area must not hit any card so the ? help button receives clicks cleanly.
        assert_eq!(
            switcher_hit_test((w - 28.0, h - 24.0), w, h, (cols, rows), 0, 8),
            None
        );
        // Page dots at bottom center for 2 pages (8 windows, capacity 6).
        let total_w = 12.0 + 16.0;
        let first_dot_x = (w - total_w) * 0.5 + 8.0;
        let second_dot_x = (w - total_w) * 0.5 + 16.0 + 6.5 + 2.5;
        assert_eq!(
            switcher_hit_test((first_dot_x, 14.0), w, h, (cols, rows), 0, 8),
            Some(SwitchHit::Page(0))
        );
        assert_eq!(
            switcher_hit_test((second_dot_x, 14.0), w, h, (cols, rows), 0, 8),
            Some(SwitchHit::Page(1))
        );
    }
}
