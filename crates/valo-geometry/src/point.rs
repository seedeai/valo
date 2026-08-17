/// `Point` is a position or vector in two-dimensional coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Point {
    /// `x` is the horizontal component.
    pub x: f32,
    /// `y` is the vertical component.
    pub y: f32,
}

impl Point {
    /// `ZERO` is the origin.
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };

    /// `new` creates a point from its components.
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl From<(f32, f32)> for Point {
    fn from((x, y): (f32, f32)) -> Self {
        Self { x, y }
    }
}

impl std::ops::Add for Point {
    type Output = Point;
    fn add(self, o: Point) -> Point {
        Point::new(self.x + o.x, self.y + o.y)
    }
}

impl std::ops::Sub for Point {
    type Output = Point;
    fn sub(self, o: Point) -> Point {
        Point::new(self.x - o.x, self.y - o.y)
    }
}

/// `Size` is a width and height in two-dimensional coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Size {
    /// `width` is the horizontal extent.
    pub width: f32,
    /// `height` is the vertical extent.
    pub height: f32,
}

impl Size {
    /// `new` creates a size from its extents.
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    /// `is_empty` reports whether either extent is nonpositive.
    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
}
