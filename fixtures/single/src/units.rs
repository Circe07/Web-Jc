/// Centimetres to metres.
pub fn cm_to_m(cm: f64) -> f64 {
    cm / 100.0
}

#[cfg(test)]
mod tests {
    #[test]
    fn converts() {
        assert_eq!(super::cm_to_m(250.0), 2.5);
    }
}
