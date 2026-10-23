use crate::shapes::{Circle, Shpae, Square};

/// One line describing both shapes.
pub fn describe(c: &Circle, s: &Square) -> String {
    format!("circle {} / square {}", c.area(), s.area())
}

/// Sum of both perimeters.
pub fn perimeters(c: &Circle, s: &Square) -> f64 {
    c.perimeter() + s.perimeter()
}

/// The larger of the two areas.
pub fn biggest(c: &Circle, s: &Square) -> f64 {
    c.area().max(s.area())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_report() {
        let c = Circle { r: 1.0 };
        let s = Square { s: 2.0 };
        assert_eq!(perimeters(&c, &s), 14.0);
        assert_eq!(biggest(&c, &s), 4.0);
        assert_eq!(describe(&c, &s), "circle 3 / square 4");
    }
}
