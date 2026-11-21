//! (P5-REFINE-6, C-1071) A bad package says so in words (UX review 5, admin-12). One table keyed by
//! the violation code; the raw `code`, `path` and technical `message` stay in
//! `details.violations[]` as the machine contract.

use aurora_core::limits::PACKAGE_MAX;

const NOT_A_PACKAGE: &str =
    "That isn't an app package. Use a `.tar.gz` made with `aurora plugin pack`.";

/// The sentence for one violation. Never the raw error text of the archive reader (e.g.
/// "invalid gzip header") nor the code word itself.
#[must_use]
pub fn violation_sentence(code: &str, path: &str, message: &str) -> String {
    let path = aurora_core::sanitize::one_line(path, 200);
    match code {
        "archive_corrupt" | "archive_entry_type" | "archive_link" => NOT_A_PACKAGE.to_owned(),
        "file_missing" => format!("The package is missing {path}."),
        "icon_not_png" | "icon_not_square" | "icon_too_small" => {
            "The icon must be a square PNG.".to_owned()
        }
        "icon_too_large" => "The icon file is too large.".to_owned(),
        c if c.starts_with("path_") => {
            format!("The package has a file with an unsafe name: {path}.")
        }
        "guide_not_utf8" => "The setup guide must be plain UTF-8 text.".to_owned(),
        "guide_too_large" => "The setup guide is too large.".to_owned(),
        "manifest_not_utf8" => "The package has no readable plugin.yaml.".to_owned(),
        "manifest_too_large" => "The package's plugin.yaml is too large.".to_owned(),
        "package_too_large" => too_large_sentence(),
        "signature_missing"
        | "signature_untrusted"
        | "signature_malformed"
        | "key_revoked"
        | "digest_mismatch"
        | "signer_not_source_key"
        | "builtin_not_project_signed" => {
            "The package isn't signed by a key this server trusts.".to_owned()
        }
        _ => manifest_sentence(code, message).unwrap_or_else(|| {
            if path.is_empty() {
                "The package has a problem.".to_owned()
            } else {
                format!("The package has a problem: {path}.")
            }
        }),
    }
}

/// A manifest validator's message as a sentence, without the code word ("template_syntax: …").
fn manifest_sentence(code: &str, message: &str) -> Option<String> {
    let m = aurora_core::sanitize::one_line(message, 300);
    let m = m.replace(&format!("{code}: "), "").replace(code, "");
    let m = m.trim().trim_end_matches('.').trim();
    if m.is_empty() {
        return None;
    }
    let mut c = m.chars();
    let first = c.next()?;
    Some(format!("{}{}.", first.to_uppercase(), c.as_str()))
}

/// `413`: "That package is too large (limit 20 MB)."
#[must_use]
pub fn too_large_sentence() -> String {
    format!(
        "That package is too large (limit {} MB).",
        PACKAGE_MAX / (1024 * 1024)
    )
}

/// The `message` of a `422 package_invalid`: the sentence of the only violation, or "That app
/// package has N problems. The first: …".
#[must_use]
pub fn summary(sentences: &[String]) -> String {
    match sentences {
        [] => "The package is invalid.".to_owned(),
        [one] => one.clone(),
        [first, ..] => format!(
            "That app package has {} problems. The first: {first}",
            sentences.len()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_code_has_words_and_no_raw_error_text() {
        let s = |c: &str| violation_sentence(c, "icons/app.png", "invalid gzip header");
        assert_eq!(s("archive_corrupt"), NOT_A_PACKAGE);
        assert_eq!(s("archive_entry_type"), NOT_A_PACKAGE);
        assert_eq!(s("archive_link"), NOT_A_PACKAGE);
        assert_eq!(
            violation_sentence("file_missing", "plugin.yaml", "x"),
            "The package is missing plugin.yaml."
        );
        assert_eq!(s("icon_not_png"), "The icon must be a square PNG.");
        assert_eq!(s("icon_not_square"), "The icon must be a square PNG.");
        for c in [
            "path_traversal",
            "path_absolute",
            "path_duplicate",
            "path_invalid",
            "path_unknown",
        ] {
            assert_eq!(
                s(c),
                "The package has a file with an unsafe name: icons/app.png."
            );
        }
        assert_eq!(
            s("guide_not_utf8"),
            "The setup guide must be plain UTF-8 text."
        );
        assert_eq!(
            s("package_too_large"),
            "That package is too large (limit 20 MB)."
        );
        assert_eq!(
            s("signature_untrusted"),
            "The package isn't signed by a key this server trusts."
        );
        for c in [
            "archive_corrupt",
            "icon_not_png",
            "path_traversal",
            "guide_not_utf8",
            "signature_missing",
        ] {
            let t = s(c);
            assert!(!t.contains("gzip header") && !t.contains(c), "{c}: {t}");
        }
    }

    #[test]
    fn manifest_and_unknown_codes() {
        assert_eq!(
            violation_sentence(
                "custom_only_without_tools",
                "apps/0/grants",
                "`custom_only` needs at least one tool in the catalogue"
            ),
            "`custom_only` needs at least one tool in the catalogue."
        );
        assert_eq!(
            violation_sentence("template_syntax", "x", "template_syntax: unclosed `{`"),
            "Unclosed `{`."
        );
        assert_eq!(
            violation_sentence("something_new", "apps/0", ""),
            "The package has a problem: apps/0."
        );
        assert_eq!(summary(&["A.".into()]), "A.");
        assert_eq!(
            summary(&["A.".into(), "B.".into(), "C.".into()]),
            "That app package has 3 problems. The first: A."
        );
    }
}
