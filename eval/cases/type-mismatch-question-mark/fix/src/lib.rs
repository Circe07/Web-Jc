/// Errors produced while reading settings.
#[derive(Debug, PartialEq)]
pub enum ConfigError {
    /// The port is not a valid number.
    BadPort(String),
    /// A required key is absent.
    Missing(&'static str),
}

impl From<std::num::ParseIntError> for ConfigError {
    fn from(err: std::num::ParseIntError) -> Self {
        ConfigError::BadPort(err.to_string())
    }
}

/// Settings read from `key=value` lines.
#[derive(Debug, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
}

/// Parses `host=...` and `port=...` lines; unknown keys are ignored.
pub fn parse_settings(input: &str) -> Result<Settings, ConfigError> {
    let mut host = None;
    let mut port = None;
    for line in input.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "host" => host = Some(value.trim().to_string()),
            "port" => port = Some(value.trim().parse::<u16>()?),
            _ => {}
        }
    }
    Ok(Settings {
        host: host.ok_or(ConfigError::Missing("host"))?,
        port: port.ok_or(ConfigError::Missing("port"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_input() {
        let s = parse_settings("host = example.org\nport = 8080").unwrap();
        assert_eq!(s.port, 8080);
    }

    #[test]
    fn rejects_bad_port() {
        let err = parse_settings("host=a\nport=eighty").unwrap_err();
        assert_eq!(err, ConfigError::BadPort("invalid digit found in string".into()));
    }
}
