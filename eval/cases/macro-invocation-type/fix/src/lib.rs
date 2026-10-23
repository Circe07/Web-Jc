macro_rules! define_settings {
    ($($name:ident : $ty:ty = $default:expr),* $(,)?) => {
        /// Runtime settings.
        pub struct Settings {
            $(pub $name: $ty),*
        }

        impl Default for Settings {
            fn default() -> Self {
                Settings { $($name: $default),* }
            }
        }
    };
}

define_settings! {
    retries: u32 = 3,
    timeout_ms: u64 = 1_500,
    verbose: bool = false,
    name: String = String::from("worker"),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_set() {
        let s = Settings::default();
        assert_eq!(s.retries, 3);
        assert!(!s.verbose);
        assert_eq!(s.name, "worker");
    }
}
