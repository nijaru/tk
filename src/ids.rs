//! Task identity: ULIDs, short aliases, and reference resolution.
//!
//! A task ID is a 26-character ULID: Crockford base32, lowercased, sortable by
//! creation time. Identity never changes — `project` is a mutable display field
//! and the 4-character alias is the handle people type. Resolution order is
//! exact alias → exact ID → unique ID prefix.
//!
//! Both namespaces draw from the same alphabet, so an input of exactly
//! [`ALIAS_LEN`] characters that matches an alias resolves as that alias; type
//! more of the ULID to reach a record whose ID happens to start the same way.

use std::fmt;
use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum IdError {
    #[error("invalid project name {0:?}: use lowercase letters, digits, and internal hyphens")]
    BadProject(String),
    #[error("ambiguous reference {input:?}: matched {matches}")]
    Ambiguous { input: String, matches: String },
    #[error(
        "reference {0:?} is too short: use the {ALIAS_LEN}-character alias or at least {MIN_PREFIX} characters of an ID"
    )]
    TooShort(String),
    #[error("task not found: {0}")]
    NotFound(String),
}

impl miette::Diagnostic for IdError {
    fn code(&self) -> Option<Box<dyn fmt::Display + '_>> {
        let code = match self {
            Self::NotFound(_) => crate::output::code::NOT_FOUND,
            Self::Ambiguous { .. } => crate::output::code::AMBIGUOUS,
            Self::BadProject(_) | Self::TooShort(_) => crate::output::code::INVALID_INPUT,
        };
        Some(Box::new(code))
    }
}

/// Crockford base32: no `i`, `l`, `o`, or `u`, so IDs stay unambiguous.
const CROCKFORD: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Length of a ULID in characters.
pub const ID_LEN: usize = 26;
/// Length of a short alias in characters.
pub const ALIAS_LEN: usize = 4;
/// Shortest ID prefix accepted as a reference.
pub const MIN_PREFIX: usize = 4;

// ---------------------------------------------------------------------------
// ULID
// ---------------------------------------------------------------------------

/// A fresh, unique ID for a task created now.
pub fn new_id() -> String {
    new_id_at(now_ms())
}

/// A unique ID whose timestamp component is `ms` since the Unix epoch.
pub fn new_id_at(ms: u64) -> String {
    use rand::Rng as _;
    let random: u128 = rand::rng().random::<u128>() & ((1u128 << 80) - 1);
    let value = ((u128::from(ms) & 0xffff_ffff_ffff) << 80) | random;
    encode(value)
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

/// Big-endian 128-bit value → 26 Crockford characters.
fn encode(mut value: u128) -> String {
    let mut out = [0u8; ID_LEN];
    for slot in out.iter_mut().rev() {
        *slot = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8(out.to_vec()).unwrap_or_default()
}

/// 26 Crockford characters → the 128-bit value, or `None` when malformed.
pub fn decode(value: &str) -> Option<u128> {
    let bytes = value.as_bytes();
    if bytes.len() != ID_LEN {
        return None;
    }
    let mut out: u128 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let digit = CROCKFORD.iter().position(|&c| c == b)? as u128;
        // 26 * 5 = 130 bits, so the first character may only use two of them.
        if i == 0 && digit > 7 {
            return None;
        }
        out = (out << 5) | digit;
    }
    Some(out)
}

/// Creation time (ms since the Unix epoch) encoded in an ID.
pub fn timestamp_ms(id: &str) -> Option<u64> {
    Some((decode(id)? >> 80) as u64)
}

pub fn is_valid_id(s: &str) -> bool {
    decode(s).is_some()
}

// ---------------------------------------------------------------------------
// Alias
// ---------------------------------------------------------------------------

/// A random 4-character alias.
pub fn new_alias() -> String {
    use rand::Rng as _;
    let mut rng = rand::rng();
    (0..ALIAS_LEN)
        .map(|_| CROCKFORD[rng.random_range(0..CROCKFORD.len())] as char)
        .collect()
}

pub fn is_valid_alias(s: &str) -> bool {
    s.len() == ALIAS_LEN && s.bytes().all(|b| CROCKFORD.contains(&b))
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// One record's identity, as the resolver sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    pub id: String,
    pub alias: String,
    /// Aliases carried over from a migration, so pre-cutover references
    /// (`project-ref`, a bare `ref`) keep resolving.
    pub legacy_aliases: Vec<String>,
}

impl Known {
    /// Does this record answer to `needle` (already lowercased)?
    fn answers_to(&self, needle: &str) -> bool {
        self.alias == needle
            || self
                .legacy_aliases
                .iter()
                .any(|l| l.to_lowercase() == needle)
    }
}

/// Resolve a user-supplied alias, ID, or ID prefix.
///
/// Ambiguity within a step is an error, never a guess.
pub fn resolve(known: &[Known], input: &str) -> Result<String, IdError> {
    let needle = input.trim().to_lowercase();
    if needle.is_empty() {
        return Err(IdError::NotFound(input.to_owned()));
    }

    // 1. Exact alias (current or carried over from a migration).
    let mut matches: Vec<&Known> = known.iter().filter(|k| k.answers_to(&needle)).collect();
    if let Some(id) = single(&mut matches, input)? {
        return Ok(id);
    }

    // 2. Exact ID.
    if let Some(k) = known.iter().find(|k| k.id == needle) {
        return Ok(k.id.clone());
    }

    // 3. Unique ID prefix.
    if needle.len() < MIN_PREFIX {
        return Err(IdError::TooShort(input.to_owned()));
    }
    if !needle.bytes().all(|b| CROCKFORD.contains(&b)) {
        return Err(IdError::NotFound(input.to_owned()));
    }
    let mut matches: Vec<&Known> = known.iter().filter(|k| k.id.starts_with(&needle)).collect();
    match single(&mut matches, input)? {
        Some(id) => Ok(id),
        None => Err(IdError::NotFound(input.to_owned())),
    }
}

/// `Ok(None)` for no matches, `Ok(Some(id))` for exactly one.
///
/// The ambiguity message names each candidate by alias, because a prefix that
/// collides is nearly always two tasks created in the same millisecond, whose
/// IDs agree for their first ten characters — quoting those would tell the
/// reader nothing.
fn single(matches: &mut Vec<&Known>, input: &str) -> Result<Option<String>, IdError> {
    matches.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    matches.dedup_by(|a, b| a.id == b.id);
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches[0].id.clone())),
        _ => Err(IdError::Ambiguous {
            input: input.to_owned(),
            matches: matches
                .iter()
                .map(|k| {
                    if k.alias.is_empty() {
                        short_id(&k.id)
                    } else {
                        format!("{} ({})", k.alias, short_id(&k.id))
                    }
                })
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
}

/// Enough of an ID to tell two of them apart in a message.
fn short_id(id: &str) -> String {
    id.chars().take(10).collect()
}

/// Reject a name that must never be used as a path component.
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != ".."
}

/// Look up one record's identity by ID, from an already-built index.
pub fn known_by_id<'a>(known: &'a [Known], id: &str) -> Option<&'a Known> {
    known.iter().find(|k| k.id == id)
}

/// Paths are always built from validated IDs; this keeps callers honest.
pub fn record_file_name(id: &str) -> Option<String> {
    is_valid_id(id).then(|| format!("{id}.jsonl"))
}

impl fmt::Display for Known {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.alias, self.id)
    }
}

/// Resolve within a records directory (convenience for callers without a store).
pub fn resolve_in(records_dir: &Path, input: &str) -> Result<String, IdError> {
    let mut known = Vec::new();
    let entries =
        std::fs::read_dir(records_dir).map_err(|_| IdError::NotFound(input.to_owned()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".jsonl") else {
            continue;
        };
        if is_valid_id(id) {
            known.push(Known {
                id: id.to_owned(),
                alias: String::new(),
                legacy_aliases: Vec::new(),
            });
        }
    }
    resolve(&known, input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(id: &str, alias: &str) -> Known {
        Known {
            id: id.to_owned(),
            alias: alias.to_owned(),
            legacy_aliases: Vec::new(),
        }
    }

    #[test]
    fn ids_have_ulid_shape() {
        for _ in 0..200 {
            let id = new_id();
            assert_eq!(id.len(), ID_LEN, "{id}");
            assert!(is_valid_id(&id), "{id}");
            // 26 * 5 = 130 bits, so the first character encodes only 2 bits.
            assert!(id.as_bytes()[0] <= b'7', "{id}");
        }
    }

    #[test]
    fn ids_sort_by_creation_time() {
        // A later timestamp must never sort before an earlier one, whatever the
        // random tail happens to be.
        let early = (0..50).map(|_| new_id_at(1_000_000)).collect::<Vec<_>>();
        let late = (0..50).map(|_| new_id_at(2_000_000)).collect::<Vec<_>>();
        let max_early = early.iter().max().unwrap();
        let min_late = late.iter().min().unwrap();
        assert!(max_early < min_late, "{max_early} !< {min_late}");
    }

    #[test]
    fn ids_round_trip_through_decode() {
        let id = new_id_at(1_700_000_000_123);
        assert_eq!(timestamp_ms(&id), Some(1_700_000_000_123));
        assert!(decode(&id).is_some());
        assert!(
            decode(&id.to_uppercase()).is_none(),
            "alphabet is lowercase"
        );
        assert!(decode("short").is_none());
        // 26 valid characters whose value overflows 128 bits.
        assert!(decode("zzzzzzzzzzzzzzzzzzzzzzzzzz").is_none());
    }

    #[test]
    fn aliases_have_shape() {
        for _ in 0..200 {
            let a = new_alias();
            assert!(is_valid_alias(&a), "{a}");
        }
        assert!(!is_valid_alias("abc"));
        assert!(!is_valid_alias("ABCD"));
        assert!(!is_valid_alias("iiii"), "ambiguous letters are excluded");
    }

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
    fn resolution_prefers_alias_then_id_then_prefix() {
        // Far enough apart that an 8-character prefix is unambiguous: the
        // timestamp occupies the most significant bits of a ULID.
        let id = new_id_at(1_700_000_000_000);
        let other = new_id_at(1_900_000_000_000);
        let known = vec![known(&id, "a7b3"), known(&other, "b7c4")];

        assert_eq!(resolve(&known, "a7b3").unwrap(), id);
        assert_eq!(resolve(&known, "A7B3").unwrap(), id, "case-insensitive");
        assert_eq!(resolve(&known, &id).unwrap(), id);

        let prefix = &id[..8];
        assert_ne!(prefix, &other[..8], "test premise: prefixes differ");
        assert_eq!(resolve(&known, prefix).unwrap(), id);
    }

    #[test]
    fn an_ambiguous_prefix_lists_its_candidates() {
        // Two records created in the same millisecond share a long prefix.
        let a = new_id_at(1_700_000_000_000);
        let mut b = new_id_at(1_700_000_000_000);
        while b[..4] != a[..4] {
            b = new_id_at(1_700_000_000_000);
        }
        let known = vec![known(&a, "aaaa"), known(&b, "bbbb")];
        match resolve(&known, &a[..4]) {
            Err(IdError::Ambiguous { matches, .. }) => {
                // Candidates are named by alias: the IDs themselves agree for
                // their first ten characters here, so quoting only IDs would
                // not help the reader choose.
                assert!(matches.contains("aaaa"), "{matches}");
                assert!(matches.contains("bbbb"), "{matches}");
            }
            other => panic!("expected ambiguity, got {other:?}"),
        }
    }

    #[test]
    fn resolution_reports_the_failure_mode() {
        let id = new_id_at(1_700_000_000_000);
        let known = vec![known(&id, "a7b3")];
        assert!(matches!(resolve(&known, "nope"), Err(IdError::NotFound(_))));
        assert!(matches!(
            resolve(&known, &id[..3]),
            Err(IdError::TooShort(_))
        ));
        assert!(matches!(resolve(&known, ""), Err(IdError::NotFound(_))));
    }

    #[test]
    fn aliases_prefer_the_shortest_form_and_legacy_names_resolve() {
        let id = new_id_at(1_700_000_000_000);
        let mut k = known(&id, "a7b3");
        k.legacy_aliases.push("demo-a7b3".to_owned());
        k.legacy_aliases.push("a7b3-old".to_owned());
        let packed = vec![k];
        assert_eq!(resolve(&packed, "demo-a7b3").unwrap(), id);
        assert_eq!(resolve(&packed, "A7B3-OLD").unwrap(), id);
        assert_eq!(resolve(&packed, "a7b3").unwrap(), id);
    }

    #[test]
    fn two_records_cannot_claim_one_alias() {
        let a = new_id_at(1);
        let b = new_id_at(2);
        let packed = vec![known(&a, "dup0"), known(&b, "dup0")];
        assert!(matches!(
            resolve(&packed, "dup0"),
            Err(IdError::Ambiguous { .. })
        ));
    }

    #[test]
    fn unsafe_names_rejected() {
        for bad in ["", ".", "..", "a/b", "a\\b"] {
            assert!(!is_safe_name(bad), "{bad}");
        }
        assert!(is_safe_name("demo-a7b3"));
        assert!(record_file_name("nope").is_none());
        assert!(record_file_name(&new_id()).is_some());
    }
}
