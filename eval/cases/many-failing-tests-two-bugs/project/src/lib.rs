//! Durations written as `1h30m15s` and rendered back.

/// Seconds per unit suffix.
fn unit_seconds(unit: char) -> Option<u64> {
    match unit {
        'h' => Some(3600),
        'm' => Some(600),
        's' => Some(1),
        _ => None,
    }
}

/// Parses `1h30m`, `45s`, `2h` ... into seconds. `None` on malformed input.
pub fn parse(text: &str) -> Option<u64> {
    let mut total = 0;
    let mut digits = String::new();
    for c in text.trim().chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            let n: u64 = digits.parse().ok()?;
            total += n * unit_seconds(c)?;
            digits.clear();
        }
    }
    digits.is_empty().then_some(total)
}

/// Splits seconds into (hours, minutes, seconds).
fn split(secs: u64) -> (u64, u64, u64) {
    (secs / 3600, secs / 60, secs % 60)
}

/// Renders seconds in the shortest `1h30m15s` form; `0s` for zero.
pub fn render(secs: u64) -> String {
    if secs == 0 {
        return "0s".to_string();
    }
    let (h, m, s) = split(secs);
    let mut out = String::new();
    for (n, unit) in [(h, 'h'), (m, 'm'), (s, 's')] {
        if n > 0 {
            out.push_str(&format!("{n}{unit}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hours() {
        assert_eq!(parse("2h"), Some(7200));
    }

    #[test]
    fn parses_minutes() {
        assert_eq!(parse("15m"), Some(900));
    }

    #[test]
    fn parses_mixed() {
        assert_eq!(parse("1h30m15s"), Some(5415));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse("10x"), None);
        assert_eq!(parse("10"), None);
        assert_eq!(parse("h"), None);
    }

    #[test]
    fn renders_minutes() {
        assert_eq!(render(900), "15m");
    }

    #[test]
    fn renders_mixed() {
        assert_eq!(render(5415), "1h30m15s");
    }

    #[test]
    fn renders_zero() {
        assert_eq!(render(0), "0s");
    }

    #[test]
    fn round_trips() {
        for text in ["1h", "59m59s", "3h1s", "12m"] {
            assert_eq!(render(parse(text).unwrap()), text, "{text}");
        }
    }
}
