//! Generic helpers over collections of labels.

use std::collections::HashMap;
use std::fmt::Display;
use std::hash::Hash;

/// The most frequent value; ties go to the smallest value.
pub fn mode<T: Clone + Eq + Hash + Ord>(values: &[T]) -> Option<T> {
    let mut counts: HashMap<T, usize> = HashMap::new();
    for v in values {
        *counts.entry(v.clone()).or_insert(0) += 1;
    }
    let best = counts.values().copied().max()?;
    counts
        .into_iter()
        .filter(|(_, n)| *n == best)
        .map(|(v, _)| v)
        .min()
}

/// Values sorted ascending with duplicates removed.
pub fn unique_sorted<T: Clone + Ord>(values: &[T]) -> Vec<T> {
    let mut out = values.to_vec();
    out.sort();
    out.dedup();
    out
}

/// `a, b, c` rendering of the values.
pub fn join<T: Display>(values: &[T]) -> String {
    values
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Largest value, rendered, or `"-"` when empty.
pub fn describe_max<T: Display + Ord>(values: &[T]) -> String {
    match values.iter().max() {
        Some(v) => format!("max={v}"),
        None => "-".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert_eq!(mode(&["b", "a", "b", "a", "c"]), Some("a"));
        assert_eq!(mode::<u8>(&[]), None);
    }

    #[test]
    fn uniques() {
        assert_eq!(unique_sorted(&[3, 1, 3, 2, 1]), vec![1, 2, 3]);
    }

    #[test]
    fn joins() {
        assert_eq!(join(&[1, 2, 3]), "1, 2, 3");
        assert_eq!(describe_max(&[4, 9, 2]), "max=9");
        assert_eq!(describe_max::<i32>(&[]), "-");
    }
}
