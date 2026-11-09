#![allow(clippy::unwrap_used, clippy::expect_used)]
//! SEC-61 against a real Vault dev server (`AURORA_TEST_VAULT=1`, see `support::RealVault`).

use aurora_config::{VaultAuthSettings, VaultReferenceScope, VaultSettings};
use aurora_core::Secret;
use aurora_secrets::{SecretError, SecretRef, SecretStore, VaultStore};
use serde_json::json;
use uuid::Uuid;

use crate::support::{TOKEN_ENV, ctx, real, store_config};

fn uniq() -> String {
    format!("t-{}", Uuid::now_v7().simple())
}

#[tokio::test]
async fn in_scope_reads_latest_version_after_rotation() {
    let Some(v) = real() else { return };
    let (org, t) = (Uuid::now_v7(), uniq());
    let scope = format!("{t}/*");
    let s = v.store(&t, &[(org, &[scope.as_str()])]);
    let c = ctx(org);
    let p = format!("{t}/openai");

    assert_eq!(v.write(&p, json!({ "key": "sk-1", "other": "x" })).await, 1);
    let r = s.resolve_reference(&c, &p, Some("key")).await.unwrap();
    assert_eq!(r.to_column(), format!("ext:vault:{p}#key@latest"));
    assert_eq!(s.get(&c, &r).await.unwrap().expose(), b"sk-1");

    // Rotation in Vault: the same stored ref now yields version 2 (no pin).
    assert_eq!(v.write(&p, json!({ "key": "sk-2" })).await, 2);
    assert_eq!(s.get(&c, &r).await.unwrap().expose(), b"sk-2");
    let r2: SecretRef = r.to_column().parse().unwrap();
    assert_eq!(s.get(&c, &r2).await.unwrap().expose(), b"sk-2");

    // The default field is `value`.
    v.write(&format!("{t}/plain"), json!({ "value": "pv" }))
        .await;
    let r = s
        .resolve_reference(&c, &format!("{t}/plain"), None)
        .await
        .unwrap();
    assert!(r.to_column().ends_with("#value@latest"));
    assert_eq!(s.get(&c, &r).await.unwrap().expose(), b"pv");
}

#[tokio::test]
async fn out_of_scope_and_cross_org_paths_refused_at_create_and_open() {
    let Some(v) = real() else { return };
    let (org_a, org_b, t) = (Uuid::now_v7(), Uuid::now_v7(), uniq());
    let (ga, gb) = (format!("{t}/a/*"), format!("{t}/b/*"));
    let s = v.store(&t, &[(org_a, &[ga.as_str()]), (org_b, &[gb.as_str()])]);
    let (ca, cb) = (ctx(org_a), ctx(org_b));
    let (pa, pb) = (format!("{t}/a/key"), format!("{t}/b/key"));
    v.write(&pa, json!({ "value": "A" })).await;
    v.write(&pb, json!({ "value": "B" })).await;

    // Each org reads its own path…
    let ra = s.resolve_reference(&ca, &pa, None).await.unwrap();
    let rb = s.resolve_reference(&cb, &pb, None).await.unwrap();
    assert_eq!(s.get(&ca, &ra).await.unwrap().expose(), b"A");
    assert_eq!(s.get(&cb, &rb).await.unwrap().expose(), b"B");

    // …at create: the other org's path, an unscoped path, a sibling prefix, an org with no scope.
    let stranger = ctx(Uuid::now_v7());
    for (who, p) in [
        (&ca, &pb),
        (&cb, &pa),
        (&ca, &format!("{t}/c/key")),
        (&ca, &format!("{t}/a")),
        (&stranger, &pa),
    ] {
        assert_eq!(
            s.resolve_reference(who, p, None).await.unwrap_err(),
            SecretError::ReferenceNotPermitted,
            "{p}"
        );
    }
    // …and at open: a ref made for org A, replayed under org B or a stranger (row swap).
    for who in [&cb, &stranger] {
        assert_eq!(
            s.get(who, &ra).await.unwrap_err(),
            SecretError::ReferenceNotPermitted
        );
    }
    // A hand-made ref into the other org's path is refused at open too.
    let forged = SecretRef::ext("vault", &pb, Some("value"), "latest").unwrap();
    assert_eq!(
        s.get(&ca, &forged).await.unwrap_err(),
        SecretError::ReferenceNotPermitted
    );
}

#[tokio::test]
async fn scope_removed_refuses_existing_reference_at_next_open() {
    let Some(v) = real() else { return };
    let (org, t) = (Uuid::now_v7(), uniq());
    let scope = format!("{t}/*");
    let c = ctx(org);
    let p = format!("{t}/key");
    v.write(&p, json!({ "value": "secret" })).await;

    let with_scope = v.store(&t, &[(org, &[scope.as_str()])]);
    let r = with_scope.resolve_reference(&c, &p, None).await.unwrap();
    assert_eq!(with_scope.get(&c, &r).await.unwrap().expose(), b"secret");

    // The operator removes the scope and restarts: the stored ref is refused at the next open.
    let without = v.store(&t, &[]);
    assert_eq!(
        without.get(&c, &r).await.unwrap_err(),
        SecretError::ReferenceNotPermitted
    );
    // Narrowing the glob has the same effect.
    let narrowed = v.store(&t, &[(org, &[&format!("{t}/other/*")])]);
    assert_eq!(
        narrowed.get(&c, &r).await.unwrap_err(),
        SecretError::ReferenceNotPermitted
    );
}

#[tokio::test]
async fn managed_prefix_path_refused_even_with_covering_glob() {
    let Some(v) = real() else { return };
    let (org, other_org, t) = (Uuid::now_v7(), Uuid::now_v7(), uniq());
    let prefix = format!("{t}-managed");

    // Configuration that would allow it refuses startup.
    let covering = [
        "*".to_owned(),
        format!("{}*", &prefix[..4]),
        format!("{prefix}/*"),
        format!("{prefix}/{other_org}/*"),
        prefix.clone(),
        format!("{prefix}/{other_org}/secret.value/x"),
    ];
    for glob in &covering {
        let toml = v.store_toml(&prefix, &[(org, &[glob.as_str()])]);
        let errs = aurora_config::VaultSettings::parse(&store_config(&toml)).unwrap_err();
        assert!(
            errs.iter()
                .any(|(k, _)| k == "reference_scopes[0].paths[0]"),
            "{glob}: {errs:?}"
        );
    }

    // Defence in depth: even if settings bypassed validation (built by hand), the store itself
    // refuses every path under the managed prefix, where all orgs' managed values live.
    let settings = VaultSettings {
        id: "vault".into(),
        address: v.addr.clone(),
        ca_file: None,
        auth: VaultAuthSettings::Token {
            token_env: TOKEN_ENV.into(),
        },
        kv_mount: "secret".into(),
        managed_prefix: prefix.clone(),
        reference_scopes: vec![VaultReferenceScope {
            org_id: org,
            paths: vec!["*".into()],
        }],
    };
    let s = VaultStore::new(settings, None).unwrap();
    let victim = ctx(other_org);
    let r = s
        .put(&victim, &Secret::new(b"victim-secret".to_vec()))
        .await
        .unwrap();
    let SecretRef::Ext { path, .. } = &r else {
        panic!()
    };
    assert_eq!(
        s.resolve_reference(&ctx(org), path, Some("value"))
            .await
            .unwrap_err(),
        SecretError::ReferenceNotPermitted
    );
    assert_eq!(
        s.resolve_reference(&ctx(org), &prefix, None)
            .await
            .unwrap_err(),
        SecretError::ReferenceNotPermitted
    );
    // …and a hand-made reference ref to it, opened by the attacker org.
    let forged = SecretRef::ext("vault", path, Some("value"), "latest").unwrap();
    assert_eq!(
        s.get(&ctx(org), &forged).await.unwrap_err(),
        SecretError::ReferenceNotPermitted
    );
    // The legitimate owner still opens the managed ref.
    assert_eq!(s.get(&victim, &r).await.unwrap().expose(), b"victim-secret");
}

#[tokio::test]
async fn missing_wrong_field_and_out_of_scope_give_the_same_error() {
    let Some(v) = real() else { return };
    let (org, t) = (Uuid::now_v7(), uniq());
    let scope = format!("{t}/in/*");
    let s = v.store(&t, &[(org, &[scope.as_str()])]);
    let c = ctx(org);
    v.write(
        &format!("{t}/in/s"),
        json!({ "value": "x", "num": 5, "obj": { "a": 1 } }),
    )
    .await;
    v.write(&format!("{t}/out/s"), json!({ "value": "y" }))
        .await;

    let cases = [
        (format!("{t}/in/s"), "nope"),        // wrong field
        (format!("{t}/in/s"), "num"),         // not a string
        (format!("{t}/in/s"), "obj"),         // not a string
        (format!("{t}/in/missing"), "value"), // missing secret
        (format!("{t}/out/s"), "value"),      // exists, outside the scope
        (format!("{t}/out/absent"), "value"), // absent, outside the scope
    ];
    let mut errs = Vec::new();
    for (p, f) in &cases {
        errs.push(s.resolve_reference(&c, p, Some(f)).await.unwrap_err());
    }
    assert!(
        errs.iter()
            .all(|e| *e == SecretError::ReferenceNotPermitted),
        "{errs:?}"
    );
    // The error carries nothing that tells the cases apart.
    let texts: std::collections::HashSet<_> = errs.iter().map(|e| format!("{e} / {e:?}")).collect();
    assert_eq!(texts.len(), 1);
}

#[tokio::test]
async fn scope_follows_org_id_not_a_reused_slug() {
    let Some(v) = real() else { return };
    let (old_org, new_org, t) = (Uuid::now_v7(), Uuid::now_v7(), uniq());
    let scope = format!("{t}/*");
    let p = format!("{t}/key");
    v.write(&p, json!({ "value": "x" })).await;

    // The operator granted the scope to the first org's id. Its slug is freed and re-taken by
    // a new org (a new id): the new org inherits nothing, with the same path and slug.
    let s = v.store(&t, &[(old_org, &[scope.as_str()])]);
    let r = s.resolve_reference(&ctx(old_org), &p, None).await.unwrap();
    assert_eq!(
        s.resolve_reference(&ctx(new_org), &p, None)
            .await
            .unwrap_err(),
        SecretError::ReferenceNotPermitted
    );
    assert_eq!(
        s.get(&ctx(new_org), &r).await.unwrap_err(),
        SecretError::ReferenceNotPermitted
    );

    // A slug in place of the id refuses startup.
    let toml =
        v.store_toml(&t, &[]) + "[[reference_scopes]]\norg_id = \"acme\"\npaths = [\"x/*\"]\n";
    let errs = VaultSettings::parse(&store_config(&toml)).unwrap_err();
    assert!(
        errs.iter().any(|(k, _)| k == "reference_scopes[0].org_id"),
        "{errs:?}"
    );
}
