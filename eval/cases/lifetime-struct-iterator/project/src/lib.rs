//! A zero-copy tokenizer for `key=value; key=value` lists.

/// One `key=value` pair borrowed from the input.
#[derive(Debug, PartialEq, Eq)]
pub struct Pair {
    pub key: &str,
    pub value: &str,
}

/// Iterates over the pairs of the input, skipping empty segments.
pub struct Pairs {
    rest: &str,
}

impl Pairs {
    pub fn new(input: &str) -> Self {
        Pairs { rest: input }
    }
}

impl Iterator for Pairs {
    type Item = Pair;

    fn next(&mut self) -> Option<Pair> {
        loop {
            if self.rest.is_empty() {
                return None;
            }
            let (segment, rest) = match self.rest.find(';') {
                Some(i) => (&self.rest[..i], &self.rest[i + 1..]),
                None => (self.rest, ""),
            };
            self.rest = rest;
            let segment = segment.trim();
            if segment.is_empty() {
                continue;
            }
            let (key, value) = segment.split_once('=').unwrap_or((segment, ""));
            return Some(Pair {
                key: key.trim(),
                value: value.trim(),
            });
        }
    }
}

/// Value of the first pair named `key`.
pub fn lookup(input: &str, key: &str) -> Option<&str> {
    Pairs::new(input).find(|p| p.key == key).map(|p| p.value)
}

/// The longer of two values (the first one on a tie).
pub fn longer(a: &str, b: &str) -> &str {
    if b.len() > a.len() {
        b
    } else {
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iterates() {
        let pairs: Vec<Pair> = Pairs::new("a=1; ;b = two;c").collect();
        assert_eq!(
            pairs,
            vec![
                Pair {
                    key: "a",
                    value: "1"
                },
                Pair {
                    key: "b",
                    value: "two"
                },
                Pair {
                    key: "c",
                    value: ""
                },
            ]
        );
    }

    #[test]
    fn looks_up() {
        let owned = String::from("host=example.org; port=8080");
        let port = {
            let key = String::from("port");
            lookup(&owned, &key)
        };
        assert_eq!(port, Some("8080"));
        assert_eq!(longer("ab", "abc"), "abc");
    }
}
