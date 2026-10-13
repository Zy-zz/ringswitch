#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Layout {
    Maximize,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    TopLeft,
    Top,
    TopRight,
}

impl Layout {
    pub const DIRECTIONS: [Self; 8] = [
        Self::Right,
        Self::BottomRight,
        Self::Bottom,
        Self::BottomLeft,
        Self::Left,
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Maximize => "最大化",
            Self::Right => "右半屏",
            Self::BottomRight => "右下角",
            Self::Bottom => "下半屏",
            Self::BottomLeft => "左下角",
            Self::Left => "左半屏",
            Self::TopLeft => "左上角",
            Self::Top => "上半屏",
            Self::TopRight => "右上角",
        }
    }

    pub fn rect(self, screen: Rect) -> Rect {
        let (x, y, w, h) = match self {
            Self::Maximize => (0., 0., 1., 1.),
            Self::Right => (0.5, 0., 0.5, 1.),
            Self::BottomRight => (0.5, 0.5, 0.5, 0.5),
            Self::Bottom => (0., 0.5, 1., 0.5),
            Self::BottomLeft => (0., 0.5, 0.5, 0.5),
            Self::Left => (0., 0., 0.5, 1.),
            Self::TopLeft => (0., 0., 0.5, 0.5),
            Self::Top => (0., 0., 1., 0.5),
            Self::TopRight => (0.5, 0., 0.5, 0.5),
        };
        Rect {
            x: screen.x + screen.w * x,
            y: screen.y + screen.h * y,
            w: screen.w * w,
            h: screen.h * h,
        }
    }
}

// Event coordinates have a top-left origin. A small dead zone prevents a click resizing.
pub fn selection(dx: f64, dy: f64, armed: bool) -> Option<Layout> {
    let distance = dx.hypot(dy);
    if distance < 28. {
        return armed.then_some(Layout::Maximize);
    }
    let sector =
        ((dy.atan2(dx) / std::f64::consts::FRAC_PI_4).round() as i32).rem_euclid(8) as usize;
    Some(Layout::DIRECTIONS[sector])
}

pub fn cycle(index: usize, len: usize, backwards: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if backwards {
        (index + len - 1) % len
    } else {
        (index + 1) % len
    }
}

pub fn cocoa_to_ax(rect: Rect, primary_height: f64) -> Rect {
    Rect {
        y: primary_height - rect.y - rect.h,
        ..rect
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_compass_directions_and_dead_zone() {
        for (i, layout) in Layout::DIRECTIONS.iter().enumerate() {
            let a = i as f64 * std::f64::consts::FRAC_PI_4;
            assert_eq!(
                selection(a.cos() * 90., a.sin() * 90., false),
                Some(*layout)
            );
        }
        assert_eq!(selection(1., 1., false), None);
        assert_eq!(selection(1., 1., true), Some(Layout::Maximize));
    }

    #[test]
    fn halves_and_quarters_cover_offset_screen() {
        let s = Rect {
            x: -1728.,
            y: -900.,
            w: 1728.,
            h: 1080.,
        };
        let top = Layout::Top.rect(s);
        let bottom = Layout::Bottom.rect(s);
        assert_eq!(top.h + bottom.h, s.h);
        assert_eq!(bottom.y, top.y + top.h);
        assert_eq!(
            Layout::BottomRight.rect(s),
            Rect {
                x: -864.,
                y: -360.,
                w: 864.,
                h: 540.
            }
        );
        assert_eq!(Layout::Maximize.rect(s), s);
    }

    #[test]
    fn cocoa_conversion_handles_above_and_left_displays() {
        let s = Rect {
            x: -1000.,
            y: 1000.,
            w: 1000.,
            h: 800.,
        };
        assert_eq!(cocoa_to_ax(s, 1000.).y, -800.);
    }

    #[test]
    fn cycling_wraps_and_handles_no_windows() {
        assert_eq!(cycle(0, 0, false), 0);
        assert_eq!(cycle(0, 1, true), 0);
        assert_eq!(cycle(2, 3, false), 0);
        assert_eq!(cycle(0, 3, true), 2);
    }
}
