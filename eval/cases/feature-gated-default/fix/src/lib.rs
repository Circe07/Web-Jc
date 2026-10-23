/// Order-sensitive checksum of a byte slice.
pub fn checksum(data: &[u8]) -> u32 {
    checksum_impl(data)
}

#[cfg(feature = "fast")]
fn checksum_impl(data: &[u8]) -> u32 {
    data.iter()
        .fold(0u32, |acc, b| acc.rotate_left(5) ^ u32::from(*b))
}

#[cfg(not(feature = "fast"))]
fn checksum_impl(data: &[u8]) -> u32 {
    let mut acc = 0u32;
    for b in data {
        acc = acc.rotate_left(5) ^ u32::from(*b);
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_is_stable() {
        assert_eq!(checksum(b"orq"), checksum(b"orq"));
        assert_ne!(checksum(b"orq"), checksum(b"qro"));
    }
}
