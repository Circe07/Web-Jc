//! Invoices built from ledger entries.

use ledger::{Currency, Entry, EntryError, Money};

/// An invoice in euros: its lines and a VAT rate in percent.
pub struct Invoice {
    pub lines: Vec<Entry>,
    pub vat_percent: i64,
}

impl Invoice {
    pub fn new(vat_percent: i64) -> Self {
        Invoice { lines: Vec::new(), vat_percent }
    }

    /// Adds a line of `cents`.
    pub fn add_line(&mut self, description: &str, cents: i64) -> Result<(), EntryError> {
        self.lines.push(Entry::new(description, eur(cents))?);
        Ok(())
    }

    /// Sum of the lines before VAT.
    pub fn net(&self) -> Money {
        eur(self.lines.iter().map(|l| l.amount.minor).sum())
    }

    /// VAT on the net amount, rounded down.
    pub fn vat(&self) -> Money {
        eur(self.net().minor * self.vat_percent / 100)
    }

    /// Net plus VAT.
    pub fn gross(&self) -> Money {
        eur(self.net().minor + self.vat().minor)
    }
}

fn eur(minor: i64) -> Money {
    Money::new(minor, Currency::Eur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals() {
        let mut inv = Invoice::new(21);
        inv.add_line("design", 10_000).unwrap();
        inv.add_line("hosting", 2_000).unwrap();
        assert_eq!(inv.net(), eur(12_000));
        assert_eq!(inv.vat(), eur(2_520));
        assert_eq!(inv.gross(), eur(14_520));
    }
}
