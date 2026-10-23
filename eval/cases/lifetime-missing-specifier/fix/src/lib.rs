/// A word borrowed from a source string.
#[derive(Debug)]
pub struct Token<'a> {
    pub text: &'a str,
}

/// Returns the token with more characters (the first one on a tie).
pub fn longest_token<'a>(a: &'a Token<'a>, b: &'a Token<'a>) -> &'a Token<'a> {
    if b.text.len() > a.text.len() {
        b
    } else {
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_longer_token() {
        let a = Token { text: "if" };
        let b = Token { text: "while" };
        assert_eq!(longest_token(&a, &b).text, "while");
    }
}
