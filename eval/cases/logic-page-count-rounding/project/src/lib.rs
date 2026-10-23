/// Number of pages needed to show `total` items, `per_page` at a time.
pub fn page_count(total: usize, per_page: usize) -> usize {
    total / per_page
}

/// Index range of the items on a 0-based page.
pub fn page_range(page: usize, per_page: usize, total: usize) -> std::ops::Range<usize> {
    let start = (page * per_page).min(total);
    let end = (start + per_page).min(total);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_multiple() {
        assert_eq!(page_count(30, 10), 3);
    }

    #[test]
    fn partial_last_page() {
        assert_eq!(page_count(25, 10), 3);
    }

    #[test]
    fn empty_has_no_pages() {
        assert_eq!(page_count(0, 10), 0);
    }
}
