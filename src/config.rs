use std::{fs, path::PathBuf};

pub const SHIFT: u64 = 1 << 17;
pub const CONTROL: u64 = 1 << 18;
pub const OPTION: u64 = 1 << 19;
pub const COMMAND: u64 = 1 << 20;
pub const MODIFIERS: u64 = SHIFT | CONTROL | OPTION | COMMAND;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shortcut {
    pub key: u16,
    pub modifiers: u64,
}
impl Shortcut {
    pub fn label(self) -> String {
        let mut label = String::new();
        for (flag, symbol) in [(CONTROL, "⌃"), (OPTION, "⌥"), (SHIFT, "⇧"), (COMMAND, "⌘")]
        {
            if self.modifiers & flag != 0 {
                label.push_str(symbol);
            }
        }
        label.push_str(key_name(self.key));
        label
    }
    pub fn matches(self, key: u16, flags: u64, reverse: bool) -> bool {
        key == self.key
            && (flags & MODIFIERS & if reverse { !SHIFT } else { u64::MAX }) == self.modifiers
    }
}

pub fn modifier(key: u16) -> Option<(u64, u64)> {
    // Aggregate and device-specific masks from macOS IOLLEvent.h.
    match key {
        54 => Some((COMMAND, 0x10)),
        55 => Some((COMMAND, 0x8)),
        56 => Some((SHIFT, 0x2)),
        60 => Some((SHIFT, 0x4)),
        58 => Some((OPTION, 0x20)),
        61 => Some((OPTION, 0x40)),
        59 => Some((CONTROL, 0x1)),
        62 => Some((CONTROL, 0x2000)),
        _ => None,
    }
}
pub fn key_name(key: u16) -> &'static str {
    match key {
        0 => "A",
        1 => "S",
        2 => "D",
        3 => "F",
        4 => "H",
        5 => "G",
        6 => "Z",
        7 => "X",
        8 => "C",
        9 => "V",
        11 => "B",
        12 => "Q",
        13 => "W",
        14 => "E",
        15 => "R",
        16 => "Y",
        17 => "T",
        18 => "1",
        19 => "2",
        20 => "3",
        21 => "4",
        22 => "6",
        23 => "5",
        24 => "=",
        25 => "9",
        26 => "7",
        27 => "-",
        28 => "8",
        29 => "0",
        30 => "]",
        31 => "O",
        32 => "U",
        33 => "[",
        34 => "I",
        35 => "P",
        36 => "↩",
        37 => "L",
        38 => "J",
        39 => "'",
        40 => "K",
        41 => ";",
        42 => "\\",
        43 => ",",
        44 => "/",
        45 => "N",
        46 => "M",
        47 => ".",
        48 => "⇥",
        49 => "Space",
        50 => "`",
        51 => "⌫",
        53 => "Esc",
        54 => "右 ⌘",
        55 => "左 ⌘",
        56 => "左 ⇧",
        58 => "左 ⌥",
        59 => "左 ⌃",
        60 => "右 ⇧",
        61 => "右 ⌥",
        62 => "右 ⌃",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        99 => "F3",
        100 => "F8",
        101 => "F9",
        103 => "F11",
        109 => "F10",
        111 => "F12",
        118 => "F4",
        120 => "F2",
        122 => "F1",
        123 => "←",
        124 => "→",
        125 => "↓",
        126 => "↑",
        _ => "Key",
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    pub switch: Shortcut,
    pub radial: Option<Shortcut>,
    pub button: u32,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            switch: Shortcut {
                key: 50,
                modifiers: OPTION,
            },
            radial: None,
            button: 3,
        }
    }
}
fn path() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/RingSwitch/bindings")
}
impl Config {
    pub fn load() -> Self {
        let mut config = Self::default();
        if let Ok(data) = fs::read_to_string(path()) {
            config = Self::parse(&data).unwrap_or_default();
        } else if let Ok(button) = fs::read_to_string(path().with_file_name("mouse-button")) {
            if let Ok(button) = button.trim().parse::<u32>() {
                if (3..=31).contains(&button) {
                    config.button = button;
                }
            }
        }
        config
    }
    pub fn validate(self) -> Result<Self, String> {
        if self.switch.modifiers == 0
            || self.switch.modifiers & !MODIFIERS != 0
            || self.switch.modifiers & SHIFT != 0
            || modifier(self.switch.key).is_some()
            || self.switch.key == 53
            || self.switch.key > 126
        {
            return Err("窗口切换需要 ⌃、⌥ 或 ⌘ 加一个键；Shift 留作反向切换".into());
        }
        if !(3..=31).contains(&self.button) {
            return Err("请选择鼠标侧键".into());
        }
        if let Some(radial) = self.radial {
            if radial.modifiers & !MODIFIERS != 0
                || radial.key == 53
                || radial.key > 126
                || (modifier(radial.key).is_some() && radial.modifiers != 0)
                || self.switch.matches(radial.key, radial.modifiers, true)
            {
                return Err("圆盘按键与窗口切换冲突，或使用了不支持的组合".into());
            }
            if modifier(radial.key)
                .is_some_and(|(aggregate, _)| self.switch.modifiers & aggregate != 0)
            {
                return Err("圆盘修饰键不能同时用于窗口切换快捷键".into());
            }
        }
        Ok(self)
    }
    pub fn save(self) -> Result<(), String> {
        self.validate()?;
        let data = match self.radial {
            Some(s) => format!(
                "{} {} {} {} {}\n",
                self.switch.key, self.switch.modifiers, self.button, s.key, s.modifiers
            ),
            None => format!(
                "{} {} {} -\n",
                self.switch.key, self.switch.modifiers, self.button
            ),
        };
        let p = path();
        fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        let temp = p.with_extension("tmp");
        fs::write(&temp, data).map_err(|e| e.to_string())?;
        fs::rename(temp, p).map_err(|e| e.to_string())
    }
    fn parse(data: &str) -> Result<Self, String> {
        let parts: Vec<_> = data.split_whitespace().collect();
        if parts.len() != 4 && parts.len() != 5 {
            return Err("无效设置".into());
        }
        let key = parts[0].parse().map_err(|_| "无效按键")?;
        let modifiers = parts[1].parse().map_err(|_| "无效修饰键")?;
        let button = parts[2].parse().map_err(|_| "无效侧键")?;
        let radial = if parts[3] == "-" && parts.len() == 4 {
            None
        } else {
            Some(Shortcut {
                key: parts[3].parse().map_err(|_| "无效圆盘键")?,
                modifiers: parts
                    .get(4)
                    .ok_or("缺少修饰键")?
                    .parse()
                    .map_err(|_| "无效修饰键")?,
            })
        };
        Self {
            switch: Shortcut { key, modifiers },
            button,
            radial,
        }
        .validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_roundtrip_and_rejection() {
        let c = Config::parse("48 524288 7 62 0").unwrap();
        assert_eq!(c.switch.label(), "⌥⇥");
        assert_eq!(c.radial.unwrap().label(), "右 ⌃");
        assert!(Config::parse("48 0 7 -").is_err());
        assert!(Config::parse("48 262144 7 48 262144").is_err());
        assert!(Config::parse("48 262144 7 62 0").is_err());
        assert!(Config::parse("50 524288 2 -").is_err());
        assert!(Config::parse("garbage").is_err());
    }
    #[test]
    fn reverse_modifier_and_extra_modifiers() {
        let s = Config::default().switch;
        assert!(s.matches(50, OPTION | SHIFT, true));
        assert!(!s.matches(50, OPTION | CONTROL, true));
    }
}
