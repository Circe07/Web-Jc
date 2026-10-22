/// Greets someone. Independent of every other crate.
pub fn greet(name: &str) -> String {
    format!("hello, {name}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn greets() {
        assert_eq!(super::greet("orq"), "hello, orq");
    }
}
