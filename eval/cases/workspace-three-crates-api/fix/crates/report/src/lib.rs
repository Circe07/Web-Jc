//! Plain-text invoice report.

use billing::Invoice;

/// The invoice as text: one line per entry, then net, VAT and gross.
pub fn render(invoice: &Invoice) -> String {
    let mut out = String::new();
    for line in &invoice.lines {
        out.push_str(&format!("{}: {}\n", line.description, line.amount));
    }
    out.push_str(&format!("{}: {}\n", "net", invoice.net()));
    out.push_str(&format!("{}: {}\n", "vat", invoice.vat()));
    out.push_str(&format!("{}: {}", "gross", invoice.gross()));
    out
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
