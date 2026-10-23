use base::{parse_level, Level};

/// Level named by the user, or `Info` when the name is not recognised.
pub fn level_or_default(name: &str) -> Level {
    match parse_level(name) {
        Some(level) => level,
        None => Level::Info,
    }
}

/// True when the named level is `debug`.
pub fn is_verbose(name: &str) -> bool {
    if let Some(level) = parse_level(name) {
        return level == Level::Debug;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_info() {
        assert_eq!(level_or_default("warn"), Level::Warn);
        assert_eq!(level_or_default("nonsense"), Level::Info);
    }

    #[test]
    fn verbosity_flags() {
        assert!(is_verbose("debug"));
        assert!(!is_verbose("warn"));
    }
}
