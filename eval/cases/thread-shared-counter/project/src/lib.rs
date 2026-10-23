//! Counts words in parallel, one thread per chunk of lines.

pub mod tally;

use std::cell::RefCell;
use std::rc::Rc;
use std::thread;

use tally::Tally;

/// Word counts for `text`, split across `workers` threads.
pub fn count_words(text: &str, workers: usize) -> Tally {
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let chunk = lines.len().div_ceil(workers.max(1)).max(1);
    let shared = Rc::new(RefCell::new(Tally::default()));

    let mut handles = Vec::new();
    for part in lines.chunks(chunk) {
        let part = part.to_vec();
        let shared = Rc::clone(&shared);
        handles.push(thread::spawn(move || {
            for line in &part {
                for word in line.split_whitespace() {
                    shared.borrow_mut().add(word);
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    Rc::try_unwrap(shared).unwrap().into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_across_threads() {
        let text = "a b a\nc a\nb\n\na c c";
        let t = count_words(text, 3);
        assert_eq!(t.get("a"), 4);
        assert_eq!(t.get("b"), 2);
        assert_eq!(t.get("c"), 3);
        assert_eq!(t.total(), 9);
    }

    #[test]
    fn one_worker() {
        assert_eq!(count_words("x y x", 1).get("x"), 2);
    }
}
