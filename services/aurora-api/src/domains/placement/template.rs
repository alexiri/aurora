//! Workspace name templates (P4-ONB-1, D-140, SEC-79).
//!
//! A template is literal text with `{variable}` placeholders; `|` separates a fallback chain
//! (`{sub}|{email_local}`): the first alternative whose placeholders all resolve
//! to a non-empty value and whose result still contains a letter or digit wins; when none does,
//! the caller falls back to `user-<short id>`. Values come only from the signing-in provider's
//! validated claims (never from request input).

use serde_json::Value;

pub const MAX_TEMPLATE_LEN: usize = 200;
const MAX_NAME_CHARS: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Var {
    /// `{sub}`: the provider's unique, never-reassigned subject (OIDC Core §2).
    Sub,
    PreferredUsername,
    Name,
    EmailLocal,
    GivenName,
    FamilyName,
    /// `{claim:dot.path}`: a string claim of the validated token.
    Claim(String),
}

impl Var {
    pub fn label(&self) -> String {
        match self {
            Self::Sub => "sub".into(),
            Self::PreferredUsername => "preferred_username".into(),
            Self::Name => "name".into(),
            Self::EmailLocal => "email_local".into(),
            Self::GivenName => "given_name".into(),
            Self::FamilyName => "family_name".into(),
            Self::Claim(p) => format!("claim:{p}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Lit(String),
    Var(Var),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    chain: Vec<Vec<Part>>,
}

fn parse_var(name: &str) -> Result<Var, String> {
    Ok(match name {
        "sub" => Var::Sub,
        "preferred_username" => Var::PreferredUsername,
        "name" => Var::Name,
        "email_local" => Var::EmailLocal,
        "given_name" => Var::GivenName,
        "family_name" => Var::FamilyName,
        _ => match name.strip_prefix("claim:") {
            Some(path)
                if !path.is_empty()
                    && path.len() <= 64
                    && path
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) =>
            {
                Var::Claim(path.to_owned())
            }
            _ => return Err(format!("unknown variable `{{{name}}}`")),
        },
    })
}

/// Parse and validate a template (400 `invalid_request` text on error).
pub fn parse(s: &str) -> Result<Template, String> {
    if s.trim().is_empty() {
        return Err("must not be empty".into());
    }
    if s.chars().count() > MAX_TEMPLATE_LEN {
        return Err(format!("must be at most {MAX_TEMPLATE_LEN} characters"));
    }
    if s.chars().any(char::is_control) {
        return Err("must not contain control characters".into());
    }
    let mut chain = Vec::new();
    for alt in s.split('|') {
        let mut parts = Vec::new();
        let mut lit = String::new();
        let mut rest = alt;
        while let Some(i) = rest.find(['{', '}']) {
            lit.push_str(&rest[..i]);
            if rest[i..].starts_with('}') {
                return Err("unmatched `}`".into());
            }
            let after = &rest[i + 1..];
            let end = after.find('}').ok_or("unclosed `{`")?;
            if !lit.is_empty() {
                parts.push(Part::Lit(std::mem::take(&mut lit)));
            }
            parts.push(Part::Var(parse_var(&after[..end])?));
            rest = &after[end + 1..];
        }
        lit.push_str(rest);
        if !lit.is_empty() {
            parts.push(Part::Lit(lit));
        }
        if parts.is_empty() {
            return Err("an alternative of the fallback chain is empty".into());
        }
        chain.push(parts);
    }
    Ok(Template { chain })
}

/// Where variable values come from.
pub trait Lookup {
    fn get(&self, v: &Var) -> Option<String>;
}

/// Values of a signing-in user's validated ID-token claims.
pub struct TokenClaims<'a> {
    pub claims: &'a Value,
    /// The verified e-mail.
    pub email: &'a str,
}

fn string_at(claims: &Value, path: &str) -> Option<String> {
    let mut cur = claims;
    for seg in path.split('.') {
        cur = cur.get(seg)?;
    }
    cur.as_str().map(str::to_owned)
}

impl Lookup for TokenClaims<'_> {
    fn get(&self, v: &Var) -> Option<String> {
        match v {
            Var::EmailLocal => self.email.split('@').next().map(str::to_owned),
            // The provider's subject: the one claim OIDC guarantees unique (D-162).
            Var::Sub => string_at(self.claims, "sub"),
            Var::PreferredUsername => string_at(self.claims, "preferred_username"),
            Var::Name => string_at(self.claims, "name"),
            Var::GivenName => string_at(self.claims, "given_name"),
            Var::FamilyName => string_at(self.claims, "family_name"),
            // Dot paths walk nested objects only; a claim literally named `a.b` is not reachable.
            Var::Claim(p) => string_at(self.claims, p),
        }
    }
}

/// The admin's typed sample for the live preview: it stands for every simple variable.
pub struct Sample<'a>(pub &'a str);

impl Lookup for Sample<'_> {
    fn get(&self, v: &Var) -> Option<String> {
        let s = self.0;
        match v {
            // A typed sample names a person; it never invents an identity (`{sub}`,
            // `{claim:…}` stay unresolved so the chain falls through — D-162/D-151(b)).
            Var::Sub | Var::Claim(_) => None,
            Var::PreferredUsername | Var::Name => Some(s.to_owned()),
            Var::EmailLocal => Some(s.split('@').next().unwrap_or(s).to_owned()),
            Var::GivenName => s.split_whitespace().next().map(str::to_owned),
            Var::FamilyName => {
                let mut w = s.split_whitespace();
                w.next()?;
                w.last().map(str::to_owned)
            }
        }
    }
}

/// Drop control characters, collapse whitespace, cap the length.
fn clean(s: &str) -> String {
    let joined: String = s
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    joined.chars().take(MAX_NAME_CHARS).collect()
}

#[derive(Debug, PartialEq, Eq)]
pub struct Rendered {
    /// The workspace name, `None` when no alternative produced one.
    pub name: Option<String>,
    /// Index of the alternative used.
    pub used: Option<usize>,
    /// Variables that had no value in an alternative that was tried and skipped.
    pub unresolved: Vec<String>,
}

fn has_alnum(s: &str) -> bool {
    s.chars().any(char::is_alphanumeric)
}

impl Template {
    pub fn render(&self, src: &dyn Lookup) -> Rendered {
        let mut unresolved = Vec::new();
        for (i, alt) in self.chain.iter().enumerate() {
            let mut out = String::new();
            let mut ok = true;
            for p in alt {
                match p {
                    Part::Lit(l) => out.push_str(l),
                    Part::Var(v) => match src.get(v).map(|x| clean(&x)).filter(|x| !x.is_empty()) {
                        Some(x) => out.push_str(&x),
                        None => {
                            let l = v.label();
                            if !unresolved.contains(&l) {
                                unresolved.push(l);
                            }
                            ok = false;
                        }
                    },
                }
            }
            let out = clean(&out);
            if ok && has_alnum(&out) {
                return Rendered {
                    name: Some(out),
                    used: Some(i),
                    unresolved,
                };
            }
        }
        Rendered {
            name: None,
            used: None,
            unresolved,
        }
    }
}

/// `user-<first 8 hex digits of the id>`: the last resort.
pub fn fallback_name(user_id: uuid::Uuid) -> String {
    format!("user-{}", &user_id.simple().to_string()[..8])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(t: &str, claims: Value, email: &str) -> Rendered {
        parse(t).unwrap().render(&TokenClaims {
            claims: &claims,
            email,
        })
    }

    #[test]
    fn variables_and_literals() {
        let r = render(
            "{given_name} {family_name} ({preferred_username})",
            json!({"given_name":"Ada","family_name":"Lovelace","preferred_username":"ada"}),
            "a@x.org",
        );
        assert_eq!(r.name.as_deref(), Some("Ada Lovelace (ada)"));
    }

    #[test]
    fn fallback_chain_skips_missing_and_blank() {
        let r = render(
            "{preferred_username}|{claim:cern.upn}|{email_local}",
            json!({"preferred_username": "  "}),
            "bobj@x.org",
        );
        assert_eq!(r.name.as_deref(), Some("bobj"));
        assert_eq!(r.used, Some(2));
        assert_eq!(r.unresolved, vec!["preferred_username", "claim:cern.upn"]);
    }

    #[test]
    fn nested_claim_and_non_string_claims() {
        let c = json!({"cern": {"upn": "bobj"}, "n": 5, "a": ["x"]});
        assert_eq!(
            render("{claim:cern.upn}", c.clone(), "e@x").name.as_deref(),
            Some("bobj")
        );
        assert_eq!(render("{claim:n}", c.clone(), "e@x").name, None);
        assert_eq!(render("{claim:a}", c, "e@x").name, None);
    }

    #[test]
    fn unusable_results_fall_through() {
        // Only punctuation: no letter or digit, so the next alternative is used.
        let r = render(
            "{name}|{email_local}",
            json!({"name": "--- !!"}),
            "real@x.org",
        );
        assert_eq!(r.name.as_deref(), Some("real"));
    }

    #[test]
    fn control_characters_and_length_are_cleaned() {
        let long = "a".repeat(200);
        let r = render(
            "{name}",
            json!({"name": format!("x\u{0}\ny\t{long}")}),
            "e@x.org",
        );
        let n = r.name.unwrap();
        assert!(n.starts_with("xyaaa"), "{n}");
        assert!(n.chars().count() <= 80 && !n.chars().any(char::is_control));
    }

    #[test]
    fn rejects_bad_templates() {
        for bad in [
            "",
            "{nope}",
            "{name",
            "name}",
            "{claim:}",
            "{claim:a b}",
            "a||b",
            "{}",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(parse(&"a".repeat(201)).is_err());
        assert!(parse("ws-{preferred_username}").is_ok());
        assert!(parse("ws-{sub}").is_ok());
    }

    #[test]
    fn sample_stands_for_simple_variables() {
        let t = parse("{given_name}-{family_name}").unwrap();
        assert_eq!(
            t.render(&Sample("Ada Lovelace")).name.as_deref(),
            Some("Ada-Lovelace")
        );
        let t = parse("{claim:x}|{sub}|{preferred_username}").unwrap();
        let r = t.render(&Sample("bobj"));
        assert_eq!(r.name.as_deref(), Some("bobj"));
        // The sample never stands for an identity variable: `sub` falls through like a claim.
        assert_eq!(r.unresolved, vec!["claim:x", "sub"]);
        assert_eq!(r.used, Some(2));
    }

    #[test]
    fn sub_renders_from_the_token_claims() {
        let r = render(
            "{sub}|{email_local}",
            json!({"sub": "s-1234"}),
            "bobj@x.org",
        );
        assert_eq!(r.name.as_deref(), Some("s-1234"));
        assert_eq!(r.used, Some(0));
        assert!(r.unresolved.is_empty());
        // A missing `sub` (non-conformant provider) falls through to the e-mail.
        let r = render("{sub}|{email_local}", json!({}), "bobj@x.org");
        assert_eq!(r.name.as_deref(), Some("bobj"));
        assert_eq!(r.unresolved, vec!["sub"]);
    }

    #[test]
    fn a_255_character_sub_is_capped_at_the_80_character_name_limit() {
        // OIDC Core §2: `sub` is at most 255 case-sensitive ASCII characters; the name is
        // cut at 80 (a collision then goes to SEC-79's numbered slug).
        let long = "s".repeat(255);
        let r = render("{sub}", json!({"sub": long}), "b@x.org");
        assert_eq!(r.name.as_deref().map(str::len), Some(80));
        assert!(
            r.name
                .as_deref()
                .is_some_and(|n| n.chars().all(|c| c == 's'))
        );
    }

    #[test]
    fn sample_path_leaves_sub_unresolved_and_the_chain_falls_through() {
        // The default template with a typed sample: `{sub}` has no value, the e-mail part does.
        let t = parse("{sub}|{email_local}").unwrap();
        let r = t.render(&Sample("bobj"));
        assert_eq!(r.name.as_deref(), Some("bobj"));
        assert_eq!(r.unresolved, vec!["sub"]);
        assert_eq!(r.used, Some(1));
        // Nothing but `{sub}`: no name, the caller falls back to `user-<short id>`.
        let t = parse("{sub}").unwrap();
        let r = t.render(&Sample("bobj"));
        assert_eq!(r.name, None);
        assert_eq!(r.unresolved, vec!["sub"]);
    }

    #[test]
    fn last_resort_name() {
        let id = uuid::Uuid::parse_str("0123456789abcdef0123456789abcdef").unwrap();
        assert_eq!(fallback_name(id), "user-01234567");
    }
}
