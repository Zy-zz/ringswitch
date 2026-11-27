use crate::{
    input::{Command, Dispatcher},
    mac::*,
};
use block::ConcreteBlock;
use cocoa::{
    base::{id, nil, NO, YES},
    foundation::NSAutoreleasePool,
};
use objc::{class, msg_send, runtime::Class, sel, sel_impl};
use std::collections::HashMap;

#[link(name = "ScreenCaptureKit", kind = "framework")]
extern "C" {}

fn target_pixel_size(bounds_w: f64, bounds_h: f64) -> (usize, usize) {
    let w = bounds_w.max(1.0);
    let h = bounds_h.max(1.0);
    // Crisp 2x Retina target for ~230x135 pt preview cards while keeping capture fast.
    let scale = (520.0 / w).min(360.0 / h).min(2.0);
    let pw = (w * scale).round().clamp(1.0, 520.0) as usize;
    let ph = (h * scale).round().clamp(1.0, 360.0) as usize;
    (pw, ph)
}

unsafe fn capture_legacy(
    filter: id,
    pw: usize,
    ph: usize,
    session: u64,
    number: u32,
    tx: Dispatcher,
) {
    let config: id = msg_send![class!(SCStreamConfiguration), new];
    let _: () = msg_send![config, setWidth:pw];
    let _: () = msg_send![config, setHeight:ph];
    let _: () = msg_send![config, setShowsCursor:NO];
    let _: () = msg_send![config, setScalesToFit:YES];
    let has_aspect: bool = msg_send![config, respondsToSelector:sel!(setPreservesAspectRatio:)];
    if has_aspect {
        let _: () = msg_send![config, setPreservesAspectRatio:YES];
    }
    let has_shadow: bool =
        msg_send![config, respondsToSelector:sel!(setIgnoreShadowsSingleWindow:)];
    if has_shadow {
        let _: () = msg_send![config, setIgnoreShadowsSingleWindow:YES];
    }
    let has_opaque: bool = msg_send![config, respondsToSelector:sel!(setShouldBeOpaque:)];
    if has_opaque {
        let _: () = msg_send![config, setShouldBeOpaque:YES];
    }
    let has_res: bool = msg_send![config, respondsToSelector:sel!(setCaptureResolution:)];
    if has_res {
        // SCCaptureResolutionNominal = 2: faster thumbnailing with clean downscale.
        let _: () = msg_send![config, setCaptureResolution:2isize];
    }
    let done = ConcreteBlock::new(move |image: Ref, error: id| {
        if error == nil && !image.is_null() {
            let retained = CFRetain(image) as usize;
            if tx
                .send(Command::Thumbnail(session, number, retained))
                .is_err()
            {
                CFRelease(retained as Ref);
            }
        } else {
            let _ = tx.send(Command::Thumbnail(session, number, 0));
        }
    })
    .copy();
    let _: () = msg_send![
        class!(SCScreenshotManager),
        captureImageWithFilter:filter
        configuration:config
        completionHandler:&*done
    ];
    let _: () = msg_send![config, release];
}

// Captures only the requested page windows once per session; images stay in memory only.
pub unsafe fn thumbnails(numbers: Vec<u32>, session: u64, tx: Dispatcher) {
    if numbers.is_empty() || !CGPreflightScreenCaptureAccess() {
        return;
    }
    let use_modern_api = Class::get("SCScreenshotConfiguration").is_some() && {
        let mgr: id = class!(SCScreenshotManager) as *const _ as id;
        let responds: bool = msg_send![
            mgr,
            respondsToSelector:sel!(captureScreenshotWithFilter:configuration:completionHandler:)
        ];
        responds
    };

    let completion = ConcreteBlock::new(move |content: id, error: id| {
        if error != nil || content == nil {
            for &number in &numbers {
                let _ = tx.send(Command::Thumbnail(session, number, 0));
            }
            return;
        }
        let pool = NSAutoreleasePool::new(nil);
        let windows: id = msg_send![content, windows];
        let count: usize = msg_send![windows, count];
        let mut by_id: HashMap<u32, id> = HashMap::with_capacity(numbers.len());
        for i in 0..count {
            let window: id = msg_send![windows, objectAtIndex:i];
            let number: u32 = msg_send![window, windowID];
            if numbers.contains(&number) {
                by_id.insert(number, window);
                if by_id.len() == numbers.len() {
                    break;
                }
            }
        }

        // Dispatch in caller-specified priority order (selected window first).
        for &number in &numbers {
            let Some(&window) = by_id.get(&number) else {
                let _ = tx.send(Command::Thumbnail(session, number, 0));
                continue;
            };
            let filter: id = msg_send![class!(SCContentFilter), alloc];
            let filter: id = msg_send![filter, initWithDesktopIndependentWindow:window];
            let bounds: cocoa::foundation::NSRect = msg_send![window, frame];
            let (pw, ph) = target_pixel_size(bounds.size.width, bounds.size.height);
            let tx_item = tx.clone();

            if use_modern_api {
                let config_cls = Class::get("SCScreenshotConfiguration").unwrap();
                let config: id = msg_send![config_cls, new];
                let _: () = msg_send![config, setWidth:pw as isize];
                let _: () = msg_send![config, setHeight:ph as isize];
                let _: () = msg_send![config, setShowsCursor:NO];
                let _: () = msg_send![config, setIgnoreShadows:YES];
                let _: () = msg_send![config, setDynamicRange:0isize];
                // Retain filter for optional legacy fallback if modern capture fails.
                let fallback_filter = CFRetain(filter as Ref) as usize;
                let done = ConcreteBlock::new(move |output: id, error: id| {
                    let fallback_filter = fallback_filter as id;
                    if error == nil && output != nil {
                        let image: Ref = msg_send![output, sdrImage];
                        if !image.is_null() {
                            let retained = CFRetain(image) as usize;
                            if tx_item
                                .send(Command::Thumbnail(session, number, retained))
                                .is_err()
                            {
                                CFRelease(retained as Ref);
                            }
                            let _: () = msg_send![fallback_filter, release];
                            return;
                        }
                    }
                    let pool = NSAutoreleasePool::new(nil);
                    capture_legacy(fallback_filter, pw, ph, session, number, tx_item.clone());
                    let _: () = msg_send![fallback_filter, release];
                    pool.drain();
                })
                .copy();
                let _: () = msg_send![
                    class!(SCScreenshotManager),
                    captureScreenshotWithFilter:filter
                    configuration:config
                    completionHandler:&*done
                ];
                let _: () = msg_send![config, release];
            } else {
                capture_legacy(filter, pw, ph, session, number, tx_item);
            }
            let _: () = msg_send![filter, release];
        }
        pool.drain();
    })
    .copy();
    let _: () = msg_send![
        class!(SCShareableContent),
        getShareableContentExcludingDesktopWindows:YES
        onScreenWindowsOnly:NO
        completionHandler:&*completion
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_size_preserves_aspect_within_retina_bounds() {
        let (w, h) = target_pixel_size(1920.0, 1080.0);
        assert_eq!(w, 520);
        assert!((h as f64 - 293.0).abs() <= 1.0);
        let (w2, h2) = target_pixel_size(600.0, 1200.0);
        assert_eq!(h2, 360);
        assert_eq!(w2, 180);
    }
}
