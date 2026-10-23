/// Sum of all values, each multiplied by `factor`.
pub fn scaled_sum(values: &[f64], factor: f64) -> f64 {
    let mut sum = 0.0;
    for i in 0..values.len() {
        sum += values[i] * factor;
    }
    sum
}

/// Mean of the scaled values (0.0 for an empty slice).
pub fn scaled_mean(values: &[f64], factor: f64) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        scaled_sum(values, factor) / values.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_and_sums() {
        assert_eq!(scaled_sum(&[1.0, 2.0, 3.0], 2.0), 12.0);
    }

    #[test]
    fn mean_of_scaled() {
        assert_eq!(scaled_mean(&[2.0, 4.0], 0.5), 1.5);
        assert_eq!(scaled_mean(&[], 3.0), 0.0);
    }
}
