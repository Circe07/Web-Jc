pub fn is_empty_len(v: &Vec<u32>) -> bool {
    return v.len() == 0;
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty() {
        assert!(super::is_empty_len(&vec![]));
    }
}
