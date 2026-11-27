use crate::geometry::Rect;
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug)]
pub struct SpringParams {
    pub stiffness: f64,
    pub damping: f64,
}

pub const SPRING_SNAPPY: SpringParams = SpringParams {
    stiffness: 520.0,
    damping: 38.0,
};

pub const SPRING_FLUID: SpringParams = SpringParams {
    stiffness: 380.0,
    damping: 31.0,
};

pub const SPRING_POP: SpringParams = SpringParams {
    stiffness: 460.0,
    damping: 33.0,
};

pub const SPRING_GLOW: SpringParams = SpringParams {
    stiffness: 320.0,
    damping: 28.0,
};

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f64,
    pub velocity: f64,
    pub target: f64,
}

impl Spring {
    pub fn new(initial: f64) -> Self {
        Self {
            value: initial,
            velocity: 0.0,
            target: initial,
        }
    }

    pub fn snap(&mut self, value: f64) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    pub fn set_target(&mut self, target: f64) {
        self.target = target;
    }

    pub fn step(&mut self, dt: f64, params: SpringParams, epsilon: f64) -> bool {
        let dt = dt.clamp(1.0 / 240.0, 1.0 / 30.0);
        // Sub-step twice for rock-solid numerical stability at lower refresh rates.
        let half = dt * 0.5;
        for _ in 0..2 {
            let accel =
                (self.target - self.value) * params.stiffness - self.velocity * params.damping;
            self.velocity += accel * half;
            self.value += self.velocity * half;
        }
        if (self.target - self.value).abs() <= epsilon && self.velocity.abs() <= epsilon * 8.0 {
            self.value = self.target;
            self.velocity = 0.0;
            false
        } else {
            true
        }
    }

    pub fn is_active(&self, epsilon: f64) -> bool {
        (self.target - self.value).abs() > epsilon || self.velocity.abs() > epsilon * 8.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SpringRect {
    pub x: Spring,
    pub y: Spring,
    pub w: Spring,
    pub h: Spring,
}

impl SpringRect {
    pub fn new(rect: Rect) -> Self {
        Self {
            x: Spring::new(rect.x),
            y: Spring::new(rect.y),
            w: Spring::new(rect.w),
            h: Spring::new(rect.h),
        }
    }

    pub fn snap(&mut self, rect: Rect) {
        self.x.snap(rect.x);
        self.y.snap(rect.y);
        self.w.snap(rect.w);
        self.h.snap(rect.h);
    }

    pub fn set_target(&mut self, rect: Rect) {
        self.x.set_target(rect.x);
        self.y.set_target(rect.y);
        self.w.set_target(rect.w);
        self.h.set_target(rect.h);
    }

    pub fn current(&self) -> Rect {
        Rect {
            x: self.x.value,
            y: self.y.value,
            w: self.w.value.max(1.0),
            h: self.h.value.max(1.0),
        }
    }

    pub fn step(&mut self, dt: f64, params: SpringParams) -> bool {
        let a = self.x.step(dt, params, 0.2);
        let b = self.y.step(dt, params, 0.2);
        let c = self.w.step(dt, params, 0.2);
        let d = self.h.step(dt, params, 0.2);
        a || b || c || d
    }

    pub fn is_active(&self) -> bool {
        self.x.is_active(0.2)
            || self.y.is_active(0.2)
            || self.w.is_active(0.2)
            || self.h.is_active(0.2)
    }
}

pub fn shortest_angle_delta(from: f64, to: f64) -> f64 {
    let mut delta = (to - from).rem_euclid(2.0 * PI);
    if delta > PI {
        delta -= 2.0 * PI;
    }
    delta
}

#[derive(Clone, Copy, Debug)]
pub struct SpringAngle {
    pub value: f64,
    pub velocity: f64,
    pub target: f64,
}

impl SpringAngle {
    pub fn new(initial: f64) -> Self {
        Self {
            value: initial,
            velocity: 0.0,
            target: initial,
        }
    }

    pub fn snap(&mut self, angle: f64) {
        self.value = angle;
        self.target = angle;
        self.velocity = 0.0;
    }

    pub fn set_target(&mut self, target: f64) {
        self.target = target;
    }

    pub fn step(&mut self, dt: f64, params: SpringParams) -> bool {
        let dt = dt.clamp(1.0 / 240.0, 1.0 / 30.0);
        let half = dt * 0.5;
        for _ in 0..2 {
            let delta = shortest_angle_delta(self.value, self.target);
            let accel = delta * params.stiffness - self.velocity * params.damping;
            self.velocity += accel * half;
            self.value += self.velocity * half;
        }
        let remaining = shortest_angle_delta(self.value, self.target).abs();
        if remaining <= 0.004 && self.velocity.abs() <= 0.03 {
            self.value = self.target;
            self.velocity = 0.0;
            false
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_converges_cleanly() {
        let mut s = Spring::new(0.0);
        s.set_target(100.0);
        let mut active = true;
        for _ in 0..120 {
            active = s.step(1.0 / 120.0, SPRING_SNAPPY, 0.1);
            if !active {
                break;
            }
        }
        assert!(!active);
        assert_eq!(s.value, 100.0);
    }

    #[test]
    fn spring_rect_morphs_to_target() {
        let start = Rect {
            x: 0.0,
            y: 0.0,
            w: 500.0,
            h: 400.0,
        };
        let end = Rect {
            x: 800.0,
            y: 40.0,
            w: 800.0,
            h: 900.0,
        };
        let mut sr = SpringRect::new(start);
        sr.set_target(end);
        for _ in 0..180 {
            if !sr.step(1.0 / 120.0, SPRING_FLUID) {
                break;
            }
        }
        assert!(!sr.is_active());
        assert_eq!(sr.current(), end);
    }

    #[test]
    fn spring_angle_takes_shortest_arc_across_wrap() {
        let mut a = SpringAngle::new(PI - 0.1);
        a.set_target(-PI + 0.1);
        // Should move positively (+0.2 rad), not backwards (-6.08 rad).
        a.step(1.0 / 60.0, SPRING_SNAPPY);
        assert!(a.velocity > 0.0);
        for _ in 0..120 {
            if !a.step(1.0 / 120.0, SPRING_SNAPPY) {
                break;
            }
        }
        assert!(shortest_angle_delta(a.value, -PI + 0.1).abs() < 1e-6);
    }
}
