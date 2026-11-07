#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::field_reassign_with_default
)]

use aurora_config::{EgressEntry, KeyOrigin as CfgKeyOrigin, LlmEndpoint, RawConfig};
use aurora_llm::governance::*;
use aurora_llm::{ModelRef, ProviderRegistry};
use serde_json::json;

fn cfg(f: impl FnOnce(&mut RawConfig)) -> Instance {
    let mut c = RawConfig::default();
    c.llm.endpoints = vec![
        LlmEndpoint {
            id: "lite".into(),
            provider: "litellm".into(),
            base_url: Some("http://litellm:4000".into()),
            allowed_models: vec!["m1".into(), "m2".into()],
            key_origin: Some(CfgKeyOrigin {
                method: "endpoint".into(),
                ..Default::default()
            }),
            ..Default::default()
        },
        LlmEndpoint {
            id: "anth".into(),
            provider: "anthropic".into(),
            key_origin: Some(CfgKeyOrigin {
                method: "provider_account".into(),
                allowed_organization_ids: vec!["o1".into(), "o2".into()],
                ..Default::default()
            }),
            ..Default::default()
        },
    ];
    f(&mut c);
    let reg = ProviderRegistry::from_config(&c.llm).unwrap();
    Instance::from_config(&c, &reg).unwrap()
}

fn inst() -> Instance {
    cfg(|_| {})
}

fn codes(doc: serde_json::Value, i: &Instance) -> Vec<(&'static str, String)> {
    validate(&doc, i)
        .unwrap_err()
        .into_iter()
        .map(|e| (e.code, e.path))
        .collect()
}

#[test]
fn empty_doc_is_defaults() {
    let i = inst();
    let d = validate(&json!({}), &i).unwrap();
    assert_eq!(d, default_for_new_org(&i));
    assert_eq!(d.llm.endpoints.len(), 2);
    assert!(d.llm.endpoints["lite"].allowed);
    assert_eq!(d.llm.posture, LlmPosture::Open);
    assert!(d.egress.allowlist.is_empty());
}

#[test]
fn defaults_use_instance_minimums() {
    let i = cfg(|c| {
        c.governance.llm.minimum_posture = "sanctioned_only".into();
        c.governance.egress.minimum_posture = "credentialed_only".into();
    });
    let d = default_for_new_org(&i);
    assert_eq!(d.llm.posture, LlmPosture::SanctionedOnly);
    assert_eq!(d.egress.posture, EgressPosture::CredentialedOnly);
    assert_eq!(validate(&json!({}), &i).unwrap(), d);
}

#[test]
fn absent_endpoint_not_permitted() {
    let i = inst();
    let d = validate(&json!({"llm":{"endpoints":{"anth":{}}}}), &i).unwrap();
    let e = effective(&i, &d);
    assert!(e.endpoints["anth"].permitted);
    assert!(!e.endpoints["lite"].permitted);
    assert_eq!(e.endpoints["lite"].permitted_source, Source::Org);
}

#[test]
fn invalid_rules() {
    let i = inst();
    use aurora_llm::governance::{BELOW_MIN as B, INVALID as V};
    let c = codes(json!({"llm":{"endpoints":{"nope":{}}}}), &i);
    assert_eq!(c, [(V, "llm.endpoints.nope".into())]);
    let c = codes(
        json!({"llm":{"endpoints":{"lite":{"key_scopes":["user"]}}}}),
        &i,
    );
    assert_eq!(c, [(V, "llm.endpoints.lite.key_scopes[0]".into())]);
    let c = codes(
        json!({"llm":{"endpoints":{"lite":{"allowed_models":["m1","zz"]}}}}),
        &i,
    );
    assert_eq!(c, [(V, "llm.endpoints.lite.allowed_models[1]".into())]);
    // org origin subset of instance ids
    let c = codes(
        json!({"llm":{"endpoints":{"anth":{"origin":{"allowed_organization_ids":["o1","o9"]}}}}}),
        &i,
    );
    assert_eq!(
        c,
        [(
            V,
            "llm.endpoints.anth.origin.allowed_organization_ids[1]".into()
        )]
    );
    // endpoint-method refuses org/workspace narrowing
    let c = codes(
        json!({"llm":{"endpoints":{"lite":{"origin":{"allowed_workspace_ids":["w"]}}}}}),
        &i,
    );
    assert_eq!(
        c,
        [(V, "llm.endpoints.lite.origin.allowed_workspace_ids".into())]
    );
    // anthropic cannot do teams? provider_account rule with no team ids listed: free choice
    assert!(
        validate(
            &json!({"llm":{"endpoints":{"anth":{"origin":{"allowed_team_ids":["t"]}}}}}),
            &i
        )
        .is_ok()
    );
    assert_eq!(
        codes(json!({"llm":{"posture":"bogus"}}), &i),
        [(V, "llm.posture".into())]
    );
    assert_eq!(
        codes(
            json!({"egress":{"allowlist":[{"host":"a b"},{"host":"x","path":"nope"}]}}),
            &i
        ),
        [
            (V, "egress.allowlist[0].host".into()),
            (V, "egress.allowlist[1].path".into())
        ]
    );
    assert_eq!(codes(json!({"bogus":1}), &i), [(V, String::new())]);
    let _ = B;
}

#[test]
fn below_instance_minimum() {
    let i = cfg(|c| {
        c.governance.llm.minimum_posture = "sanctioned_only".into();
        c.governance.egress.minimum_posture = "credentialed_only".into();
    });
    let c = codes(
        json!({"llm":{"posture":"open"},"egress":{"posture":"open"}}),
        &i,
    );
    assert_eq!(
        c,
        [
            (BELOW_MIN, "llm.posture".into()),
            (BELOW_MIN, "egress.posture".into())
        ]
    );
    assert!(validate(&json!({"llm":{"posture":"sanctioned_only"}}), &i).is_ok());
}

#[test]
fn allowlist_limit_and_normalization() {
    let i = inst();
    let many: Vec<_> = (0..501)
        .map(|n| json!({"host": format!("h{n}.com")}))
        .collect();
    assert_eq!(
        codes(json!({"egress":{"allowlist":many}}), &i)[0],
        (INVALID, "egress.allowlist".into())
    );
    let ok: Vec<_> = (0..500)
        .map(|n| json!({"host": format!("h{n}.com")}))
        .collect();
    assert!(validate(&json!({"egress":{"allowlist":ok}}), &i).is_ok());
    let d = validate(
        &json!({"egress":{"allowlist":[{"host":"PyPI.org"},{"host":"pypi.org"}]}}),
        &i,
    )
    .unwrap();
    assert_eq!(d.egress.allowlist.len(), 1);
    assert_eq!(d.egress.allowlist[0].host, "pypi.org");
}

#[test]
fn instance_tightening_overrides_org_open() {
    let loose = inst();
    let d = validate(&json!({"llm":{"posture":"open"}}), &loose).unwrap();
    let tight = cfg(|c| c.governance.llm.minimum_posture = "sanctioned_only".into());
    let e = effective(&tight, &d);
    assert_eq!(e.llm_posture, LlmPosture::SanctionedOnly);
    assert_eq!(e.llm_posture_source, Source::Instance);
    let e = effective(&loose, &d);
    assert_eq!(e.llm_posture, LlmPosture::Open);
    let d2 = validate(&json!({"llm":{"posture":"sanctioned_only"}}), &loose).unwrap();
    let e = effective(&loose, &d2);
    assert_eq!(e.llm_posture_source, Source::Org);
}

#[test]
fn scopes_and_models_effective() {
    let i = inst();
    let d = validate(
        &json!({"llm":{"endpoints":{
            "lite":{"key_scopes":["workspace"],"allowed_models":["m2"]},
            "anth":{}}}}),
        &i,
    )
    .unwrap();
    let e = effective(&i, &d);
    assert!(e.key_permitted("lite", "workspace"));
    assert!(!e.key_permitted("lite", "organization"));
    assert!(e.key_permitted("anth", "organization"));
    assert!(!e.key_permitted("missing", "organization"));
    assert_eq!(e.endpoints["lite"].key_scopes_source, Source::Org);
    assert_eq!(e.endpoints["anth"].key_scopes_source, Source::Instance);
    let m = |s: &str| ModelRef::Model(s.into());
    assert!(e.model_allowed("lite", &m("m2")).is_ok());
    assert_eq!(e.model_allowed("lite", &m("m1")), Err(vec!["m2".into()]));
    assert!(e.model_allowed("lite", &ModelRef::NoModel).is_ok());
    assert!(e.model_allowed("lite", &ModelRef::Unknown).is_err());
    // no list anywhere: anything passes, even Unknown
    assert!(e.model_allowed("anth", &ModelRef::Unknown).is_ok());
    // instance list only
    let d = default_for_new_org(&i);
    let e = effective(&i, &d);
    assert!(e.model_allowed("lite", &m("m1")).is_ok());
    assert!(e.model_allowed("lite", &m("x")).is_err());
}

#[test]
fn stale_org_models_disjoint_from_instance_fail_closed() {
    let i = inst();
    let mut d = default_for_new_org(&i);
    d.llm.endpoints.get_mut("lite").unwrap().allowed_models = vec!["gone".into()];
    let e = effective(&i, &d);
    assert!(!e.endpoints["lite"].permitted);
}

#[test]
fn origin_narrowing_and_team_upgrade() {
    let i = inst();
    let d = validate(
        &json!({"llm":{"endpoints":{
            "lite":{"origin":{"allowed_team_ids":["t1"]}},
            "anth":{"origin":{"allowed_organization_ids":["o2"]}}}}}),
        &i,
    )
    .unwrap();
    let e = effective(&i, &d);
    let r = e.endpoints["lite"].origin_rule.as_ref().unwrap();
    assert_eq!(r.method, aurora_llm::OriginMethod::LitellmTeam);
    assert_eq!(r.allowed_team_ids, ["t1"]);
    assert_eq!(e.endpoints["lite"].origin_rule_source, Source::Org);
    let r = e.endpoints["anth"].origin_rule.as_ref().unwrap();
    assert_eq!(r.allowed_organization_ids, ["o2"]);

    let ok = |team: &str, v: i64| json!({"status":"verified","method":"litellm_team","team_id":team,"value_version":v,"checked_at":"2026-01-01T00:00:00Z"});
    assert!(e.origin_complies("lite", &ok("t1", 3), 3));
    assert!(!e.origin_complies("lite", &ok("t2", 3), 3));
    assert!(!e.origin_complies("lite", &ok("t1", 2), 3), "stale version");
    assert!(!e.origin_complies("lite", &json!({"status":"mismatch","value_version":3}), 3));
    assert!(!e.origin_complies("lite", &json!({"junk":1}), 3));
    let a = |o: &str| json!({"status":"verified","organization_id":o,"value_version":1});
    assert!(e.origin_complies("anth", &a("o2"), 1));
    assert!(
        !e.origin_complies("anth", &a("o1"), 1),
        "org narrowed to o2"
    );
}

#[test]
fn origin_endpoint_method_and_no_rule() {
    let i = inst();
    let e = effective(&i, &default_for_new_org(&i));
    let v = json!({"status":"verified","method":"endpoint","value_version":1});
    assert!(e.origin_complies("lite", &v, 1));
    assert!(!e.origin_complies("lite", &v, 2));
    let i2 = cfg(|c| c.llm.endpoints[0].key_origin = None);
    let e2 = effective(&i2, &default_for_new_org(&i2));
    assert!(e2.origin_complies("lite", &json!(null), 9));
    // no instance rule: litellm may pin teams, not org/workspace ids
    let d = validate(
        &json!({"llm":{"endpoints":{"lite":{"origin":{"allowed_team_ids":["t"]}}}}}),
        &i2,
    )
    .unwrap();
    let r = effective(&i2, &d).endpoints["lite"]
        .origin_rule
        .clone()
        .unwrap();
    assert_eq!(r.method, aurora_llm::OriginMethod::LitellmTeam);
    assert_eq!(r.allowed_team_ids, ["t"]);
    assert_eq!(
        codes(
            json!({"llm":{"endpoints":{"lite":{"origin":{"allowed_organization_ids":["o"]}}}}}),
            &i2
        ),
        [(INVALID, "llm.endpoints.lite.origin".into())]
    );
    // anthropic with no instance rule: provider_account with the org's ids
    let i3 = cfg(|c| c.llm.endpoints[1].key_origin = None);
    let d = validate(
        &json!({"llm":{"endpoints":{"anth":{"origin":{"allowed_organization_ids":["o7"]}}}}}),
        &i3,
    )
    .unwrap();
    let e3 = effective(&i3, &d);
    let r = e3.endpoints["anth"].origin_rule.clone().unwrap();
    assert_eq!(r.method, aurora_llm::OriginMethod::ProviderAccount);
    assert_eq!(r.allowed_organization_ids, ["o7"]);
    let a = |o: &str| json!({"status":"verified","organization_id":o,"value_version":1});
    assert!(e3.origin_complies("anth", &a("o7"), 1));
    assert!(!e3.origin_complies("anth", &a("o8"), 1));
    assert_eq!(
        codes(
            json!({"llm":{"endpoints":{"anth":{"origin":{"allowed_team_ids":["t"]}}}}}),
            &i3
        ),
        [(INVALID, "llm.endpoints.anth.origin".into())]
    );
}

#[test]
fn hosts() {
    let i = inst();
    let e = effective(&i, &default_for_new_org(&i));
    assert_eq!(e.endpoint_for_host("litellm:4000"), Some("lite"));
    assert_eq!(e.endpoint_for_host("LiteLLM:4000"), Some("lite"));
    assert_eq!(e.endpoint_for_host("litellm"), None);
    assert_eq!(e.endpoint_for_host("api.anthropic.com"), Some("anth"));
    assert_eq!(e.endpoint_for_host("api.anthropic.com:443"), Some("anth"));
    assert!(e.is_known_llm_host("api.openai.com"));
    assert!(!e.is_known_llm_host("api.anthropic.com"));
    assert!(!e.is_known_llm_host("example.com"));
    assert!(e.governed_hosts.iter().any(|h| h == "litellm:4000"));
    assert!(e.governed_hosts.iter().any(|h| h == "api.openai.com"));
}

#[test]
fn egress_union_and_matching() {
    let i = cfg(|c| {
        c.governance.egress.baseline_allowlist = vec![EgressEntry {
            host: "crates.io".into(),
            path: None,
        }];
    });
    let d = validate(
        &json!({"egress":{"allowlist":[
            {"host":"*.githubusercontent.com","path":"/**"},
            {"host":"pypi.org","path":"/simple/*"}]}}),
        &i,
    )
    .unwrap();
    let e = effective(&i, &d);
    assert_eq!(e.egress_allowlist.len(), 3);
    assert!(e.egress_allowlisted("crates.io", "/any/thing"));
    assert!(e.egress_allowlisted("raw.githubusercontent.com", "/a/b"));
    assert!(!e.egress_allowlisted("githubusercontent.com", "/a"));
    assert!(e.egress_allowlisted("pypi.org", "/simple/requests"));
    assert!(!e.egress_allowlisted("pypi.org", "/other"));
    assert!(!e.egress_allowlisted("evil.com", "/"));
}

#[test]
fn stored_origin_roundtrip() {
    let o: StoredOrigin = serde_json::from_value(json!({
        "status":"verified","method":"provider_account","organization_id":"o","value_version":4,"checked_at":"t"
    }))
    .unwrap();
    assert_eq!(o.status, OriginStatus::Verified);
    assert_eq!(o.to_key_origin().organization_id.as_deref(), Some("o"));
}

#[test]
fn stored_empty_document_means_instance_defaults() {
    let inst = inst();
    let doc = aurora_llm::governance::from_stored(&serde_json::json!({}), &inst).unwrap();
    assert_eq!(doc, aurora_llm::governance::default_for_new_org(&inst));
    let eff = aurora_llm::governance::effective(&inst, &doc);
    assert!(
        eff.endpoints.values().all(|e| e.permitted),
        "all instance endpoints permitted by default"
    );
    // An explicit llm section keeps "absent endpoint = not permitted".
    let explicit = aurora_llm::governance::from_stored(
        &serde_json::json!({"llm": {"posture": "open", "endpoints": {}}}),
        &inst,
    )
    .unwrap();
    let eff = aurora_llm::governance::effective(&inst, &explicit);
    assert!(eff.endpoints.values().all(|e| !e.permitted));
}

// ---- RFE-003: open-lane posture ----

#[test]
fn open_lane_defaults_to_the_instance_minimum_and_is_tighten_only() {
    let i = inst();
    assert_eq!(i.open_lane_minimum, OpenLanePosture::Enabled);
    assert_eq!(
        default_for_new_org(&i).egress.open_lane,
        OpenLanePosture::Enabled
    );
    let d = validate(&json!({"egress": {"open_lane": "allowlist_only"}}), &i).unwrap();
    assert_eq!(d.egress.open_lane, OpenLanePosture::AllowlistOnly);
    assert_eq!(
        codes(json!({"egress": {"open_lane": "maybe"}}), &i),
        [("governance_invalid", "egress.open_lane".to_owned())]
    );
    let strict = cfg(|c| c.governance.egress.open_lane_minimum = "allowlist_only".into());
    assert_eq!(
        default_for_new_org(&strict).egress.open_lane,
        OpenLanePosture::AllowlistOnly
    );
    assert_eq!(
        codes(json!({"egress": {"open_lane": "enabled"}}), &strict),
        [(BELOW_MIN, "egress.open_lane".to_owned())]
    );
    assert!(validate(&json!({"egress": {"open_lane": "disabled"}}), &strict).is_ok());
}

#[test]
fn open_lane_effective_is_the_strictest_and_stored_phase0_documents_read_as_enabled() {
    let i = cfg(|c| c.governance.egress.open_lane_minimum = "allowlist_only".into());
    // A phase-0 stored egress section has no `open_lane`: enabled, raised by the instance.
    let org = from_stored(&json!({"egress": {"posture": "open", "allowlist": []}}), &i).unwrap();
    assert_eq!(org.egress.open_lane, OpenLanePosture::Enabled);
    let e = effective(&i, &org);
    assert_eq!(e.open_lane, OpenLanePosture::AllowlistOnly);
    assert_eq!(e.open_lane_source, Source::Instance);
    let org = from_stored(&json!({"egress": {"open_lane": "disabled"}}), &i).unwrap();
    let e = effective(&i, &org);
    assert_eq!(e.open_lane, OpenLanePosture::Disabled);
    assert_eq!(e.open_lane_source, Source::Org);
}

#[test]
fn open_lane_admission_levels_and_path_entries() {
    use aurora_policy::eval::{AllowEntry as PolicyEntry, ScopePolicy};
    use aurora_policy::{HostPattern, PathPattern, RequestFacts, RequestIdentity};
    let facts = RequestFacts {
        host: "pypi.org".into(),
        port: 443,
        path: "/".into(),
        query: None,
        method: "CONNECT".into(),
        body_prefix: vec![],
        truncated: false,
        app_id: None,
        tool_id: None,
        tool_tags: Default::default(),
        winning_credential: None,
        is_llm_host: false,
        identity: RequestIdentity::default(),
        connect_time: true,
        host_apps: vec![],
    };
    let none = ScopePolicy::default();
    let listed = |path: Option<&str>| ScopePolicy {
        uncredentialed_allowlist: vec![PolicyEntry {
            host: HostPattern::lenient("pypi.org"),
            path: path.map(PathPattern::lenient),
        }],
        ..ScopePolicy::default()
    };
    let i = inst();
    let doc = |open_lane: &str, list: serde_json::Value| {
        from_stored(
            &json!({"egress": {"open_lane": open_lane, "allowlist": list}}),
            &i,
        )
        .unwrap()
    };
    let adm = |o: &OrgGovernance, ws, wsp: &ScopePolicy| {
        effective(&i, o).open_lane_admission(ws, &none, wsp, &facts)
    };
    use OpenLaneAdmission::*;
    use OpenLanePosture as P;
    assert_eq!(adm(&doc("enabled", json!([])), P::Enabled, &none), Admitted);
    assert_eq!(
        adm(&doc("disabled", json!([])), P::Enabled, &none),
        Disabled
    );
    assert_eq!(
        adm(&doc("enabled", json!([])), P::Disabled, &none),
        Disabled
    );
    // org level
    assert_eq!(
        adm(&doc("allowlist_only", json!([])), P::Enabled, &none),
        NotAllowlisted
    );
    assert_eq!(
        adm(
            &doc("allowlist_only", json!([{"host": "pypi.org"}])),
            P::Enabled,
            &none
        ),
        Admitted
    );
    assert_eq!(
        adm(
            &doc(
                "allowlist_only",
                json!([{"host": "pypi.org", "path": "/**"}])
            ),
            P::Enabled,
            &none
        ),
        NotAllowlisted
    );
    // workspace level, and both levels must cover
    assert_eq!(
        adm(&doc("enabled", json!([])), P::AllowlistOnly, &none),
        NotAllowlisted
    );
    assert_eq!(
        adm(&doc("enabled", json!([])), P::AllowlistOnly, &listed(None)),
        Admitted
    );
    assert_eq!(
        adm(
            &doc("enabled", json!([])),
            P::AllowlistOnly,
            &listed(Some("/x"))
        ),
        NotAllowlisted
    );
    assert_eq!(
        adm(
            &doc("allowlist_only", json!([{"host": "pypi.org"}])),
            P::AllowlistOnly,
            &none
        ),
        NotAllowlisted
    );
}
