/// A bank account.
pub struct Account {
    pub owner: String,
    pub balance_cents: i64,
}

impl Account {
    /// Opens an account.
    pub fn new(owner: &str, balance_cents: i64) -> Self {
        Account {
            owner: owner.to_string(),
            balance_cents,
        }
    }

    /// Adds money to the balance.
    pub fn deposit(&mut self, cents: i64) {
        self.balance_cents += cents;
    }
}

/// Deposits `cents` into every account.
pub fn credit_all(accounts: &mut [Account], cents: i64) {
    for account in accounts {
        account.add_funds(cents);
    }
}

/// Formats `owner: balance`.
pub fn report(account: &Account) -> String {
    format!("{}: {}", account.name, account.balance_cents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credits_and_reports() {
        let mut accounts = vec![Account::new("ana", 100), Account::new("bo", 0)];
        credit_all(&mut accounts, 50);
        assert_eq!(report(&accounts[0]), "ana: 150");
        assert_eq!(report(&accounts[1]), "bo: 50");
    }
}
