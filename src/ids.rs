//! Task identity: project names, refs, full IDs, and ID resolution.
//!
//! A task ID is `{project}-{ref}`, e.g. `my-app-a7b3`. Project names may
//! contain internal hyphens, so parsing always splits at the **last** dash.

use std::fmt;
use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum IdError {
    #[error("invalid project name {0:?}: use lowercase letters, digits, and internal hyphens")]
    BadProject(String),
    #[error("ambiguous ID {input:?}: matched {matches}")]
    Ambiguous { input: String, matches: String },
    #[error("task not found: {0}")]
    NotFound(String),
}

const REF_CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
pub const REF_LEN: usize = 4;

/// A validated `project-ref` identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskId {
    pub project: String,
    pub r#ref: String,
}

impl TaskId {
    /// Split `project-ref` at the last dash. Returns `None` on bad shape.
    pub fn parse(id: &str) -> Option<Self> {
        let s = id.to_lowercase();
        if !valid_id_shape(&s) {
            return None;
        }
        let dash = s.rfind('-')?;
        Some(Self {
            project: s[..dash].to_owned(),
            r#ref: s[dash + 1..].to_owned(),
        })
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.project, self.r#ref)
    }
}

fn is_project_char(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit()
}

/// `^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$` without regex.
pub fn validate_project(name: &str) -> Result<(), IdError> {
    let b = name.as_bytes();
    if b.is_empty() || !b[0].is_ascii_lowercase() {
        return Err(IdError::BadProject(name.to_owned()));
    }
    let mut prev_dash = false;
    for &c in &b[1..] {
        if c == b'-' {
            if prev_dash {
                return Err(IdError::BadProject(name.to_owned()));
            }
            prev_dash = true;
        } else if is_project_char(c) {
            prev_dash = false;
        } else {
            return Err(IdError::BadProject(name.to_owned()));
        }
    }
    if prev_dash {
        return Err(IdError::BadProject(name.to_owned()));
    }
    Ok(())
}

/// Overall ID shape: valid project, then `-`, then a non-empty ref segment.
fn valid_id_shape(s: &str) -> bool {
    let Some(dash) = s.rfind('-') else {
        return false;
    };
    let (proj, r#ref) = (&s[..dash], &s[dash + 1..]);
    if r#ref.is_empty() || !r#ref.bytes().all(is_project_char) {
        return false;
    }
    validate_project(proj).is_ok()
}

/// Generate a random 4-char lowercase alphanumeric ref.
pub fn generate_ref() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    (0..REF_LEN)
        .map(|_| {
            let i: usize = rng.random_range(0..REF_CHARS.len());
            REF_CHARS[i] as char
        })
        .collect()
}

/// Reject anything that could escape `.tasks/` (path traversal, `config`).
pub fn is_safe_id(id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    match id {
        "." | ".." | "config" => return false,
        _ => {}
    }
    !id.contains(['/', '\\'])
}

/// Resolve a user-supplied ID, prefix, or bare ref to a full task ID.
///
/// 1. Exact (safe) filename match.
/// 2. Case-insensitive prefix match on the full ID or on the ref segment.
pub fn resolve_id(tasks_dir: &Path, input: &str) -> Result<String, IdError> {
    if is_safe_id(input) && tasks_dir.join(format!("{input}.json")).exists() {
        return Ok(input.to_owned());
    }

    let entries = std::fs::read_dir(tasks_dir).map_err(|_| IdError::NotFound(input.to_owned()))?;
    let lower = input.to_lowercase();
    let mut matches = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        if id == "config" {
            continue;
        }
        if id.to_lowercase().starts_with(&lower) {
            matches.push(id.to_owned());
        } else if let Some(dash) = id.rfind('-')
            && id[dash + 1..].to_lowercase().starts_with(&lower)
        {
            matches.push(id.to_owned());
        }
    }

    match matches.len() {
        0 => Err(IdError::NotFound(input.to_owned())),
        1 => Ok(matches.pop().unwrap()),
        _ => {
            matches.sort();
            Err(IdError::Ambiguous {
                input: input.to_owned(),
                matches: matches.join(", "),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_validation() {
        for ok in ["tk", "my-app", "a1", "x-1-y-2"] {
            assert!(validate_project(ok).is_ok(), "{ok}");
        }
        for bad in ["", "A", "-a", "a-", "a--b", "a_b", "a b", "1a", "A-b"] {
            assert!(validate_project(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parse_splits_at_last_dash() {
        let id = TaskId::parse("my-app-a7b3").unwrap();
        assert_eq!(id.project, "my-app");
        assert_eq!(id.r#ref, "a7b3");
        assert!(TaskId::parse("nope").is_none());
        assert!(TaskId::parse("A-B").is_some()); // case-insensitive
    }

    #[test]
    fn generated_refs_have_shape() {
        for _ in 0..100 {
            let r = generate_ref();
            assert_eq!(r.len(), REF_LEN);
            assert!(r.bytes().all(|b| REF_CHARS.contains(&b)));
        }
    }

    #[test]
    fn unsafe_ids_rejected() {
        for bad in ["", ".", "..", "config", "../x", "a/b", "a\\b"] {
            assert!(!is_safe_id(bad), "{bad}");
        }
        assert!(is_safe_id("tk-a7b3"));
    }
}
