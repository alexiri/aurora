//! Identifiers: UUIDv7 row ids, slug and agent-handle validators.

use uuid::Uuid;

/// New application-generated row id (UUIDv7).
pub fn new_id() -> Uuid {
    Uuid::now_v7()
}

/// Slug: `[a-z0-9][a-z0-9-]{1,47}` (2 to 48 chars).
pub fn is_valid_slug(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=48).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// Agent handle: `[a-z][a-z0-9-]{0,39}` (1 to 40 chars).
pub fn is_valid_handle(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=40).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn ids_are_v7_and_unique() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert_eq!(a.get_version_num(), 7);
    }

    #[test]
    fn slug_examples() {
        for ok in ["ab", "a-", "0a", "acme-corp", &"a".repeat(48)] {
            assert!(is_valid_slug(ok), "{ok}");
        }
        for bad in ["", "a", "-a", "Ab", "a_b", "a b", "é1", &"a".repeat(49)] {
            assert!(!is_valid_slug(bad), "{bad}");
        }
    }

    #[test]
    fn handle_examples() {
        for ok in ["a", "bot-1", &"a".repeat(40)] {
            assert!(is_valid_handle(ok), "{ok}");
        }
        for bad in ["", "1a", "-a", "A", "a_b", &"a".repeat(41)] {
            assert!(!is_valid_handle(bad), "{bad}");
        }
    }

    proptest! {
        #[test]
        fn slug_matches_regex(s in "[a-z0-9][a-z0-9-]{1,47}") {
            prop_assert!(is_valid_slug(&s));
        }

        #[test]
        fn handle_matches_regex(s in "[a-z][a-z0-9-]{0,39}") {
            prop_assert!(is_valid_handle(&s));
        }

        #[test]
        fn slug_arbitrary_never_panics_and_is_sound(s in "\\PC{0,60}") {
            if is_valid_slug(&s) {
                prop_assert!((2..=48).contains(&s.len()));
                prop_assert!(s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
                prop_assert!(!s.starts_with('-'));
            }
        }

        #[test]
        fn handle_arbitrary_never_panics_and_is_sound(s in "\\PC{0,60}") {
            if is_valid_handle(&s) {
                prop_assert!((1..=40).contains(&s.len()));
                prop_assert!(s.starts_with(|c: char| c.is_ascii_lowercase()));
            }
        }
    }
}
