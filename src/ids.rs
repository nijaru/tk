//! Entry identity: the ref.
//!
//! A ref is four characters from the Crockford base32 alphabet, assigned once
//! and never reused. An entry's file is `<ref>-<slug>.json`: the ref is identity,
//! the slug is a creation-time mnemonic. Renaming a title therefore breaks no
//! reference, and nothing anywhere records a path.
//!
//! Crockford base32 drops `i`, `l`, `o`, and `u`, which is why a ref is safe to
//! type, read aloud, or copy out of a filename: the letters that are mistaken for
//! `1` and `0` are simply not in the alphabet. Refs are lowercase on disk and
//! matched case-insensitively, so a ref read off a screen always works.

use std::fmt;

use thiserror::Error;

/// Crockford base32: no `i`, `l`, `o`, or `u`.
const CROCKFORD: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
pub const REF_LEN: usize = 4;
const SLUG_MAX: usize = 60;

#[derive(Debug, Error)]
pub enum IdError {
    #[error("no entry matches {0:?}: use its ref (like a7b3) or part of its title")]
    NotFound(String),
    #[error("ambiguous reference {input:?}: matched {matches}")]
    Ambiguous { input: String, matches: String },
    #[error("empty reference")]
    Empty,
}

/// A fresh ref.
pub fn new_ref() -> String {
    use rand::Rng as _;
    let mut rng = rand::rng();
    (0..REF_LEN)
        .map(|_| CROCKFORD[rng.random_range(0..CROCKFORD.len())] as char)
        .collect()
}

pub fn is_valid_ref(value: &str) -> bool {
    value.len() == REF_LEN && value.bytes().all(|b| CROCKFORD.contains(&b))
}

/// A filename-safe slug of a title: lowercase, alphanumerics joined by `-`.
pub fn slug(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut pending_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    if out.len() > SLUG_MAX {
        out.truncate(SLUG_MAX);
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.is_empty() {
        out.push_str("entry");
    }
    out
}

/// One entry as the resolver sees it: identity, and whether it is finished.
///
/// The state is here because readiness depends on it — a blocker that is done
/// stops blocking — and the resolver is already reading every entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    pub r#ref: String,
    pub slug: String,
    pub title: String,
    pub state: crate::model::State,
}

impl Known {
    /// `<ref>-<slug>.json`: the ref is identity, the slug is a mnemonic that
    /// makes a directory listing readable.
    pub fn file_name(&self) -> String {
        file_name_of(&self.r#ref, &self.slug)
    }
}

/// `<ref>-<slug>.json`, or `<ref>.json` when a title produced no slug.
pub fn file_name_of(r#ref: &str, slug: &str) -> String {
    if slug.is_empty() {
        format!("{ref}.json", ref = r#ref)
    } else {
        format!("{}-{}.json", r#ref, slug)
    }
}

/// Parse `<ref>-<slug>.json`.
pub fn parse_file_name(name: &str) -> Option<Known> {
    let stem = name.strip_suffix(".json")?;
    let (r#ref, slug) = stem.split_once('-').unwrap_or((stem, ""));
    if !is_valid_ref(r#ref) {
        return None;
    }
    Some(Known {
        r#ref: r#ref.to_owned(),
        slug: slug.to_owned(),
        title: String::new(),
        state: crate::model::State::Open,
    })
}

/// Resolve a ref, or part of a title, to exactly one entry's ref.
///
/// Refs are exactly four characters, so there is no prefix rule: a full ref, or
/// a case-insensitive substring of the slug or title. Ambiguity is an error
/// rather than a guess.
pub fn resolve(known: &[Known], input: &str) -> Result<String, IdError> {
    let needle = input.trim().to_lowercase();
    if needle.is_empty() {
        return Err(IdError::Empty);
    }

    if let Some(entry) = known.iter().find(|k| k.r#ref == needle) {
        return Ok(entry.r#ref.clone());
    }

    let mut matches: Vec<&Known> = known
        .iter()
        .filter(|k| {
            k.slug.to_lowercase().contains(&needle) || k.title.to_lowercase().contains(&needle)
        })
        .collect();
    matches.sort_by(|a, b| a.r#ref.cmp(&b.r#ref));
    matches.dedup_by(|a, b| a.r#ref == b.r#ref);
    match matches.len() {
        0 => Err(IdError::NotFound(input.to_owned())),
        1 => Ok(matches[0].r#ref.clone()),
        _ => Err(IdError::Ambiguous {
            input: input.to_owned(),
            matches: matches
                .iter()
                .map(|k| {
                    if k.title.is_empty() {
                        k.file_name()
                    } else {
                        format!("{} ({})", k.r#ref, k.title)
                    }
                })
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
}

impl miette::Diagnostic for IdError {
    fn code(&self) -> Option<Box<dyn fmt::Display + '_>> {
        let code = match self {
            Self::NotFound(_) => crate::output::code::NOT_FOUND,
            Self::Ambiguous { .. } => crate::output::code::AMBIGUOUS,
            Self::Empty => crate::output::code::INVALID_INPUT,
        };
        Some(Box::new(code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn known(r#ref: &str, slug: &str, title: &str) -> Known {
        Known {
            r#ref: r#ref.to_owned(),
            slug: slug.to_owned(),
            title: title.to_owned(),
            state: crate::model::State::Open,
        }
    }

    #[test]
    fn refs_have_shape_and_a_useful_space() {
        let mut seen = HashSet::new();
        for _ in 0..500 {
            let r = new_ref();
            assert!(is_valid_ref(&r), "{r}");
            seen.insert(r);
        }
        assert!(seen.len() > 490, "refs must not collide often");
        assert!(!is_valid_ref("abc"));
        assert!(!is_valid_ref("abcdx"));
        assert!(!is_valid_ref("ABCD"));
        for confusable in ["a7bi", "a7bl", "a7bo", "a7bu"] {
            assert!(!is_valid_ref(confusable), "{confusable}");
        }
    }

    #[test]
    fn an_alphabet_without_confusables() {
        // i/l/o/u are absent, so a ref read off a screen cannot be confused with
        // 1 or 0, and `l` cannot be mistaken for `1`.
        for letter in ["i", "l", "o", "u"] {
            assert!(
                !is_valid_ref(&format!("a{letter}bc")),
                "{letter} is not in the alphabet"
            );
        }
        assert!(is_valid_ref("0123"));
        assert!(is_valid_ref("vvvv"), "v is the last letter of the alphabet");
        assert!(is_valid_ref("zzzz"), "z is too");
    }

    #[test]
    fn slugs_are_filename_safe() {
        assert_eq!(slug("Rewrite the auth layer"), "rewrite-the-auth-layer");
        assert_eq!(slug("  Trailing -- dashes  "), "trailing-dashes");
        assert_eq!(slug("feat: add 100% support!"), "feat-add-100-support");
        assert_eq!(slug("日本語"), "entry");
        assert_eq!(slug(""), "entry");
        assert!(!slug(&"x".repeat(200)).contains("--"));
        assert!(slug(&"x".repeat(200)).len() <= SLUG_MAX);
    }

    #[test]
    fn file_names_round_trip() {
        let k = known("a7b3", "rewrite-auth", "Rewrite auth");
        assert_eq!(k.file_name(), "a7b3-rewrite-auth.json");
        let parsed = parse_file_name("a7b3-rewrite-auth.json").expect("parses");
        assert_eq!(parsed.r#ref, "a7b3");
        assert_eq!(parsed.slug, "rewrite-auth");
        assert!(parse_file_name("notes.json").is_none());
        assert!(parse_file_name("README.md").is_none());
        assert!(
            parse_file_name(".tk.json").is_none(),
            "the store file is not an entry"
        );
        assert_eq!(parse_file_name("a7b3.json").expect("parses").slug, "");
        assert_eq!(file_name_of("a7b3", ""), "a7b3.json");
        assert_eq!(file_name_of("a7b3", "x"), "a7b3-x.json");
    }

    #[test]
    fn resolution_prefers_a_ref_then_a_unique_title() {
        let entries = vec![
            known("a7b3", "rewrite-auth", "Rewrite the auth layer"),
            known("b7c4", "parser", "Write the parser"),
        ];
        assert_eq!(resolve(&entries, "a7b3").unwrap(), "a7b3");
        assert_eq!(
            resolve(&entries, "A7B3").unwrap(),
            "a7b3",
            "case-insensitive"
        );
        assert_eq!(resolve(&entries, "parser").unwrap(), "b7c4");
        assert_eq!(
            resolve(&entries, "AUTH").unwrap(),
            "a7b3",
            "title substring"
        );
        assert!(matches!(
            resolve(&entries, "nope"),
            Err(IdError::NotFound(_))
        ));
        assert!(matches!(resolve(&entries, ""), Err(IdError::Empty)));
    }

    #[test]
    fn ambiguity_is_an_error_not_a_guess() {
        let entries = vec![
            known("a7b3", "auth-rewrite", "Rewrite auth"),
            known("b7c4", "auth-parser", "Parse auth"),
        ];
        match resolve(&entries, "auth") {
            Err(IdError::Ambiguous { matches, .. }) => {
                assert!(matches.contains("a7b3"), "{matches}");
                assert!(matches.contains("b7c4"), "{matches}");
            }
            other => panic!("expected ambiguity, got {other:?}"),
        }
    }
}
