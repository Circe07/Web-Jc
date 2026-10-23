/// Doubles a number using `base`.
pub fn double(x: u64) -> u64 {
    base::add(x, x)
}

#[cfg(test)]
mod tests {
    #[test]
    fn doubles() {
        assert_eq!(super::double(4), 8);
    }
}
