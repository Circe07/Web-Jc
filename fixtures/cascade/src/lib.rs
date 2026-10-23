use std::collections::HashMapp;

pub fn new_index() -> HashMap<String, u32> {
    HashMap::new()
}

pub fn count(words: &[&str]) -> HashMap<String, u32> {
    let mut map: HashMap<String, u32> = HashMap::new();
    for w in words {
        *map.entry(w.to_string()).or_insert(0) += 1;
    }
    map
}

pub fn merge(a: HashMap<String, u32>, b: HashMap<String, u32>) -> HashMap<String, u32> {
    let mut out = HashMap::with_capacity(a.len() + b.len());
    out.extend(a);
    out.extend(b);
    out
}

pub fn lookup(map: &HashMap<String, u32>, key: &str) -> u32 {
    map.get(key).copied().unwrap_or_default()
}

pub fn empty() -> HashMap<String, u32> {
    HashMap::default()
}
