use crate::{
    config::{modifier, Config, Shortcut, MODIFIERS, SHIFT},
    mac::*,
};
use std::ffi::c_void;
use std::ptr::null;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    mpsc::Sender,
    Arc, Mutex,
};

#[derive(Debug, PartialEq)]
pub enum Command {
    Cycle(bool),
    CommitSwitch,
    Cancel,
    RadialStart(f64, f64),
    RadialMove(f64, f64),
    RadialEnd(f64, f64),
    Learned(u32),
    LearnedKey(u32, Shortcut),
    Thumbnail(u64, u32, usize),
}

#[derive(Default)]
struct Gesture {
    switching: Option<u64>,
    held_button: Option<u32>,
    held_key: Option<Shortcut>,
    pending_modifier: Option<u16>,
    swallowed_keys: Vec<u16>,
}

impl Gesture {
    #[allow(clippy::too_many_arguments)]
    fn event(
        &mut self,
        kind: u32,
        key: u16,
        flags: u64,
        button: u32,
        point: (f64, f64),
        config: Config,
        recording: u32,
    ) -> (bool, Option<Command>) {
        let p = point;
        let active =
            self.switching.is_some() || self.held_button.is_some() || self.held_key.is_some();
        if kind == 10 && key == 53 && (active || recording != 0) {
            self.switching = None;
            self.pending_modifier = None;
            self.swallow(key);
            return (true, Some(Command::Cancel));
        }
        if kind == 11 && self.swallowed_keys.contains(&key) {
            self.swallowed_keys.retain(|k| *k != key);
            if self.held_key.is_some_and(|s| s.key == key) {
                self.held_key = None;
                return (true, Some(Command::RadialEnd(p.0, p.1)));
            }
            return (true, None);
        }
        if recording == 1 || recording == 2 {
            if kind == 12 && modifier(key).is_some() {
                let (_, device) = modifier(key).unwrap();
                if flags & device != 0 {
                    self.pending_modifier = Some(key);
                } else if recording == 2 && self.pending_modifier == Some(key) {
                    self.pending_modifier = None;
                    return (
                        false,
                        Some(Command::LearnedKey(
                            recording,
                            Shortcut { key, modifiers: 0 },
                        )),
                    );
                }
            }
            if kind == 10 {
                self.pending_modifier = None;
                self.swallow(key);
                return (
                    true,
                    Some(Command::LearnedKey(
                        recording,
                        Shortcut {
                            key,
                            modifiers: flags & MODIFIERS,
                        },
                    )),
                );
            }
            return (false, None);
        }
        self.pending_modifier = None;
        if kind == 12 {
            if self.switching.is_some_and(|held| flags & held != held) {
                self.switching = None;
                return (false, Some(Command::CommitSwitch));
            }
            if let Some(held) = self.held_key {
                let released = if let Some((_, device)) = modifier(held.key) {
                    key == held.key && flags & device == 0
                } else {
                    held.modifiers != 0 && flags & held.modifiers != held.modifiers
                };
                if released {
                    self.held_key = None;
                    return (false, Some(Command::RadialEnd(p.0, p.1)));
                }
            }
            if let Some(s) = config.radial {
                if let Some((_, device)) = modifier(s.key) {
                    if key == s.key
                        && flags & device != 0
                        && self.held_button.is_none()
                        && self.held_key.is_none()
                        && self.switching.is_none()
                    {
                        self.held_key = Some(s);
                        return (false, Some(Command::RadialStart(p.0, p.1)));
                    }
                }
            }
        }
        if kind == 10 {
            if config.switch.matches(key, flags, true)
                && self.held_button.is_none()
                && self.held_key.is_none()
            {
                self.switching = Some(config.switch.modifiers);
                self.swallow(key);
                return (true, Some(Command::Cycle(flags & SHIFT != 0)));
            }
            if self.switching.is_some() && [123, 124, 125, 126].contains(&key) {
                self.swallow(key);
                return (true, Some(Command::Cycle(key == 123 || key == 126)));
            }
            if let Some(s) = config.radial {
                if modifier(s.key).is_none()
                    && s.matches(key, flags, false)
                    && self.held_button.is_none()
                    && self.switching.is_none()
                {
                    self.swallow(key);
                    if self.held_key.is_none() {
                        self.held_key = Some(s);
                        return (true, Some(Command::RadialStart(p.0, p.1)));
                    }
                    return (true, None);
                }
            }
        }
        if kind == 25
            && (button == config.button || (recording == 3 && (3..=31).contains(&button)))
            && self.held_key.is_none()
        {
            self.held_button = Some(button);
            if recording == 3 {
                return (true, Some(Command::Learned(button)));
            }
            self.switching = None;
            return (true, Some(Command::RadialStart(p.0, p.1)));
        }
        if kind == 26 && self.held_button == Some(button) {
            self.held_button = None;
            return (true, Some(Command::RadialEnd(p.0, p.1)));
        }
        if (self.held_button.is_some() || self.held_key.is_some()) && (kind == 5 || kind == 27) {
            return (kind == 27, Some(Command::RadialMove(p.0, p.1)));
        }
        (false, None)
    }
    fn swallow(&mut self, key: u16) {
        if !self.swallowed_keys.contains(&key) {
            self.swallowed_keys.push(key);
        }
    }
}

pub struct Controls {
    pub config: Mutex<Config>,
    pub recording: AtomicU32,
}
struct Hook {
    gesture: Gesture,
    tx: Sender<Command>,
    controls: Arc<Controls>,
    tap: Ref,
}

extern "C" fn callback(_: Ref, kind: u32, event: Ref, data: *mut c_void) -> Ref {
    unsafe {
        let hook = &mut *(data as *mut Hook);
        if kind == u32::MAX || kind == u32::MAX - 1 {
            hook.gesture.switching = None;
            hook.gesture.held_key = None;
            hook.gesture.held_button = None;
            let _ = hook.tx.send(Command::Cancel);
            CGEventTapEnable(hook.tap, true);
            return event;
        }
        let point = CGEventGetLocation(event);
        let (consume, command) = hook.gesture.event(
            kind,
            CGEventGetIntegerValueField(event, 9) as u16,
            CGEventGetFlags(event),
            CGEventGetIntegerValueField(event, 3) as u32,
            (point.x, point.y),
            *hook.controls.config.lock().unwrap(),
            hook.controls.recording.load(Ordering::Relaxed),
        );
        if let Some(command) = command {
            if matches!(
                command,
                Command::Learned(_) | Command::LearnedKey(..) | Command::Cancel
            ) {
                hook.controls.recording.store(0, Ordering::Relaxed);
            }
            let _ = hook.tx.send(command);
        }
        if consume {
            null()
        } else {
            event
        }
    }
}

pub fn start(tx: Sender<Command>, controls: Arc<Controls>) -> Result<(), String> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || unsafe {
        let mut hook = Box::new(Hook {
            gesture: Gesture::default(),
            tx,
            controls,
            tap: null(),
        });
        let mask = [5, 10, 11, 12, 25, 26, 27]
            .iter()
            .fold(0u64, |m, k| m | (1 << k));
        hook.tap = CGEventTapCreate(1, 0, 0, mask, callback, &mut *hook as *mut _ as *mut c_void);
        if hook.tap.is_null() {
            let _ = ready_tx.send(Err("请允许辅助功能和输入监控权限".into()));
            return;
        }
        let source = CFMachPortCreateRunLoopSource(null(), hook.tap, 0);
        if source.is_null() {
            CFRelease(hook.tap);
            let _ = ready_tx.send(Err("无法创建输入监听".into()));
            return;
        }
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
        CGEventTapEnable(hook.tap, true);
        let _ = ready_tx.send(Ok(()));
        CFRunLoopRun();
        CFRelease(source);
        CFRelease(hook.tap);
    });
    ready_rx
        .recv()
        .unwrap_or_else(|_| Err("输入线程启动失败".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CONTROL, OPTION};
    fn event(
        g: &mut Gesture,
        kind: u32,
        key: u16,
        flags: u64,
        button: u32,
        config: Config,
        recording: u32,
    ) -> (bool, Option<Command>) {
        g.event(kind, key, flags, button, (10., 10.), config, recording)
    }
    #[test]
    fn custom_switch_commit_and_reverse() {
        let mut g = Gesture::default();
        let c = Config {
            switch: Shortcut {
                key: 48,
                modifiers: CONTROL,
            },
            ..Config::default()
        };
        assert!(!event(&mut g, 10, 50, OPTION, 0, c, 0).0);
        assert_eq!(
            event(&mut g, 10, 48, CONTROL | SHIFT, 0, c, 0).1,
            Some(Command::Cycle(true))
        );
        assert_eq!(
            event(&mut g, 12, 59, 0, 0, c, 0).1,
            Some(Command::CommitSwitch)
        );
        assert_eq!(event(&mut g, 11, 48, 0, 0, c, 0), (true, None));
    }
    #[test]
    fn selected_button_learning_and_escape() {
        let mut g = Gesture::default();
        let c = Config::default();
        assert!(!event(&mut g, 25, 0, 0, 2, c, 3).0);
        assert_eq!(
            event(&mut g, 25, 0, 0, 7, c, 3).1,
            Some(Command::Learned(7))
        );
        assert_eq!(event(&mut g, 10, 53, 0, 0, c, 0).1, Some(Command::Cancel));
        assert!(!event(&mut g, 26, 0, 0, 4, c, 0).0);
        assert!(event(&mut g, 26, 0, 0, 7, c, 0).0);
        assert!(!event(&mut g, 26, 0, 0, 7, c, 0).0);
    }
    #[test]
    fn keyboard_radial_release_and_no_repeat() {
        let mut g = Gesture::default();
        let c = Config {
            radial: Some(Shortcut {
                key: 49,
                modifiers: CONTROL,
            }),
            ..Config::default()
        };
        assert_eq!(
            event(&mut g, 10, 49, CONTROL, 0, c, 0).1,
            Some(Command::RadialStart(10., 10.))
        );
        assert_eq!(event(&mut g, 10, 49, CONTROL, 0, c, 0), (true, None));
        assert_eq!(
            event(&mut g, 12, 59, 0, 0, c, 0).1,
            Some(Command::RadialEnd(10., 10.))
        );
        assert_eq!(event(&mut g, 11, 49, 0, 0, c, 0), (true, None));
    }
    #[test]
    fn right_modifier_release_while_left_is_still_down() {
        let mut g = Gesture::default();
        let c = Config {
            radial: Some(Shortcut {
                key: 62,
                modifiers: 0,
            }),
            ..Config::default()
        };
        assert_eq!(
            event(&mut g, 12, 62, CONTROL | 0x2000, 0, c, 0).1,
            Some(Command::RadialStart(10., 10.))
        );
        assert_eq!(
            event(&mut g, 12, 62, CONTROL | 1, 0, c, 0).1,
            Some(Command::RadialEnd(10., 10.))
        );
    }
    #[test]
    fn shortcut_recorder_chord_and_modifier_only() {
        let mut g = Gesture::default();
        let c = Config::default();
        assert_eq!(event(&mut g, 12, 62, CONTROL | 0x2000, 0, c, 2).1, None);
        assert_eq!(
            event(&mut g, 10, 49, CONTROL, 0, c, 2).1,
            Some(Command::LearnedKey(
                2,
                Shortcut {
                    key: 49,
                    modifiers: CONTROL
                }
            ))
        );
        assert_eq!(event(&mut g, 11, 49, 0, 0, c, 0), (true, None));
        event(&mut g, 12, 62, CONTROL | 0x2000, 0, c, 2);
        assert_eq!(
            event(&mut g, 12, 62, 0, 0, c, 2).1,
            Some(Command::LearnedKey(
                2,
                Shortcut {
                    key: 62,
                    modifiers: 0
                }
            ))
        );
    }
}
