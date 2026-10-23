//! Money and ledger entries.
//!
//! v2: amounts carry their currency, and `Entry::new` validates its input.

use std::fmt;

/// ISO currency code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Currency {
    Eur,
    Usd,
}

/// An amount in minor units (cents) of one currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Money {
    pub minor: i64,
    pub currency: Currency,
}

impl Money {
    pub fn new(minor: i64, currency: Currency) -> Self {
        Money { minor, currency }
    }

    /// Sum of two amounts, `None` when the currencies differ.
    pub fn checked_add(self, other: Money) -> Option<Money> {
        (self.currency == other.currency).then(|| Money::new(self.minor + other.minor, self.currency))
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = match self.currency {
            Currency::Eur => "EUR",
            Currency::Usd => "USD",
        };
        write!(f, "{}.{:02} {}", self.minor / 100, (self.minor % 100).abs(), code)
    }
}

/// Why an entry was rejected.
#[derive(Debug, PartialEq, Eq)]
pub enum EntryError {
    EmptyDescription,
    ZeroAmount,
}

/// One ledger line.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub description: String,
    pub amount: Money,
}

impl Entry {
    pub fn new(description: &str, amount: Money) -> Result<Entry, EntryError> {
        if description.trim().is_empty() {
            return Err(EntryError::EmptyDescription);
        }
        if amount.minor == 0 {
            return Err(EntryError::ZeroAmount);
        }
        Ok(Entry { description: description.to_string(), amount })
    }
}
