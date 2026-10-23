/// Scores a string; higher is better.
pub trait Scorer {
    fn score(&self, item: &str) -> u32;
}

/// Scores by length.
pub struct LengthScorer;

impl Scorer for LengthScorer {
    fn score(&self, item: &str) -> u32 {
        item.len() as u32
    }
}

/// Scores 1 for strings that start with a vowel.
pub struct VowelScorer;

impl Scorer for VowelScorer {
    fn score(&self, item: &str) -> u32 {
        u32::from(item.starts_with(['a', 'e', 'i', 'o', 'u']))
    }
}

/// Returns the item with the highest score (the first one on a tie).
pub fn best_item<T: Clone + AsRef<str>>(items: &[T], scorer: &dyn Scorer) -> Option<T> {
    let mut best: Option<&T> = None;
    let mut best_score = 0;
    for item in items {
        let s = scorer.score(item.as_ref());
        if best.is_none() || s > best_score {
            best = Some(item);
            best_score = s;
        }
    }
    best.cloned()
}

/// A set of scorers whose results are added up.
pub struct Registry {
    scorers: Vec<Box<dyn Scorer>>,
}

impl Registry {
    /// The default set: length and vowel scorers.
    pub fn standard() -> Self {
        Registry {
            scorers: vec![Box::new(LengthScorer), Box::new(VowelScorer)],
        }
    }

    /// Sum of all scores for `item`.
    pub fn total(&self, item: &str) -> u32 {
        self.scorers.iter().map(|s| s.score(item)).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_best_by_length() {
        let words = ["a", "abcd", "abc"];
        assert_eq!(best_item(&words, &LengthScorer), Some("abcd"));
    }

    #[test]
    fn registry_scores_with_every_scorer() {
        let r = Registry::standard();
        assert_eq!(r.total("ab"), 2 + 1);
    }
}
