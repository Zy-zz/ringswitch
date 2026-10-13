use crate::{
    capture,
    config::Config,
    geometry::{cycle, selection, Layout, Rect},
    input::{self, Command, Controls},
    mac::*,
};
use cocoa::base::{id, nil, NO, YES};
use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize};
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{Class, Object, Sel},
    sel, sel_impl,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

thread_local! { static APP: RefCell<Option<App>> = const { RefCell::new(None) }; }
struct Radial {
    window: Owned,
    origin: (f64, f64),
    screen: Rect,
    selected: Option<Layout>,
    armed: bool,
    started: Instant,
}
struct App {
    panel: id,
    view: id,
    preview: id,
    preview_view: id,
    status: id,
    menu: id,
    target: id,
    controls: Arc<Controls>,
    rx: mpsc::Receiver<Command>,
    tx: mpsc::Sender<Command>,
    hooked: bool,
    last_check: Instant,
    windows: Vec<Window>,
    selected: usize,
    switching: bool,
    radial: Option<Radial>,
    mru: Vec<Owned>,
    message: String,
    message_until: Instant,
    demo: bool,
    grid: (usize, usize),
    session: u64,
    thumbnails: HashMap<u32, Owned>,
    requested: HashSet<u32>,
}

unsafe fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}
unsafe fn color(r: f64, g: f64, b: f64, a: f64) -> id {
    msg_send![class!(NSColor), colorWithCalibratedRed:r green:g blue:b alpha:a]
}
unsafe fn fill(r: NSRect, c: id, radius: f64) {
    let _: () = msg_send![c, set];
    let path: id =
        msg_send![class!(NSBezierPath), bezierPathWithRoundedRect:r xRadius:radius yRadius:radius];
    let _: () = msg_send![path, fill];
}
unsafe fn text(s: &str, r: NSRect, size: f64, c: id) {
    let font: id = msg_send![class!(NSFont), systemFontOfSize:size];
    let keys = [ns("NSFont"), ns("NSColor")];
    let values = [font, c];
    let attrs: id = msg_send![class!(NSDictionary), dictionaryWithObjects:values.as_ptr() forKeys:keys.as_ptr() count:2usize];
    let _: () = msg_send![ns(s),drawInRect:r withAttributes:attrs];
}
fn shorten(s: &str, count: usize) -> String {
    if s.chars().count() <= count {
        s.into()
    } else {
        s.chars().take(count - 1).chain(['…']).collect()
    }
}

unsafe fn outline(r: NSRect, c: id, radius: f64, width: f64) {
    let _: () = msg_send![c, set];
    let path: id =
        msg_send![class!(NSBezierPath),bezierPathWithRoundedRect:r xRadius:radius yRadius:radius];
    let _: () = msg_send![path,setLineWidth:width];
    let _: () = msg_send![path, stroke];
}
unsafe fn icon(pid: i32, r: NSRect) {
    let app: id =
        msg_send![class!(NSRunningApplication),runningApplicationWithProcessIdentifier:pid];
    if app == nil {
        return;
    }
    let image: id = msg_send![app, icon];
    if image != nil {
        let _: () = msg_send![image,drawInRect:r];
    }
}
unsafe fn layout_symbol(layout: Layout, r: NSRect) {
    outline(r, color(0.63, 0.68, 0.77, 1.), 4., 1.5);
    let inset = Rect {
        x: r.origin.x + 3.,
        y: 0.,
        w: r.size.width - 6.,
        h: r.size.height - 6.,
    };
    let part = layout.rect(inset);
    fill(
        rect(
            part.x,
            r.origin.y + r.size.height - 3. - part.y - part.h,
            part.w,
            part.h,
        ),
        color(0.34, 0.68, 1., 1.),
        2.,
    );
}
extern "C" fn draw(view: &Object, _: Sel, _: NSRect) {
    unsafe {
        APP.with(|cell| {
        let Ok(app)=cell.try_borrow()else{return};let Some(app)=app.as_ref()else{return};
        let b:NSRect=msg_send![view,bounds];
        if std::ptr::eq(view,app.preview_view) {
            fill(b,color(0.25,0.57,1.,0.14),12.);outline(rect(2.,2.,b.size.width-4.,b.size.height-4.),color(0.40,0.72,1.,0.92),10.,3.);return;
        }
        let white=color(0.95,0.97,1.,1.);
        if app.radial.is_some()||(app.demo&&!app.switching) {
            let selected=app.radial.as_ref().and_then(|r|r.selected).or(if app.demo{Some(Layout::Top)}else{None});
            let center=NSPoint::new(90.,105.);
            for (i,layout) in Layout::DIRECTIONS.iter().enumerate() {
                let a=-(i as f64)*45.;let path:id=msg_send![class!(NSBezierPath),bezierPath];
                let _:()=msg_send![path,appendBezierPathWithArcWithCenter:center radius:58. startAngle:a-22.3 endAngle:a+22.3];
                let _:()=msg_send![path,appendBezierPathWithArcWithCenter:center radius:43. startAngle:a+22.3 endAngle:a-22.3 clockwise:YES];
                let _:()=msg_send![path,closePath];
                let c=if selected==Some(*layout)||selected==Some(Layout::Maximize){color(0.30,0.63,1.,0.98)}else{color(0.13,0.16,0.22,0.90)};
                let _:()=msg_send![c,set];let _:()=msg_send![path,fill];
            }
            fill(rect(56.,77.,68.,56.),color(0.10,0.13,0.19,0.92),12.);
            layout_symbol(selected.unwrap_or(Layout::Maximize),rect(68.,91.,44.,28.));
            let label=selected.map(|s|s.label()).unwrap_or("移动选择 · Esc 取消");
            fill(rect(12.,12.,156.,27.),color(0.10,0.13,0.19,0.92),8.);
            text(label,rect(28.,16.,136.,19.),12.,white);return;
        }
        fill(b,color(0.12,0.14,0.19,0.96),18.);
        if !app.switching {text(&app.message,rect(24.,25.,b.size.width-48.,55.),15.,white);return;}
        let config=*app.controls.config.lock().unwrap();
        text("窗口",rect(22.,b.size.height-36.,180.,22.),15.,white);
        text(&format!("{} / {}",app.selected+1,app.windows.len()),rect(b.size.width-80.,b.size.height-34.,65.,20.),12.,color(0.65,0.69,0.78,1.));
        let capacity=app.grid.0*app.grid.1;let start=app.selected/capacity*capacity;
        for (position,window) in app.windows.iter().skip(start).take(capacity).enumerate() {
            let index=start+position;let col=position%app.grid.0;let row=position/app.grid.0;
            let x=16.+col as f64*220.;let y=b.size.height-46.-(row+1)as f64*184.;
            let card=rect(x,y,208.,172.);
            fill(card,if index==app.selected{color(0.20,0.34,0.52,1.)}else{color(0.18,0.20,0.25,1.)},10.);
            if index==app.selected{outline(rect(x+1.,y+1.,206.,170.),color(0.40,0.72,1.,1.),9.,2.5);}
            let preview=rect(x+8.,y+40.,192.,124.);
            fill(preview,color(0.09,0.10,0.13,1.),6.);
            if let Some(image)=app.thumbnails.get(&window.number) {
                let iw=CGImageGetWidth(image.0)as f64;let ih=CGImageGetHeight(image.0)as f64;
                let scale=(192./iw).min(124./ih);let w=iw*scale;let h=ih*scale;
                let nsimage:id=msg_send![class!(NSImage),alloc];
                let nsimage:id=msg_send![nsimage,initWithCGImage:image.0 size:NSSize::new(iw,ih)];
                let _:()=msg_send![nsimage,drawInRect:rect(preview.origin.x+(192.-w)/2.,preview.origin.y+(124.-h)/2.,w,h)];
                let _:()=msg_send![nsimage,release];
            }else{
                icon(window.pid,rect(x+80.,y+86.,48.,48.));
                text(if window.minimized{"已最小化"}else{"窗口预览"},rect(x+69.,y+59.,105.,18.),11.,color(0.55,0.6,0.7,1.));
            }
            icon(window.pid,rect(x+10.,y+12.,20.,20.));
            text(&shorten(if window.title.is_empty(){&window.app}else{&window.title},24),rect(x+37.,y+18.,163.,19.),12.,white);
            text(&shorten(&window.app,26),rect(x+37.,y+3.,163.,16.),10.,color(0.65,0.70,0.79,1.));
        }
        let hold=config.switch.label();
        text(&format!("{} · 松开修饰键切换 · Shift 反向 · Esc 取消",hold),rect(22.,12.,b.size.width-44.,22.),11.,color(0.67,0.72,0.80,1.));
    });
    }
}

extern "C" fn tick(_: &Object, _: Sel, _: id) {
    unsafe {
        APP.with(|cell| {
            let mut app = cell.borrow_mut();
            let Some(app) = app.as_mut() else { return };
            let pool = NSAutoreleasePool::new(nil);
            let commands: Vec<_> = app.rx.try_iter().collect();
            // Coalesce motion so high-polling-rate mice cannot delay button release.
            let mut motion = None;
            for command in commands {
                if matches!(command, Command::RadialMove(..)) {
                    motion = Some(command);
                    continue;
                }
                if let Some(m) = motion.take() {
                    app.command(m);
                }
                app.command(command);
            }
            if let Some(m) = motion {
                app.command(m);
            }
            if app.last_check.elapsed() > Duration::from_millis(600) && !app.demo {
                app.last_check = Instant::now();
                if !app.hooked && AXIsProcessTrusted() && CGPreflightListenEventAccess() {
                    match input::start(app.tx.clone(), app.controls.clone()) {
                        Ok(()) => {
                            app.hooked = true;
                            eprintln!("RingSwitch: input tap ready");
                            app.update_menu();
                        }
                        Err(e) => eprintln!("{e}"),
                    }
                }
                if !app.switching && app.radial.is_none() && AXIsProcessTrusted() {
                    if let Some(focused) = focused() {
                        app.mru.retain(|w| w != &focused);
                        app.mru.insert(0, focused);
                        app.mru.truncate(100);
                    }
                }
            }
            if !app.switching
                && app.radial.is_none()
                && !app.demo
                && Instant::now() > app.message_until
            {
                let _: () = msg_send![app.panel,orderOut:nil];
            }
            pool.drain();
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
                3|4=>{let mut c=*app.controls.config.lock().unwrap();c.button=tag as u32;app.save_config(c);}
                5|7|8=> {
                    if !app.hooked { app.toast("请先允许辅助功能和输入监控权限，再录制按键"); return; }
                    let mode=match tag{7=>1,8=>2,_=>3};
                    app.controls.recording.store(mode,Ordering::Relaxed);
                    app.hide();
                    app.toast(match mode{1=>"按下新的窗口切换组合键（⌃ / ⌥ / ⌘ + 一个键）；Esc 取消",
                        2=>"按下圆盘触发键或组合键；单独修饰键请按下后松开；Esc 取消",
                        _=>"请按一次希望使用的鼠标侧键；Esc 取消"});
                }
                9=>{let mut c=*app.controls.config.lock().unwrap();c.radial=None;app.save_config(c);}
                10=>{app.save_config(Config::default());}
                11=>{CGRequestScreenCaptureAccess();open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture");}
                6 => {
                    let application: id = msg_send![class!(NSApplication), sharedApplication];
                    let _: () = msg_send![application,terminate:nil];
                }
                _ => (),
            }
        });
    }
}

unsafe fn open_url(s: &str) {
    let url: id = msg_send![class!(NSURL),URLWithString:ns(s)];
    let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
    let _: bool = msg_send![workspace,openURL:url];
}
unsafe fn panel(view_class: &Class) -> (id, id) {
    let p: id = msg_send![class!(NSPanel), alloc];
    let p: id = msg_send![p,initWithContentRect:rect(0.,0.,300.,300.) styleMask:128usize backing:2usize defer:NO];
    let _: () = msg_send![p,setOpaque:NO];
    let clear: id = msg_send![class!(NSColor), clearColor];
    let _: () = msg_send![p,setBackgroundColor:clear];
    let _: () = msg_send![p,setLevel:25isize];
    let _: () = msg_send![p,setCollectionBehavior:257usize];
    let _: () = msg_send![p,setIgnoresMouseEvents:YES];
    let _: () = msg_send![p,setHidesOnDeactivate:NO];
    let _: () = msg_send![p,setReleasedWhenClosed:NO];
    let view: id = msg_send![view_class, alloc];
    let view: id = msg_send![view,initWithFrame:rect(0.,0.,300.,300.)];
    let _: () = msg_send![p,setContentView:view];
    (p, view)
}

impl App {
    unsafe fn show(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let _: () = msg_send![self.panel,setFrame:rect(x,primary_height()-y-h,w,h) display:YES];
        let _: () = msg_send![self.view,setNeedsDisplay:YES];
        let _: () = msg_send![self.panel, orderFrontRegardless];
    }
    unsafe fn toast(&mut self, message: &str) {
        eprintln!("RingSwitch: {message}");
        self.message = message.into();
        self.message_until = Instant::now() + Duration::from_secs(4);
        let screen = screens().first().map(|(_, v)| *v);
        if let Some(s) = screen {
            self.show(s.x + (s.w - 580.) / 2., s.y + s.h / 2. - 45., 580., 100.);
        }
    }
    unsafe fn hide(&mut self) {
        self.switching = false;
        self.radial = None;
        let _: () = msg_send![self.panel,orderOut:nil];
        let _: () = msg_send![self.preview,orderOut:nil];
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
        let _: () = msg_send![button,setTitle:ns(if self.hooked{"RS"}else{"RS !"})];
        let c = *self.controls.config.lock().unwrap();
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
            ("辅助功能权限…".into(), 1),
            ("输入监控权限…".into(), 2),
            ("窗口缩略图权限…".into(), 11),
            ("退出 RingSwitch".into(), 6),
        ];
        for (label, tag) in labels {
            let item: id = msg_send![class!(NSMenuItem), alloc];
            let item: id = msg_send![item,initWithTitle:ns(&label) action:sel!(menuAction:) keyEquivalent:ns("")];
            let _: () = msg_send![item,setTag:tag as isize];
            let _: () = msg_send![item,setTarget:self.target];
            if tag == 0 {
                let _: () = msg_send![item,setEnabled:NO];
            }
            if (tag == 3 || tag == 4) && c.button == tag as u32 || tag == 9 && c.radial.is_none() {
                let _: () = msg_send![item,setState:1isize];
            }
            let _: () = msg_send![self.menu,addItem:item];
            let _: () = msg_send![item, release];
        }
    }
    unsafe fn show_switcher(&mut self) {
        let mouse: NSPoint = msg_send![class!(NSEvent), mouseLocation];
        let s = screen_at(&screens(), mouse.x, primary_height() - mouse.y).unwrap();
        let cols = (((s.w - 32.) / 220.) as usize)
            .clamp(1, 4)
            .min(self.windows.len().max(1));
        let rows = (((s.h - 86.) / 184.) as usize)
            .clamp(1, 3)
            .min(self.windows.len().div_ceil(cols).max(1));
        self.grid = (cols, rows);
        let w = 32. + cols as f64 * 220.;
        let h = 86. + rows as f64 * 184.;
        self.show(s.x + (s.w - w) / 2., s.y + (s.h - h) / 2., w, h);
        let capacity = cols * rows;
        let start = self.selected / capacity * capacity;
        let numbers = self
            .windows
            .iter()
            .skip(start)
            .take(capacity)
            .map(|w| w.number)
            .filter(|id| *id != 0 && self.requested.insert(*id))
            .collect();
        capture::thumbnails(numbers, self.session, self.tx.clone());
    }
    unsafe fn command(&mut self, command: Command) {
        match command {
            Command::Cycle(backwards) => {
                if !self.switching {
                    self.hide();
                    self.session = self.session.wrapping_add(1);
                    self.thumbnails.clear();
                    self.requested.clear();
                    self.windows = windows();
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
                }
                self.selected = cycle(self.selected, self.windows.len(), backwards);
                self.show_switcher();
            }
            Command::CommitSwitch => {
                if self.switching {
                    let selected = self.windows.get(self.selected).cloned();
                    self.hide();
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
            Command::Cancel => {
                self.controls.recording.store(0, Ordering::Relaxed);
                self.hide();
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
                let image = Owned(image as Ref);
                if self.switching && session == self.session {
                    self.thumbnails.insert(number, image);
                    let _: () = msg_send![self.view,setNeedsDisplay:YES];
                }
            }
            Command::RadialStart(x, y) => {
                self.hide();
                let Some(window) = focused() else {
                    self.toast("当前没有可调整的窗口");
                    return;
                };
                let Some(screen) = screen_at(&screens(), x, y) else {
                    return;
                };
                self.radial = Some(Radial {
                    window,
                    origin: (x, y),
                    screen,
                    selected: None,
                    armed: false,
                    started: Instant::now(),
                });
                // Keep the ring centered at the actual cursor, even near display edges.
                self.show(x - 90., y - 75., 180., 180.);
            }
            Command::RadialMove(x, y) => self.move_radial(x, y),
            Command::RadialEnd(x, y) => {
                self.move_radial(x, y);
                let radial = self.radial.take();
                self.hide();
                if let Some(r) = radial {
                    if let Some(layout) = r.selected {
                        if let Err(e) = resize(r.window.0, layout.rect(r.screen)) {
                            self.toast(&e);
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
        if dx.hypot(dy) >= 28. || r.started.elapsed() > Duration::from_millis(500) {
            r.armed = true;
        }
        r.selected = selection(dx, dy, r.armed);
        let _: () = msg_send![self.view,setNeedsDisplay:YES];
        if let Some(layout) = r.selected {
            let t = layout.rect(r.screen);
            let _: () = msg_send![self.preview,setFrame:rect(t.x,primary_height()-t.y-t.h,t.w,t.h) display:YES];
            let _: () = msg_send![self.preview_view,setNeedsDisplay:YES];
            let _: () = msg_send![self.preview, orderFrontRegardless];
            let _: () = msg_send![self.panel, orderFrontRegardless];
        } else {
            let _: () = msg_send![self.preview,orderOut:nil];
        }
    }
}

pub unsafe fn run() {
    let pool = NSAutoreleasePool::new(nil);
    let application: id = msg_send![class!(NSApplication), sharedApplication];
    let _: () = msg_send![application,setActivationPolicy:1isize];
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
        capture::thumbnails(numbers, 1, tx);
        let runloop: id = msg_send![class!(NSRunLoop), currentRunLoop];
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut count = 0;
        while count < 2 && Instant::now() < deadline {
            let date: id = msg_send![class!(NSDate),dateWithTimeIntervalSinceNow:0.05f64];
            let _: () = msg_send![runloop,runUntilDate:date];
            for command in rx.try_iter() {
                if let Command::Thumbnail(_, _, image) = command {
                    let image = Owned(image as Ref);
                    if CGImageGetWidth(image.0) > 0 && CGImageGetHeight(image.0) > 0 {
                        count += 1;
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
    let mut view = ClassDecl::new("RingSwitchCanvas", class!(NSView)).unwrap();
    view.add_method(sel!(drawRect:), draw as extern "C" fn(&Object, Sel, NSRect));
    let view_class = view.register();
    let mut delegate = ClassDecl::new("RingSwitchDelegate", class!(NSObject)).unwrap();
    delegate.add_method(sel!(tick:), tick as extern "C" fn(&Object, Sel, id));
    delegate.add_method(sel!(menuAction:), action as extern "C" fn(&Object, Sel, id));
    let delegate_class = delegate.register();
    let target: id = msg_send![delegate_class, new];
    let (panel, view) = panel(view_class);
    let (preview, preview_view) = self::panel(view_class);
    let _: () = msg_send![preview,setLevel:24isize];
    let status_bar: id = msg_send![class!(NSStatusBar), systemStatusBar];
    let status: id = msg_send![status_bar,statusItemWithLength:-1.0f64];
    let menu: id = msg_send![class!(NSMenu), new];
    let _: () = msg_send![menu,setAutoenablesItems:NO];
    let _: () = msg_send![status,setMenu:menu];
    let (tx, rx) = mpsc::channel();
    let qa = args.iter().any(|a| a == "--qa-windows");
    if qa {
        qa_windows();
    }
    let demo = qa || args.iter().any(|a| a.starts_with("--demo"));
    let mut app = App {
        panel,
        view,
        preview,
        preview_view,
        status,
        menu,
        target,
        controls: Arc::new(Controls {
            config: Mutex::new(Config::load()),
            recording: AtomicU32::new(0),
        }),
        rx,
        tx,
        hooked: false,
        last_check: Instant::now() - Duration::from_secs(1),
        windows: vec![],
        selected: 0,
        switching: false,
        radial: None,
        mru: vec![],
        message: String::new(),
        message_until: Instant::now(),
        demo,
        grid: (1, 1),
        session: 0,
        thumbnails: HashMap::new(),
        requested: HashSet::new(),
    };
    app.update_menu();
    if args.iter().any(|a| a == "--demo-switch") {
        app.switching = true;
        app.windows = (0..5)
            .map(|i| Window {
                ax: Owned(CFRetain(kCFBooleanFalse)),
                pid: std::process::id() as i32,
                app: ["Finder", "Terminal", "Editor", "Browser", "Notes"][i].into(),
                title: [
                    "Documents",
                    "Local shell",
                    "main.rs — RingSwitch",
                    "Window switcher example",
                    "Project notes",
                ][i]
                    .into(),
                minimized: i == 4,
                number: 0,
            })
            .collect();
        app.selected = 2;
        app.show_switcher();
    } else if demo && !qa {
        let s = screens()[0].1;
        app.show(s.x + s.w / 2. - 90., s.y + s.h / 2. - 90., 180., 180.);
    } else if !demo && (!AXIsProcessTrusted() || !CGPreflightListenEventAccess()) {
        app.toast("请在菜单栏 RS ! 中开启辅助功能与输入监控权限");
        CGRequestListenEventAccess();
    }
    eprintln!(
        "RingSwitch: startup AX={} input={} demo={}",
        AXIsProcessTrusted(),
        CGPreflightListenEventAccess(),
        demo
    );
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    let timer: id = msg_send![class!(NSTimer),timerWithTimeInterval:0.016f64 target:target selector:sel!(tick:) userInfo:nil repeats:YES];
    let runloop: id = msg_send![class!(NSRunLoop), mainRunLoop];
    let _: () = msg_send![runloop,addTimer:timer forMode:ns("kCFRunLoopCommonModes")];
    let _: () = msg_send![application, run];
    pool.drain();
}
