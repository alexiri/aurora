//! `Secret<T>`: zeroize-on-drop wrapper with redacted formatting and no `Serialize`.

use std::fmt;
use zeroize::Zeroize;

/// Wrapper for secret material. Access requires an explicit [`Secret::expose`]
/// call (grep for `expose()` is a review checklist item).
pub struct Secret<T: Zeroize>(T);

impl<T: Zeroize> Secret<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Explicitly access the secret value.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T: Zeroize> Drop for Secret<T> {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl<T: Zeroize> From<T> for Secret<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T: Zeroize + Clone> Clone for Secret<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: Zeroize> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl<T: Zeroize> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_is_redacted() {
        let s = Secret::new(String::from("hunter2"));
        assert_eq!(format!("{s:?}"), "[redacted]");
        assert_eq!(format!("{s}"), "[redacted]");
        assert_eq!(format!("{:?}", Some(&s)), "Some([redacted])");
        assert_eq!(s.expose(), "hunter2");
    }

    #[test]
    fn clone_keeps_value() {
        let s = Secret::new(vec![1u8, 2, 3]);
        assert_eq!(s.clone().expose(), &vec![1u8, 2, 3]);
    }
}
