//! Shared log-level parsing.

/// Verbosity levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
}

/// The text was not a known level.
#[derive(Debug, PartialEq, Eq)]
pub struct UnknownLevel(pub String);

/// Parses a level name (case-insensitive).
pub fn parse_level(text: &str) -> Result<Level, UnknownLevel> {
    match text.to_ascii_lowercase().as_str() {
        "debug" => Ok(Level::Debug),
        "info" => Ok(Level::Info),
        "warn" => Ok(Level::Warn),
        other => Err(UnknownLevel(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names() {
        assert_eq!(parse_level("WARN"), Ok(Level::Warn));
        assert!(parse_level("loud").is_err());
    }
}
