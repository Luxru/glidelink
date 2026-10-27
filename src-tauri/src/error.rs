//! Small, locale-neutral error envelope shared with the UI.
//!
//! Commands return either `CODE` or `CODE|detail`. The web UI owns the actual
//! localized strings, so changing the language never requires restarting the
//! backend or re-deriving errors.

use std::fmt::Display;

pub fn code(code: &'static str) -> String {
    code.to_string()
}

pub fn detail(code: &'static str, detail: impl Display) -> String {
    format!("{code}|{detail}")
}

pub fn split(raw: &str) -> (String, Option<String>) {
    match raw.split_once('|') {
        Some((code, detail)) => (code.to_string(), Some(detail.to_string())),
        None => (raw.to_string(), None),
    }
}
