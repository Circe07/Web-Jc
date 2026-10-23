//! Plain-text invoice report.

use billing::Invoice;

/// The invoice as text: one line per entry, then net, VAT and gross.
pub fn render(invoice: &Invoice) -> String {
    let mut out = String::new();
    for line in &invoice.lines {
        out.push_str(&format!("{}: {}\n", line.description, cents(line.amount.0)));
    }
    out.push_str(&format!("{}: {}\n", "net", cents(invoice.net().0)));
    out.push_str(&format!("{}: {}\n", "vat", cents(invoice.vat().0)));
    out.push_str(&format!("{}: {}", "gross", cents(invoice.gross().0)));
    out
}

fn cents(minor: i64) -> String {
    format!("{}.{:02}", minor / 100, minor % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders() {
        let mut inv = Invoice::new(10);
        inv.add_line("book", 1_999).unwrap();
        let text = render(&inv);
        assert_eq!(
            text,
            "book: 19.99 EUR\nnet: 19.99 EUR\nvat: 1.99 EUR\ngross: 21.98 EUR"
        );
    }
}
