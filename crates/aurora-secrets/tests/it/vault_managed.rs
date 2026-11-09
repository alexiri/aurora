#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Managed mode against a real Vault dev server (`AURORA_TEST_VAULT=1`).

use aurora_core::Secret;
use aurora_secrets::{Purpose, SecretError, SecretRef, SecretStore, StoreHealth};
use uuid::Uuid;

use crate::support::{ctx, real};

fn pt(s: &str) -> Secret<Vec<u8>> {
    Secret::new(s.as_bytes().to_vec())
}

#[tokio::test]
async fn managed_round_trip_versions_and_destroy() {
    let Some(v) = real() else { return };
    let t = format!("t-{}", Uuid::now_v7().simple());
    let s = v.store(&t, &[]);
    assert_eq!(s.health().await, StoreHealth::Healthy);
    let c = ctx(Uuid::now_v7());

    let r1 = s.put(&c, &pt("first")).await.unwrap();
    let r2 = s.put(&c, &pt("second")).await.unwrap();
    let expected = format!("{t}-managed/{}/secret.value/{}", c.org_id, c.owner_id);
    assert_eq!(r1.to_column(), format!("ext:vault:{expected}@1"));
    assert_eq!(r2.to_column(), format!("ext:vault:{expected}@2"));
    // Every ref reads exactly its own version.
    assert_eq!(s.get(&c, &r1).await.unwrap().expose(), b"first");
    assert_eq!(s.get(&c, &r2).await.unwrap().expose(), b"second");

    // Raw bytes (non-UTF-8) survive the base64 layer.
    let bin = s.put(&c, &Secret::new(vec![0, 159, 255, 1])).await.unwrap();
    assert_eq!(
        s.get(&c, &bin).await.unwrap().expose(),
        &vec![0, 159, 255, 1]
    );

    // The value is stored with its context.
    let (st, body) = v.read(&expected, Some(1)).await;
    assert_eq!(st, 200);
    let d = &body["data"]["data"];
    assert_eq!(d["purpose"], "secret.value");
    assert_eq!(d["owner_id"], c.owner_id.to_string());
    assert_eq!(d["org_id"], c.org_id.to_string());

    // `delete` destroys the superseded version only.
    s.delete(&c, &r1).await.unwrap();
    assert_eq!(s.get(&c, &r1).await.unwrap_err(), SecretError::NotFound);
    assert_eq!(s.get(&c, &r2).await.unwrap().expose(), b"second");
    // Idempotent.
    s.delete(&c, &r1).await.unwrap();
}

#[tokio::test]
async fn managed_purpose_owner_and_org_mismatch_refused() {
    let Some(v) = real() else { return };
    let t = format!("t-{}", Uuid::now_v7().simple());
    let s = v.store(&t, &[]);
    let c = ctx(Uuid::now_v7());
    let r = s.put(&c, &pt("mine")).await.unwrap();

    let mut o = c;
    o.owner_id = Uuid::now_v7();
    assert_eq!(
        s.get(&o, &r).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );
    let mut o = c;
    o.org_id = Uuid::now_v7();
    assert_eq!(
        s.get(&o, &r).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );
    let mut o = c;
    o.purpose = Purpose::ConnectionCredentials;
    assert_eq!(
        s.get(&o, &r).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );
    assert_eq!(
        s.delete(&o, &r).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );
    // Still intact for the owner.
    assert_eq!(s.get(&c, &r).await.unwrap().expose(), b"mine");

    // A ref pointing at another org's managed path is refused under my context.
    let victim = ctx(Uuid::now_v7());
    let vr = s.put(&victim, &pt("theirs")).await.unwrap();
    let SecretRef::Ext { path, version, .. } = &vr else {
        panic!()
    };
    let forged = SecretRef::ext("vault", path, None, version).unwrap();
    assert_eq!(
        s.get(&c, &forged).await.unwrap_err(),
        SecretError::SecretRefMismatch
    );
}

#[tokio::test]
async fn a_bad_token_is_store_unavailable_not_a_wrong_answer() {
    let Some(v) = real() else { return };
    // Vault answers 403 to everything for an unknown token; the store reports unavailable.
    let t = format!("t-{}", Uuid::now_v7().simple());
    let dir = std::env::temp_dir().join(format!("aurora-vault-it-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    // Kubernetes login against a Vault with no such auth mount fails at login.
    std::fs::write(dir.join("jwt"), "not-a-jwt").unwrap();
    let toml = format!(
        "id = \"vault\"\nkind = \"hashicorp-vault\"\naddress = \"{}\"\nmanaged_prefix = \"{t}\"\nauth = {{ method = \"kubernetes\", role = \"x\", jwt_path = \"{}\" }}\n",
        v.addr,
        dir.join("jwt").display()
    );
    let s = crate::support::build(&toml);
    let c = ctx(Uuid::now_v7());
    assert_eq!(
        s.put(&c, &pt("x")).await.unwrap_err(),
        SecretError::StoreUnavailable
    );
    assert_eq!(s.health().await, StoreHealth::Unavailable);
    let _ = std::fs::remove_dir_all(dir);
}
