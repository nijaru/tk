//! One JSON envelope for every command.
//!
//! Agents branch on shape, so `--json` emits the same five keys everywhere
//! (plus a stable `error_code` on failure) instead of a per-command payload.
//! A command that has nothing to say still says so explicitly: `data` is
//! `null` rather than absent, so a reader never has to guess.

use serde::Serialize;
use serde_json::Value;

/// Stable machine-readable failure kinds. Values are part of the contract.
pub mod code {
    pub const NOT_FOUND: &str = "not_found";
    pub const AMBIGUOUS: &str = "ambiguous";
    pub const STALE_REVISION: &str = "stale_revision";
    /// The directory is a task store, but not a format this binary reads.
    pub const NOT_A_STORE: &str = "not_a_store";
    pub const STORE_NOT_FOUND: &str = "store_not_found";
    pub const INVALID_INPUT: &str = "invalid_input";
    pub const IO: &str = "io";
    pub const PARSE: &str = "parse";
    pub const CHECK_FAILED: &str = "check_failed";
    pub const USAGE: &str = "usage";
    pub const ERROR: &str = "error";
}

#[derive(Debug, Serialize)]
pub struct Envelope {
    pub ok: bool,
    pub command: String,
    pub rev: Option<String>,
    pub data: Value,
    pub issues: Vec<String>,
    pub error_code: Option<String>,
}

/// An error whose JSON envelope the command already printed.
///
/// Lets a command report a structured failure (its own `error_code`) and still
/// exit non-zero, without the top level printing a second envelope.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
#[error("{0}")]
pub struct Reported(pub String);

/// A command-line or request-body mistake, with a stable code.
///
/// Guards like "this needs -f" are the caller's error, not a failure of the
/// store, and a caller should be able to tell those apart.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct InputError {
    pub message: String,
    pub code: &'static str,
}

impl miette::Diagnostic for InputError {}

/// Build an "your input is wrong" report.
pub fn invalid(message: impl Into<String>) -> miette::Report {
    InputError {
        message: message.into(),
        code: code::INVALID_INPUT,
    }
    .into()
}

/// A successful result.
pub fn ok(command: &str, data: Value, rev: Option<String>, issues: Vec<String>) -> Envelope {
    Envelope {
        ok: true,
        command: command.to_owned(),
        rev,
        data,
        issues,
        error_code: None,
    }
}

/// A failure, reported on stdout so a `--json` caller never has to parse prose
/// out of stderr to find out what kind of failure it was.
pub fn err(command: &str, error_code: &str, message: &str) -> Envelope {
    Envelope {
        ok: false,
        command: command.to_owned(),
        rev: None,
        data: Value::Null,
        issues: vec![message.to_owned()],
        error_code: Some(error_code.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_set_is_stable() {
        let value = serde_json::to_value(ok("show", Value::Null, None, Vec::new())).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["command", "data", "error_code", "issues", "ok", "rev"],
            "every command emits the same key set"
        );

        let failure =
            serde_json::to_value(err("show", code::NOT_FOUND, "task not found: x")).unwrap();
        assert_eq!(failure["ok"], serde_json::json!(false));
        assert_eq!(failure["error_code"], "not_found");
        assert_eq!(failure["data"], Value::Null);
        assert_eq!(failure["rev"], Value::Null);
        // The failure carries the same keys as a success, so a reader can
        // index into either without a shape check.
        let mut failure_keys: Vec<&str> = failure
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        failure_keys.sort_unstable();
        assert_eq!(failure_keys, keys);
    }
}
