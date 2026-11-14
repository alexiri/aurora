#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use aurora_registry::Manifest;
use aurora_registry::claims::{claims, conflict_groups, conflicts, hosts_overlap};
use aurora_registry::diff::{Widening, widens};
use aurora_registry::index::{HostIndex, Snapshot};
use aurora_registry::manifest::Hook;
use aurora_registry::snapshot::{ActivePackage, CompileError, Decision, compile, select_active};

fn pkg_yaml(pkg: &str, app: &str, hosts: &[(&str, Option<&str>)], extra_perm: &str) -> String {
    let mut s = format!(
        "package: {{ id: {pkg}, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: P }}\n{extra_perm}\napps:\n  - about: {{ id: {app}, name: A }}\n    credentials: [ {{ name: api_token, secret: true }} ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ {{ name: api_token, label: T, type: secret, required: true }} ]\n    hosts:\n"
    );
    for (h, p) in hosts {
        let pp = p.map(|p| format!(", path_prefix: {p}")).unwrap_or_default();
        s += &format!(
            "      - match: {{ host: \"{h}\"{pp} }}\n        inject:\n          - bearer: \"{{credentials.api_token}}\"\n"
        );
    }
    s
}

fn manifest(y: &str) -> Manifest {
    Manifest::parse(y, "0.1.0").unwrap()
}

fn active(pkg: &str, app: &str, hosts: &[(&str, Option<&str>)]) -> ActivePackage {
    ActivePackage {
        manifest: manifest(&pkg_yaml(pkg, app, hosts, "")),
        digest: format!("d-{pkg}"),
        publisher: "aurora".into(),
        hooks_digest: None,
    }
}

fn tagged(p: &ActivePackage) -> Vec<(String, aurora_registry::claims::Claim)> {
    claims(&p.manifest)
        .into_iter()
        .map(|c| (p.manifest.package.id.clone(), c))
        .collect()
}

fn all(ps: &[&ActivePackage]) -> Vec<(String, aurora_registry::claims::Claim)> {
    ps.iter().flat_map(|p| tagged(p)).collect()
}

#[test]
fn deterministic_across_orders_and_sorted_keys() {
    let a = active("pa", "aa", &[("api.a.com", None)]);
    let b = active("pb", "bb", &[("api.b.com", Some("/x")), ("b.com", None)]);
    let c = active("pc", "cc", &[("c.com", None)]);
    let r = vec!["z.com".to_owned(), "a.com".to_owned()];
    let s1 = compile(&[a, b, c], &[], &r, 1).unwrap();
    let a = active("pa", "aa", &[("api.a.com", None)]);
    let b = active("pb", "bb", &[("b.com", None), ("api.b.com", Some("/x"))]);
    let c = active("pc", "cc", &[("c.com", None)]);
    let s2 = compile(
        &[c, b, a],
        &[],
        &["a.com".to_owned(), "z.com".to_owned()],
        1,
    )
    .unwrap();
    assert_eq!(s1.json, s2.json);
    assert_eq!(s1.hash, s2.hash);
    let v: serde_json::Value = serde_json::from_slice(&s1.json).unwrap();
    let txt = String::from_utf8(s1.json.clone()).unwrap();
    assert!(txt.starts_with("{\"apps\":["), "{txt}");
    assert_eq!(v["reserved_hosts"][0], "a.com");
    // roundtrip through the typed snapshot
    let snap = Snapshot::from_slice(&s1.json).unwrap();
    assert_eq!(snap.apps.len(), 3);
    assert!(snap.app("bb").is_some());
    assert_eq!(snap.hosts[0].host, "api.a.com");
}

#[test]
fn overlap_rules() {
    assert!(hosts_overlap("api.x.com", "api.x.com"));
    assert!(hosts_overlap("*.x.com", "a.x.com"));
    assert!(hosts_overlap("*.x.com", "api.*"));
    assert!(hosts_overlap("*.x.com", "*.com"));
    assert!(!hosts_overlap("*.x.com", "x.com"));
    assert!(!hosts_overlap("*.x.com", "*.y.com"));
    assert!(!hosts_overlap("a.x.com:8443", "a.x.com:443"));
    assert!(hosts_overlap("a.x.com", "a.x.com:443"));
}

#[test]
fn conflicts_detected_and_disjoint_paths_ok() {
    let a = active("pa", "aa", &[("api.x.com", None)]);
    let b = active("pb", "bb", &[("api.x.com", Some("/v1"))]);
    assert_eq!(conflicts(&all(&[&a, &b])).len(), 1);
    let c = active("pc", "cc", &[("*.x.com", None)]);
    let d = active("pd", "dd", &[("a.x.com", None)]);
    let cs = conflicts(&all(&[&c, &d]));
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].claim_key_a, "*.x.com|/");
    let e = active("pe", "ee", &[("api.x.com", Some("/a/"))]);
    let f = active("pf", "ff", &[("api.x.com", Some("/b/"))]);
    assert!(conflicts(&all(&[&e, &f])).is_empty());
    // same package never conflicts with itself
    let g = active("pg", "gg", &[("api.x.com", None), ("*.x.com", None)]);
    assert!(conflicts(&all(&[&g])).is_empty());
}

#[test]
fn decision_resolves_and_fingerprint_change_invalidates() {
    let mk = |bearer_host: &str| {
        vec![
            active("pa", "aa", &[("api.x.com", None)]),
            active("pb", "bb", &[(bearer_host, None)]),
        ]
    };
    let pk = mk("api.x.com");
    let err = compile(&pk, &[], &[], 1).unwrap_err();
    let CompileError::UnresolvedConflicts(groups) = err else {
        panic!("expected conflicts")
    };
    assert_eq!(groups.len(), 1);
    let g = &groups[0];
    let d = Decision {
        claim_key: g.claim_key.clone(),
        fingerprint: g.fingerprint.clone(),
        winner_package_id: "pb".into(),
    };
    let s = compile(&pk, std::slice::from_ref(&d), &[], 1).unwrap();
    let snap = Snapshot::from_slice(&s.json).unwrap();
    let status = |app: &str| {
        format!(
            "{:?}",
            snap.hosts
                .iter()
                .find(|h| h.app_id == app)
                .unwrap()
                .claim_status
        )
    };
    assert_eq!(status("aa"), "Lost");
    assert_eq!(status("bb"), "Active");
    assert!(
        String::from_utf8(s.json)
            .unwrap()
            .contains("\"claim_status\":\"lost\"")
    );

    // change pa's claim definition (different inject) => fingerprint changes
    let mut changed = mk("api.x.com");
    changed[0].manifest.apps[0].hosts[0].passthrough =
        vec![aurora_registry::manifest::Passthrough::RegistryTokenChallenge];
    assert!(matches!(
        compile(&changed, std::slice::from_ref(&d), &[], 1),
        Err(CompileError::UnresolvedConflicts(_))
    ));
    // winner not in group
    let bad = Decision {
        winner_package_id: "zz".into(),
        ..d
    };
    assert!(compile(&pk, &[bad], &[], 1).is_err());
    assert_eq!(conflict_groups(&all(&[&pk[0], &pk[1]])).len(), 1);
}

fn perm(hooks: &str, egress: &str, ctx: &str) -> Manifest {
    manifest(&pkg_yaml(
        "pa",
        "aa",
        &[("api.x.com", None)],
        &format!("permissions:\n  hooks: [{hooks}]\n  egress: {{ {egress} }}\n  context: [{ctx}]"),
    ))
}

#[test]
fn widening_table() {
    let base = perm("token.mint", "token.mint: [api.x.com]", "org_id");
    assert!(widens(&base, &base).is_empty());
    let hook = perm(
        "token.mint, connect.exchange",
        "token.mint: [api.x.com]",
        "org_id",
    );
    assert_eq!(
        widens(&base, &hook),
        vec![Widening::Hook(Hook::ConnectExchange)]
    );
    let egress = perm("token.mint", "token.mint: [api.x.com, evil.com]", "org_id");
    assert_eq!(
        widens(&base, &egress),
        vec![Widening::Egress {
            hook: Hook::TokenMint,
            host: "evil.com".into()
        }]
    );
    let ctx = perm(
        "token.mint",
        "token.mint: [api.x.com]",
        "org_id, agent_name",
    );
    assert_eq!(
        widens(&base, &ctx),
        vec![Widening::Context("agent_name".into())]
    );
    let claim = manifest(&pkg_yaml(
        "pa",
        "aa",
        &[("api.x.com", None), ("other.com", None)],
        "permissions:\n  hooks: [token.mint]\n  egress: { token.mint: [api.x.com] }\n  context: [org_id]",
    ));
    assert_eq!(
        widens(&base, &claim),
        vec![Widening::HostClaim("other.com|/".into())]
    );
    // narrower => empty
    let narrow = perm("token.mint", "token.mint: []", "");
    assert!(widens(&base, &narrow).is_empty());
}

#[test]
fn widening_zone() {
    let old = manifest(&pkg_yaml("pa", "aa", &[("api.x.com", None)], ""));
    let mut new = old.clone();
    new.apps[0].hosts[0].tenant = Some(aurora_registry::manifest::Tenant {
        field: "connection.tenant_host".into(),
        zone: "*.x.com".into(),
    });
    assert_eq!(widens(&old, &new), vec![Widening::Zone("*.x.com".into())]);
}

/// (FIX-107, SEC-17) A static token endpoint on a new host, or a new `token_hosts` entry, is a
/// widening; the same host (or a narrower set) is not.
#[test]
fn widening_token_host() {
    let with = |url: &str, extra: &str| {
        manifest(&pkg_yaml(
            "pa",
            "aa",
            &[("api.x.com", None)],
            &format!(
                "oauth_clients:\n  - id: cli\n    authorize_url: https://api.x.com/authorize\n    token_url: {url}\n{extra}"
            ),
        ))
    };
    let old = with("https://api.x.com/token", "");
    assert!(widens(&old, &old).is_empty());
    assert!(widens(&old, &with("https://api.x.com/other-path", "")).is_empty());
    assert_eq!(
        widens(&old, &with("https://evil.example/token", "")),
        vec![Widening::TokenHost("evil.example".into())]
    );
    let listed = with(
        "https://api.x.com/token",
        "permissions:\n  token_hosts: [auth.x.com]\n",
    );
    assert_eq!(
        widens(&old, &listed),
        vec![Widening::TokenHost("auth.x.com".into())]
    );
    assert!(widens(&listed, &old).is_empty());
}

fn snap(hosts: &[(&str, Option<&str>, &str)], lost: &[&str]) -> Snapshot {
    // apps need unique ids; build one package per app
    let pk: Vec<ActivePackage> = hosts
        .iter()
        .map(|(h, p, app)| active(&format!("p{app}"), app, &[(h, *p)]))
        .collect();
    let mut s = Snapshot::from_slice(&compile_ignoring(&pk)).unwrap();
    for h in &mut s.hosts {
        if lost.contains(&h.app_id.as_str()) {
            h.claim_status = aurora_registry::snapshot::ClaimStatus::Lost;
        }
    }
    s
}

/// Compile resolving every conflict in favour of the first member package.
fn compile_ignoring(pk: &[ActivePackage]) -> Vec<u8> {
    let t: Vec<_> = pk.iter().flat_map(tagged).collect();
    let ds: Vec<Decision> = conflict_groups(&t)
        .into_iter()
        .map(|g| Decision {
            claim_key: g.claim_key,
            fingerprint: g.fingerprint,
            winner_package_id: g.members[0].package_id.clone(),
        })
        .collect();
    let mut s: serde_json::Value =
        serde_json::from_slice(&compile(pk, &ds, &[], 1).unwrap().json).unwrap();
    for h in s["hosts"].as_array_mut().unwrap() {
        h["claim_status"] = "active".into();
    }
    serde_json::to_vec(&s).unwrap()
}

#[test]
fn index_precedence() {
    let s = snap(
        &[
            ("*.x.com", None, "wild"),
            ("*.api.x.com", None, "wild2"),
            ("a.api.x.com", Some("/v1"), "exact"),
            ("a.api.x.com", Some("/v1/long"), "exactlong"),
        ],
        &[],
    );
    let ix = HostIndex::new(&s);
    assert_eq!(
        ix.lookup("a.api.x.com", "/v1/long/z").unwrap().app_id,
        "exactlong"
    );
    assert_eq!(ix.lookup("a.api.x.com", "/v1/z").unwrap().app_id, "exact");
    // exact does not match the path => fall back to suffix tier, longest suffix
    assert_eq!(ix.lookup("a.api.x.com", "/other").unwrap().app_id, "wild2");
    assert_eq!(ix.lookup("b.x.com", "/").unwrap().app_id, "wild");
    assert!(ix.lookup("y.com", "/").is_none());
}

#[test]
fn index_skips_lost_and_longest_path() {
    let s = snap(
        &[
            ("*.x.com", Some("/a"), "short"),
            ("*.x.com", Some("/a/b"), "long"),
        ],
        &["long"],
    );
    let ix = HostIndex::new(&s);
    assert_eq!(ix.lookup("q.x.com", "/a/b/c").unwrap().app_id, "short");
}

#[test]
fn tool_lookup() {
    let y = format!(
        "{}    tools:\n      read:\n        items:\n          - {{ id: t1, name: T1, host: api.x.com, path: /rest/items, methods: [GET] }}\n",
        pkg_yaml("pa", "aa", &[("api.x.com", Some("/rest"))], "")
    );
    let m = manifest(&y);
    let p = ActivePackage {
        manifest: m,
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: None,
    };
    let s = Snapshot::from_slice(&compile(&[p], &[], &[], 1).unwrap().json).unwrap();
    let ix = HostIndex::new(&s);
    assert_eq!(
        ix.tool("api.x.com", "/rest/items", "get").unwrap().1.id,
        "t1"
    );
    assert!(ix.tool("api.x.com", "/rest/items", "POST").is_none());
    assert!(ix.tool("api.x.com", "/rest/other", "GET").is_none());
}

#[test]
fn select_active_rule() {
    let c = vec![
        (1u32, 10i64, "h1".to_owned()),
        (1, 20, "h2".to_owned()),
        (2, 5, "h3".to_owned()),
        (3, 99, "h4".to_owned()),
    ];
    assert_eq!(select_active(&c, 2), Some("h3"));
    assert_eq!(select_active(&c, 1), Some("h2"));
    assert_eq!(select_active(&c, 3), Some("h4"));
    assert_eq!(select_active(&c[..2], 3), None);
    assert_eq!(select_active::<i64>(&[], 1), None);
}

#[test]
fn full_fixture_roundtrips() {
    let y = std::fs::read_to_string(format!(
        "{}/tests/fixtures/manifests/valid-full.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let p = ActivePackage {
        manifest: manifest(&y),
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: Some("h".into()),
    };
    let c = compile(&[p], &[], &[], 1).unwrap();
    let snap = Snapshot::from_slice(&c.json).unwrap();
    let again = serde_json::to_value(&snap).unwrap();
    assert_eq!(aurora_registry::snapshot::canonical_bytes(&again), c.json);
}

/// (FIX-123, D-195(a)) A fresh instance: every shipped built-in package compiles into one snapshot
/// with NO host-claim decision, and there is one GitHub and one AWS app (the former GitHub App and
/// assume-role apps are connect methods, not apps of their own).
#[test]
fn the_shipped_builtin_packages_compile_without_a_host_claim_decision() {
    let dir = format!("{}/../../plugins/builtin", env!("CARGO_MANIFEST_DIR"));
    let mut pkgs = Vec::new();
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path().join("plugin.yaml");
        if !p.is_file() {
            continue;
        }
        let m = Manifest::parse(&std::fs::read_to_string(&p).unwrap(), "9.9.9").unwrap();
        pkgs.push(ActivePackage {
            digest: format!("d-{}", m.package.id),
            manifest: m,
            publisher: "aurora".into(),
            hooks_digest: Some("h".into()),
        });
    }
    let c = compile(&pkgs, &[], &[], 1)
        .unwrap_or_else(|e| panic!("built-ins conflict on a fresh instance: {e:?}"));
    let snap = Snapshot::from_slice(&c.json).unwrap();
    let ids: Vec<&str> = snap.apps.iter().map(|a| a.app_id.as_str()).collect();
    for retired in ["github-app", "github-app-ghes", "aws-role"] {
        assert!(
            !ids.contains(&retired),
            "{retired} is a connect method now: {ids:?}"
        );
    }
    for app in ["github", "github-ghes", "aws-keys"] {
        assert!(ids.contains(&app), "{app} missing: {ids:?}");
    }
    let gh = snap.apps.iter().find(|a| a.app_id == "github").unwrap();
    let methods: Vec<&str> = gh.connect.iter().map(|c| c.id()).collect();
    assert_eq!(methods, ["github_app", "token", "oauth"]);
    assert_eq!(gh.connect.iter().filter(|c| c.recommended()).count(), 1);
}
