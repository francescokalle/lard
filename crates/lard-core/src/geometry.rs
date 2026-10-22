//! Small, allocation-free geometry types (logical pixels).

use std::ops::{Add, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            x: self.x + (other.x - self.x) * t,
            y: self.y + (other.y - self.y) * t,
        }
    }
}

impl Add for Point {
    type Output = Point;
    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Point {
    type Output = Point;
    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

impl Size {
    #[must_use]
    pub const fn new(w: f32, h: f32) -> Self {
        Self { w, h }
    }

    #[must_use]
    pub const fn splat(v: f32) -> Self {
        Self::new(v, v)
    }

    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self::new(
            self.w + (other.w - self.w) * t,
            self.h + (other.h - self.h) * t,
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0, 0.0);

    #[must_use]
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    #[must_use]
    pub fn from_point_size(origin: Point, size: Size) -> Self {
        Self::new(origin.x, origin.y, size.w, size.h)
    }

    #[must_use]
    pub fn centered(center: Point, size: Size) -> Self {
        Self::new(
            center.x - size.w * 0.5,
            center.y - size.h * 0.5,
            size.w,
            size.h,
        )
    }

    #[must_use]
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    #[must_use]
    pub fn center(&self) -> Point {
        Point::new(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    #[must_use]
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.right() && p.y < self.bottom()
    }

    #[must_use]
    pub fn inset(&self, amount: f32) -> Self {
        Self::new(
            self.x + amount,
            self.y + amount,
            (self.w - amount * 2.0).max(0.0),
            (self.h - amount * 2.0).max(0.0),
        )
    }

    #[must_use]
    pub fn inset_xy(&self, x: f32, y: f32) -> Self {
        Self::new(
            self.x + x,
            self.y + y,
            (self.w - x * 2.0).max(0.0),
            (self.h - y * 2.0).max(0.0),
        )
    }

    #[must_use]
    pub fn with_size(&self, size: Size) -> Self {
        Self::new(self.x, self.y, size.w, size.h)
    }

    #[must_use]
    pub fn translated(&self, dx: f32, dy: f32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    /// Scale the rect about its own center (used for Apple-style reveal zoom).
    #[must_use]
    pub fn scale_centered(&self, factor: f32) -> Self {
        let c = self.center();
        Self::new(
            c.x - self.w * factor * 0.5,
            c.y - self.h * factor * 0.5,
            self.w * factor,
            self.h * factor,
        )
    }

    #[must_use]
    pub fn scaled(&self, factor: f32) -> Self {
        Self::new(
            self.x * factor,
            self.y * factor,
            self.w * factor,
            self.h * factor,
        )
    }

    #[must_use]
    pub fn lerp(&self, other: Self, t: f32) -> Self {
        Self::new(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
            self.w + (other.w - self.w) * t,
            self.h + (other.h - self.h) * t,
        )
    }
}
