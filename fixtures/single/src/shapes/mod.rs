/// A rectangle.
pub struct Rect {
    pub w: f64,
    pub h: f64,
}

impl Rect {
    /// Area of the rectangle.
    pub fn area(&self) -> f64 {
        self.w * self.h
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn area() {
        let r = super::Rect { w: 2.0, h: 3.0 };
        assert_eq!(r.area(), 6.0);
    }
}
