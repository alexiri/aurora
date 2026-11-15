//! (FIX-154, D-306(c), C-895) "Credential-free": an app with nothing to sign in to or enter.
//! One pure function, used by the API (`credential_free` on `AppSummary`/`App`, the grant route)
//! and by the gateway (refusals that lead with the Access tab). Derived, never stored.

use crate::manifest::{App, Connect, CredentialDecl, Database, Instances, Token};
use crate::snapshot::AppEntry;

/// True iff the app declares no credentials, no `token`, no `instances` and no `database`, and it
/// has at least one connect method and every method is `kind: fields` with no fields, no scopes
/// and no token of its own.
pub fn is_credential_free(
    credentials: &[CredentialDecl],
    token: Option<&Token>,
    instances: Option<&Instances>,
    database: Option<&Database>,
    connect: &[Connect],
) -> bool {
    credentials.is_empty()
        && token.is_none()
        && instances.is_none()
        && database.is_none()
        && !connect.is_empty()
        && connect.iter().all(|c| match c {
            Connect::Fields(f) => f.fields.is_empty() && f.scopes.is_empty() && f.token.is_none(),
            Connect::Oauth2(_) | Connect::Hook(_) => false,
        })
}

/// The function over an installed manifest app.
pub fn app_is_credential_free(a: &App) -> bool {
    is_credential_free(
        &a.credentials,
        a.token.as_ref(),
        a.instances.as_ref(),
        a.database.as_ref(),
        &a.connect,
    )
}

/// The function over a snapshot app (the gateway's view).
pub fn entry_is_credential_free(a: &AppEntry) -> bool {
    is_credential_free(
        &a.credentials,
        a.token.as_ref(),
        a.instances.as_ref(),
        a.database.as_ref(),
        &a.connect,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;

    fn app(yaml: &str) -> App {
        serde_yaml::from_str(yaml).expect("app yaml")
    }

    /// The shape of `cern-phonebook` (external `aurora-addons-cern` repo): no credentials, one
    /// `fields` method with no fields.
    const PHONEBOOK: &str = r#"
about: { id: cern-phonebook, name: CERN Phonebook, description: d, category: directory }
connect:
  - { id: anonymous, kind: fields, fields: [] }
"#;

    #[test]
    fn a_fieldless_fields_app_is_credential_free() {
        assert!(app_is_credential_free(&app(PHONEBOOK)));
    }

    #[test]
    fn one_field_makes_it_not() {
        let a = app(r#"
about: { id: x, name: X, description: d, category: c }
connect:
  - id: pat
    kind: fields
    fields: [{ name: token, label: Token }]
"#);
        assert!(!app_is_credential_free(&a));
    }

    #[test]
    fn no_connect_methods_is_not() {
        let a = app("about: { id: x, name: X, description: d, category: c }\n");
        assert!(!app_is_credential_free(&a));
    }

    #[test]
    fn a_declared_credential_or_scope_makes_it_not() {
        let a = app(r#"
about: { id: x, name: X, description: d, category: c }
credentials: [{ name: key, secret: true }]
connect:
  - { id: anonymous, kind: fields, fields: [] }
"#);
        assert!(!app_is_credential_free(&a));
    }

    #[test]
    fn an_instance_bound_app_is_not() {
        let a = app(r#"
about: { id: x, name: X, description: d, category: c }
instances: { label: Server }
connect:
  - { id: anonymous, kind: fields, fields: [] }
"#);
        assert!(a.instances.is_some());
        assert!(!app_is_credential_free(&a));
    }

    #[test]
    fn no_builtin_app_is_credential_free() {
        let dir = format!("{}/../../plugins/builtin", env!("CARGO_MANIFEST_DIR"));
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path().join("plugin.yaml");
            let Ok(y) = std::fs::read_to_string(&p) else {
                continue;
            };
            let m = Manifest::parse(&y, "9.9.9").unwrap_or_else(|e| panic!("{p:?}: {e:?}"));
            for a in &m.apps {
                assert!(
                    !app_is_credential_free(a),
                    "{} is credential-free",
                    a.about.id
                );
                n += 1;
            }
        }
        assert!(n >= 8, "only {n} builtin apps read");
    }
}
