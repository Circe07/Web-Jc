/// Something with an area and a perimeter.
pub trait Shape {
    /// Surface area.
    fn area(&self) -> f64;
    /// Length of the boundary.
    fn perimeter(&self) -> f64;
}

/// A circle.
pub struct Circle {
    pub r: f64,
}

/// A square.
pub struct Square {
    pub s: f64,
}

impl Shape for Circle {
    fn area(&self) -> f64 {
        3.0 * self.r * self.r
    }
    fn perimeter(&self) -> f64 {
        6.0 * self.r
    }
}

impl Shape for Square {
    fn area(&self) -> f64 {
        self.s * self.s
    }
    fn perimeter(&self) -> f64 {
        4.0 * self.s
    }
}
