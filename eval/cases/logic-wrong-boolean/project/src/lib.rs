/// True when `year` is a leap year in the Gregorian calendar.
pub fn is_leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && !year.is_multiple_of(100) && year.is_multiple_of(400)
}

/// Days in a February of the given year.
pub fn february_days(year: u32) -> u32 {
    if is_leap_year(year) {
        29
    } else {
        28
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_leap_year() {
        assert!(is_leap_year(2024));
    }

    #[test]
    fn common_year() {
        assert!(!is_leap_year(2023));
    }

    #[test]
    fn century_is_not_leap() {
        assert!(!is_leap_year(1900));
    }

    #[test]
    fn divisible_by_400_is_leap() {
        assert!(is_leap_year(2000));
    }
}
