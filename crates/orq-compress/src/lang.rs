//! Localized fixed strings of the report.

use orq_core::Lang;

/// All the words the report uses, for one language.
pub(crate) struct Words {
    lang: Lang,
    pub crates: &'static str,
    pub workspace: &'static str,
    pub suggestion: &'static str,
    pub signature: &'static str,
    pub derived: &'static str,
    pub in_dependency: &'static str,
    pub more_there: &'static str,
}

impl Words {
    pub fn of(lang: Lang) -> Self {
        match lang {
            Lang::En => Words {
                lang,
                crates: "crates:",
                workspace: "workspace",
                suggestion: "suggestion:",
                signature: "sig:",
                derived: "possibly derived from",
                in_dependency: "in dependency",
                more_there: "more there",
            },
            Lang::Es => Words {
                lang,
                crates: "crates:",
                workspace: "workspace",
                suggestion: "sugerencia:",
                signature: "firma:",
                derived: "posiblemente derivado de",
                in_dependency: "en la dependencia",
                more_there: "más ahí",
            },
        }
    }

    pub fn errors(&self, n: usize) -> String {
        match (self.lang, n) {
            (_, 1) => "1 error".into(),
            (Lang::En, n) => format!("{n} errors"),
            (Lang::Es, n) => format!("{n} errores"),
        }
    }

    pub fn warnings(&self, n: usize, hidden: bool) -> String {
        let base = match n {
            1 => "1 warning".to_string(),
            n => format!("{n} warnings"),
        };
        match (hidden, self.lang) {
            (false, _) => base,
            (true, Lang::En) => format!("{base} hidden"),
            (true, Lang::Es) => format!("{base} {}", if n == 1 { "oculto" } else { "ocultos" }),
        }
    }

    pub fn failed_tests(&self, n: usize) -> String {
        match (self.lang, n) {
            (Lang::En, 1) => "1 failed test".into(),
            (Lang::En, n) => format!("{n} failed tests"),
            (Lang::Es, 1) => "1 test fallido".into(),
            (Lang::Es, n) => format!("{n} tests fallidos"),
        }
    }

    pub fn more_errors(&self, n: usize) -> String {
        match (self.lang, n) {
            (Lang::En, 1) => "+1 more error".into(),
            (Lang::En, n) => format!("+{n} more errors"),
            (Lang::Es, 1) => "+1 error más".into(),
            (Lang::Es, n) => format!("+{n} errores más"),
        }
    }

    pub fn more_tests(&self, n: usize) -> String {
        match self.lang {
            Lang::En => format!("+{n} more tests"),
            Lang::Es => format!("+{n} tests más"),
        }
    }

    pub fn cache(&self, hit: bool) -> &'static str {
        match (self.lang, hit) {
            (Lang::En, false) => "cache: no",
            (Lang::En, true) => "cache: yes",
            (Lang::Es, false) => "caché: no",
            (Lang::Es, true) => "caché: sí",
        }
    }
}
