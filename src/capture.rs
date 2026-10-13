use crate::{input::Command, mac::*};
use block::ConcreteBlock;
use cocoa::{
    base::{id, nil, NO, YES},
    foundation::NSAutoreleasePool,
};
use objc::{class, msg_send, sel, sel_impl};
use std::sync::mpsc::Sender;

#[link(name = "ScreenCaptureKit", kind = "framework")]
extern "C" {}

// Captures only the current page, once per switcher session; images stay in memory.
pub unsafe fn thumbnails(numbers: Vec<u32>, session: u64, tx: Sender<Command>) {
    if numbers.is_empty() || !CGPreflightScreenCaptureAccess() {
        return;
    }
    let completion=ConcreteBlock::new(move |content:id,error:id| {
        if error!=nil||content==nil{return;}
        let pool=NSAutoreleasePool::new(nil);
        let windows:id=msg_send![content,windows];
        let count:usize=msg_send![windows,count];
        for i in 0..count {
            let window:id=msg_send![windows,objectAtIndex:i];
            let number:u32=msg_send![window,windowID];
            if !numbers.contains(&number){continue;}
            let filter:id=msg_send![class!(SCContentFilter),alloc];
            let filter:id=msg_send![filter,initWithDesktopIndependentWindow:window];
            let config:id=msg_send![class!(SCStreamConfiguration),new];
            let bounds:cocoa::foundation::NSRect=msg_send![window,frame];
            let height=(320.*bounds.size.height/bounds.size.width.max(1.)).clamp(1.,640.) as usize;
            let _:()=msg_send![config,setWidth:320usize];let _:()=msg_send![config,setHeight:height];
            let _:()=msg_send![config,setShowsCursor:NO];
            let tx=tx.clone();
            let done=ConcreteBlock::new(move |image:Ref,error:id| {
                if error==nil&&!image.is_null() {
                    let retained=CFRetain(image) as usize;
                    if tx.send(Command::Thumbnail(session,number,retained)).is_err(){CFRelease(retained as Ref);}
                }
            }).copy();
            let _:()=msg_send![class!(SCScreenshotManager),captureImageWithFilter:filter configuration:config completionHandler:&*done];
            let _:()=msg_send![filter,release];let _:()=msg_send![config,release];
        }
        pool.drain();
    }).copy();
    let _: () = msg_send![class!(SCShareableContent),getShareableContentExcludingDesktopWindows:YES onScreenWindowsOnly:NO completionHandler:&*completion];
}
