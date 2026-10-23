use std::collections::HashMap;

/// Price list in cents.
#[derive(Default)]
pub struct Catalog {
    prices: HashMap<String, u32>,
}

impl Catalog {
    /// Sets the price of an item.
    pub fn add(&mut self, sku: &str, cents: u32) {
        self.prices.insert(sku.to_string(), cents);
    }

    /// Sum of the prices of `cart`.
    pub fn total(&self, cart: &[&str]) -> u32 {
        cart.iter()
            .filter_map(|sku| self.prices.get(*sku))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        let mut c = Catalog::default();
        c.add("tea", 300);
        c.add("cake", 450);
        c
    }

    #[test]
    fn totals_known_items() {
        assert_eq!(catalog().total(&["tea", "cake"]), 750);
    }

    #[test]
    fn unknown_items_are_free() {
        assert_eq!(catalog().total(&["tea", "unicorn"]), 300);
    }
}
