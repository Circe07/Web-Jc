/// Returns the canonical (trimmed, lowercase) form of a user name.
pub fn canonical_name(name: &str) -> String {
    name.trim().to_lowercase()
}

/// True when two names refer to the same user.
pub fn same_user(a: &str, b: &str) -> bool {
    canonical_name(a) == canonical_name(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes() {
        assert_eq!(canonical_name("  Alice "), "alice");
    }

    #[test]
    fn compares_users() {
        assert!(same_user("BOB", " bob"));
        assert!(!same_user("bob", "rob"));
    }
}
