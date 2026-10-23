//! Word tally.

use std::collections::HashMap;

/// Case-insensitive word counts.
#[derive(Debug, Default)]
pub struct Tally {
    counts: HashMap<String, usize>,
}

impl Tally {
    pub fn add(&mut self, word: &str) {
        *self.counts.entry(word.to_lowercase()).or_insert(0) += 1;
    }

    pub fn get(&self, word: &str) -> usize {
        self.counts.get(&word.to_lowercase()).copied().unwrap_or(0)
    }

    pub fn total(&self) -> usize {
        self.counts.values().sum()
    }
}
