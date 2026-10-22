/// Quadruples a number using `mid`.
pub fn quadruple(x: u64) -> u64 {
    mid::double(mid::double(x))
}

#[cfg(test)]
mod tests {
    #[test]
    fn quadruples() {
        assert_eq!(super::quadruple(3), 12);
    }
}
