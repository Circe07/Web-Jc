/// Appends a `total: N` line to `items` and returns the longest name
/// that was in the list before the summary was added.
pub fn record_summary(items: &mut Vec<String>) -> Option<String> {
    let longest = items.iter().max_by_key(|s| s.len()).cloned();
    let count = items.len();
    items.push(format!("total: {count}"));
    longest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_total_and_returns_longest() {
        let mut items = vec!["tea".to_string(), "coffee".to_string(), "milk".to_string()];
        let longest = record_summary(&mut items);
        assert_eq!(longest.as_deref(), Some("coffee"));
        assert_eq!(items.last().map(String::as_str), Some("total: 3"));
    }

    #[test]
    fn empty_list_has_no_longest() {
        let mut items = Vec::new();
        assert_eq!(record_summary(&mut items), None);
    }
}
