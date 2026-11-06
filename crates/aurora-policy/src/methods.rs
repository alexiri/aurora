//! Method lists: empty = any; comparison is ASCII case-insensitive.
pub fn matches(methods: &[String], method: &str) -> bool {
    methods.is_empty() || methods.iter().any(|m| m.eq_ignore_ascii_case(method))
}

#[cfg(test)]
mod tests {
    #[test]
    fn basics() {
        assert!(super::matches(&[], "GET"));
        assert!(super::matches(&["POST".into()], "post"));
        assert!(!super::matches(&["POST".into()], "GET"));
    }
}
