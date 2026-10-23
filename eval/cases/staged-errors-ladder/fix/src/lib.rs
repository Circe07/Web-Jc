//! Inventory summary: totals, low-stock alerts and a text report.

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub name: String,
    pub quantity: u32,
    pub unit_price_cents: u64,
}

/// Total stock value in cents.
pub fn total_value(items: &[Item]) -> u64 {
    items
        .iter()
        .map(|i| u64::from(i.quantity) * i.unit_price_cents)
        .sum()
}

/// Names of the items whose quantity is at or below `threshold`.
pub fn low_stock(items: &[Item], threshold: u32) -> Vec<&str> {
    items
        .iter()
        .filter(|i| i.quantity <= threshold)
        .map(|i| i.name.as_str())
        .collect()
}

/// One line per item, `name: quantity x price`, prices as euros with two decimals.
pub fn report(items: &[Item]) -> String {
    let mut lines = Vec::new();
    for item in items {
        let euros = item.unit_price_cents / 100;
        let cents = item.unit_price_cents % 100;
        lines.push(format!("{}: {} x {}.{:02}", item.name, item.quantity, euros, cents));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stock() -> Vec<Item> {
        vec![
            Item { name: "bolt".into(), quantity: 120, unit_price_cents: 15 },
            Item { name: "nut".into(), quantity: 3, unit_price_cents: 5 },
            Item { name: "gear".into(), quantity: 7, unit_price_cents: 1250 },
        ]
    }

    #[test]
    fn totals() {
        assert_eq!(total_value(&stock()), 120 * 15 + 3 * 5 + 7 * 1250);
    }

    #[test]
    fn low() {
        assert_eq!(low_stock(&stock(), 7), vec!["nut", "gear"]);
    }

    #[test]
    fn formats_report() {
        assert_eq!(report(&stock()), "bolt: 120 x 0.15\nnut: 3 x 0.05\ngear: 7 x 12.50");
    }
}
