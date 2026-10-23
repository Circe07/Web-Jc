//! Invoices built from ledger entries.

use ledger::{Entry, Money};

/// An invoice: its lines and a VAT rate in percent.
pub struct Invoice {
    pub lines: Vec<Entry>,
    pub vat_percent: i64,
}

impl Invoice {
    pub fn new(vat_percent: i64) -> Self {
        Invoice {
            lines: Vec::new(),
            vat_percent,
        }
    }

    /// Adds a line of `cents`.
    pub fn add_line(&mut self, description: &str, cents: i64) {
        self.lines.push(Entry::new(description, Money(cents)));
    }

    /// Sum of the lines before VAT.
    pub fn net(&self) -> Money {
        let mut total = Money(0);
        for line in &self.lines {
            total = total + line.amount;
        }
        total
    }

    /// VAT on the net amount, rounded down.
    pub fn vat(&self) -> Money {
        Money(self.net().0 * self.vat_percent / 100)
    }

    /// Net plus VAT.
    pub fn gross(&self) -> Money {
        Money(self.net().0 + self.vat().0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ledger::Currency;

    #[test]
    fn totals() {
        let mut inv = Invoice::new(21);
        inv.add_line("design", 10_000).unwrap();
        inv.add_line("hosting", 2_000).unwrap();
        assert_eq!(inv.net(), Money::new(12_000, Currency::Eur));
        assert_eq!(inv.vat(), Money::new(2_520, Currency::Eur));
        assert_eq!(inv.gross(), Money::new(14_520, Currency::Eur));
    }
}
