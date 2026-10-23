use std::fmt;

/// A semantic version.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Release {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl fmt::Display for Release {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Returns the newest release.
pub fn latest(mut releases: Vec<Release>) -> Option<Release> {
    releases.sort();
    releases.pop()
}

/// Formats a release for display.
pub fn label(r: &Release) -> String {
    format!("v{}", r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_latest() {
        let rs = vec![
            Release { major: 1, minor: 9, patch: 0 },
            Release { major: 2, minor: 0, patch: 1 },
            Release { major: 1, minor: 10, patch: 3 },
        ];
        assert_eq!(latest(rs).map(|r| label(&r)), Some("v2.0.1".to_string()));
    }
}
