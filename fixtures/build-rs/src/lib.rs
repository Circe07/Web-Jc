/// Value set by `build.rs`.
pub fn built() -> &'static str {
    env!("ORQ_FIXTURE_BUILT")
}

#[cfg(test)]
mod tests {
    #[test]
    fn built_by_script() {
        assert_eq!(super::built(), "yes");
    }
}
