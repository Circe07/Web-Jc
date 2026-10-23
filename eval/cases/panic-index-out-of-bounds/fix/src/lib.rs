/// Differences between each value and the next one.
pub fn deltas(values: &[i32]) -> Vec<i32> {
    values.windows(2).map(|w| w[1] - w[0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn differences_between_neighbours() {
        assert_eq!(deltas(&[1, 4, 9, 16]), vec![3, 5, 7]);
    }

    #[test]
    fn short_input_has_no_deltas() {
        assert!(deltas(&[5]).is_empty());
    }
}
