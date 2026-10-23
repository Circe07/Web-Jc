/// Largest sum of `size` consecutive values, or `None` when there is no window.
pub fn max_window_sum(data: &[i64], size: usize) -> Option<i64> {
    if size == 0 || size > data.len() {
        return None;
    }
    let mut best = i64::MIN;
    for start in 0..data.len() - size {
        let sum: i64 = data[start..start + size].iter().sum();
        best = best.max(sum);
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_window_in_the_middle() {
        assert_eq!(max_window_sum(&[1, 9, 9, 1, 1], 2), Some(18));
    }

    #[test]
    fn best_window_at_the_end() {
        assert_eq!(max_window_sum(&[1, 1, 1, 5, 9], 2), Some(14));
    }

    #[test]
    fn window_larger_than_data() {
        assert_eq!(max_window_sum(&[1, 2], 3), None);
    }
}
