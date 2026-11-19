use super::*;
use proptest::prelude::*;
use serde_json::{Value, json};

fn slot(name: &str, ty: SlotType, required: bool) -> Slot {
    Slot {
        name: name.to_owned(),
        ty,
        required,
        description: String::new(),
        max_length: None,
        max_items: None,
        r#enum: None,
        schema: None,
        example: None,
    }
}

fn cfg(ct: ContentType, template: &str, slots: Vec<Slot>) -> WebhookConfig {
    WebhookConfig {
        version: 1,
        method: Method::Post,
        content_type: ct,
        template: template.to_owned(),
        slots,
        headers: Vec::new(),
        description: String::new(),
        signing: false,
        rate: Rate::default(),
        preset: None,
    }
}

fn names(n: &[&str]) -> Vec<String> {
    n.iter().map(|s| (*s).to_owned()).collect()
}

fn codes(c: &WebhookConfig, secrets: &[&str]) -> Vec<(String, &'static str)> {
    match validate(c, &names(secrets)) {
        Ok(_) => Vec::new(),
        Err(e) => e.into_iter().map(|f| (f.path, f.code)).collect(),
    }
}

fn has(c: &WebhookConfig, secrets: &[&str], code: &str) -> bool {
    codes(c, secrets).iter().any(|(_, k)| *k == code)
}

fn compiled(c: &WebhookConfig, secrets: &[&str]) -> CompiledConfig {
    validate(c, &names(secrets)).unwrap_or_else(|e| panic!("{e:?}"))
}

fn render_with(c: &WebhookConfig, values: Value, secrets: &[(&str, &str)]) -> Rendered {
    let names: Vec<&str> = secrets.iter().map(|(n, _)| *n).collect();
    let cc = compiled(c, &names);
    let checked = check_values(&cc, &values).unwrap();
    let sv = SecretValues(
        secrets
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect(),
    );
    render(&cc, &checked, &sv).unwrap()
}

fn body(c: &WebhookConfig, values: Value) -> String {
    String::from_utf8(render_with(c, values, &[]).body).unwrap()
}

// ---- validation: every C-909 code has a failing config -------------------------------------

#[test]
fn a_valid_config_compiles() {
    let c = cfg(
        ContentType::Json,
        r#"{"username":"Aurora","text":{{text}},"n":"{{n}}"}"#,
        vec![
            slot("text", SlotType::String, true),
            slot("n", SlotType::Integer, false),
        ],
    );
    assert!(validate(&c, &[]).is_ok());
}

#[test]
fn template_json_is_refused() {
    for t in [
        r#"{"a": }"#,
        r#"{"a": {{x}}"#,
        r#"{{{x}}: 1}"#,
        r#"{"{{x}}": 1}"#,
        r#"{"a": {{x}} {{x}}}"#,
        r#"{"a": nope}"#,
        r#"{"a": "line
break"}"#,
        r#"{"a": "\ud83d{{x}}"}"#,
        r#"[1, 2,]"#,
        "",
        r#"{"a":1} trailing"#,
        r#"{"a": 12{{x}}}"#,
    ] {
        let c = cfg(
            ContentType::Json,
            t,
            vec![slot("x", SlotType::String, true)],
        );
        assert!(
            has(&c, &[], "template_json") || has(&c, &[], "template_syntax"),
            "{t}"
        );
    }
    let c = cfg(ContentType::Json, r#"{"a": }"#, vec![]);
    assert!(has(&c, &[], "template_json"));
}

#[test]
fn template_syntax_is_refused() {
    for t in [
        r#"{"a":"{{x"}"#,
        r#"{"a":"{{X}}"}"#,
        r#"{"a":"{{ x }}"}"#,
        r#"{"a":"{{secret:}}"}"#,
    ] {
        let c = cfg(
            ContentType::Json,
            t,
            vec![slot("x", SlotType::String, true)],
        );
        assert!(has(&c, &[], "template_syntax"), "{t}");
    }
    let c = cfg(ContentType::Form, "a", vec![]);
    assert!(has(&c, &[], "template_syntax"));
    let c = cfg(
        ContentType::Form,
        "{{x}}=1",
        vec![slot("x", SlotType::String, true)],
    );
    assert!(has(&c, &[], "template_syntax"));
    let c = cfg(ContentType::Form, "a=1&&b=2", vec![]);
    assert!(has(&c, &[], "template_syntax"));
}

#[test]
fn placeholder_unknown_is_refused() {
    let c = cfg(ContentType::Json, r#"{"a":{{nope}}}"#, vec![]);
    assert!(has(&c, &[], "placeholder_unknown"));
    let c = cfg(ContentType::Json, r#"{"a":"{{secret:nope}}"}"#, vec![]);
    assert!(has(&c, &[], "placeholder_unknown"));
    let c = cfg(ContentType::Text, "hi {{nope}}", vec![]);
    assert!(has(&c, &[], "placeholder_unknown"));
}

#[test]
fn unused_slots_and_secrets_are_refused() {
    let c = cfg(
        ContentType::Json,
        r#"{"a":1}"#,
        vec![slot("x", SlotType::String, true)],
    );
    assert!(has(&c, &[], "slot_unused"));
    let c = cfg(ContentType::Json, r#"{"a":1}"#, vec![]);
    assert!(has(&c, &["token"], "secret_unused"));
    // A secret used only in a header is used.
    let mut c = cfg(ContentType::Json, r#"{"a":1}"#, vec![]);
    c.headers.push(HeaderTemplate {
        name: "Authorization".into(),
        value: "Bearer {{secret:token}}".into(),
    });
    assert!(validate(&c, &names(&["token"])).is_ok());
}

#[test]
fn slot_names_are_checked() {
    for bad in ["", "Text", "1a", "a-b", "a b", &"a".repeat(41)] {
        let c = cfg(
            ContentType::Json,
            "{}",
            vec![slot(bad, SlotType::String, true)],
        );
        assert!(has(&c, &[], "slot_name"), "{bad:?}");
    }
    let c = cfg(ContentType::Json, "{}", vec![]);
    assert!(has(&c, &["Bad"], "slot_name"));
    let long = "a".repeat(40);
    let c = cfg(
        ContentType::Json,
        &format!(r#"{{"a":{{{{{long}}}}}}}"#),
        vec![slot(&long, SlotType::String, true)],
    );
    assert!(validate(&c, &[]).is_ok());
}

#[test]
fn duplicates_are_refused() {
    let c = cfg(
        ContentType::Json,
        r#"{"a":{{x}}}"#,
        vec![
            slot("x", SlotType::String, true),
            slot("x", SlotType::String, true),
        ],
    );
    assert!(has(&c, &[], "slot_duplicate"));
    let c = cfg(ContentType::Json, r#"{"a":"{{secret:k}}"}"#, vec![]);
    assert!(has(&c, &["k", "k"], "slot_duplicate"));
}

#[test]
fn json_slots_are_refused_for_form_and_text() {
    let j = || vec![slot("x", SlotType::Json, true)];
    assert!(has(
        &cfg(ContentType::Form, "a={{x}}", j()),
        &[],
        "slot_type_content_type"
    ));
    assert!(has(
        &cfg(ContentType::Text, "a {{x}}", j()),
        &[],
        "slot_type_content_type"
    ));
    // Declared but unused in form is refused for its type as well.
    assert!(has(
        &cfg(ContentType::Form, "a=1", j()),
        &[],
        "slot_type_content_type"
    ));
    // A string_list in text has no encoding; in a JSON string literal neither.
    let l = || vec![slot("x", SlotType::StringList, true)];
    assert!(has(
        &cfg(ContentType::Text, "{{x}}", l()),
        &[],
        "slot_type_content_type"
    ));
    assert!(has(
        &cfg(ContentType::Json, r#"{"a":"{{x}}"}"#, l()),
        &[],
        "slot_type_content_type"
    ));
    assert!(has(
        &cfg(ContentType::Json, r#"{"a":"{{x}}"}"#, j()),
        &[],
        "slot_type_content_type"
    ));
    assert!(has(
        &cfg(ContentType::Form, "a=b{{x}}", l()),
        &[],
        "slot_type_content_type"
    ));
}

#[test]
fn optional_slot_at_the_root_is_refused() {
    let c = cfg(
        ContentType::Json,
        "{{x}}",
        vec![slot("x", SlotType::Json, false)],
    );
    assert!(has(&c, &[], "optional_slot_at_root"));
    let c = cfg(
        ContentType::Json,
        "{{x}}",
        vec![slot("x", SlotType::Json, true)],
    );
    assert!(validate(&c, &[]).is_ok());
}

#[test]
fn slot_limits_are_checked() {
    let with = |f: &dyn Fn(&mut Slot)| {
        let mut s = slot("x", SlotType::String, true);
        f(&mut s);
        cfg(ContentType::Json, r#"{"a":{{x}}}"#, vec![s])
    };
    assert!(has(&with(&|s| s.max_length = Some(0)), &[], "slot_limits"));
    assert!(has(
        &with(&|s| s.max_length = Some(65_537)),
        &[],
        "slot_limits"
    ));
    assert!(has(&with(&|s| s.max_items = Some(5)), &[], "slot_limits"));
    assert!(has(
        &with(&|s| s.description = "a".repeat(301)),
        &[],
        "slot_limits"
    ));
    assert!(has(
        &with(&|s| s.r#enum = Some(names(&["a"]))),
        &[],
        "slot_limits"
    ));
    // enum slot: needs 1..=50 unique non-empty values.
    let e = |v: Vec<String>| {
        let mut s = slot("x", SlotType::Enum, true);
        s.r#enum = Some(v);
        cfg(ContentType::Json, r#"{"a":{{x}}}"#, vec![s])
    };
    assert!(has(&e(vec![]), &[], "slot_limits"));
    assert!(has(&e(names(&["a", "a"])), &[], "slot_limits"));
    assert!(has(
        &e((0..51).map(|i| i.to_string()).collect()),
        &[],
        "slot_limits"
    ));
    assert!(validate(&e(names(&["a", "b"])), &[]).is_ok());
    assert!(has(
        &cfg(
            ContentType::Json,
            r#"{"a":{{x}}}"#,
            vec![slot("x", SlotType::Enum, true)]
        ),
        &[],
        "slot_limits"
    ));
    // Too many slots, too many secret values.
    let many: Vec<Slot> = (0..21)
        .map(|i| slot(&format!("s{i}"), SlotType::String, true))
        .collect();
    assert!(has(&cfg(ContentType::Json, "{}", many), &[], "slot_limits"));
    assert!(has(
        &cfg(ContentType::Json, "{}", vec![]),
        &["a", "b", "c", "d", "e"],
        "slot_limits"
    ));
}

#[test]
fn schema_subset_is_enforced() {
    let with = |sc: Value| {
        let mut s = slot("x", SlotType::Json, true);
        s.schema = Some(sc);
        cfg(ContentType::Json, r#"{"a":{{x}}}"#, vec![s])
    };
    assert!(validate(
        &with(json!({"type":"object","properties":{"a":{"type":"string","maxLength":5}},"required":["a"],"additionalProperties":false})),
        &[]
    )
    .is_ok());
    for bad in [
        json!({"$ref": "#/x"}),
        json!({"oneOf": []}),
        json!({"type": "nope"}),
        json!({"properties": {"a": {"pattern": "x"}}}),
        json!({"maxLength": -1}),
        json!({"required": [1]}),
        json!("string"),
    ] {
        assert!(has(&with(bad.clone()), &[], "schema_unsupported"), "{bad}");
    }
    // A schema on a non-json slot.
    let mut s = slot("x", SlotType::String, true);
    s.schema = Some(json!({"type": "string"}));
    assert!(has(
        &cfg(ContentType::Json, r#"{"a":{{x}}}"#, vec![s]),
        &[],
        "schema_unsupported"
    ));
    // Too deep.
    let mut deep = json!({"type": "string"});
    for _ in 0..12 {
        deep = json!({"items": deep});
    }
    assert!(has(&with(deep), &[], "schema_unsupported"));
}

#[test]
fn headers_are_checked() {
    let h = |n: &str, v: &str| {
        let mut c = cfg(ContentType::Json, r#"{"a":"{{secret:k}}"}"#, vec![]);
        c.headers.push(HeaderTemplate {
            name: n.into(),
            value: v.into(),
        });
        c
    };
    for denied in [
        "Host",
        "content-length",
        "Content-Type",
        "Transfer-Encoding",
        "Connection",
        "Cookie",
        "Forwarded",
        "X-Forwarded-For",
        "x-aurora-signature",
        "X-Aurora-Delivery",
        "Keep-Alive",
        "Upgrade",
        "TE",
        "Proxy-Authorization",
        "bad name",
        "",
        "a:b",
    ] {
        assert!(
            has(&h(denied, "x"), &["k"], "header_name_denied"),
            "{denied:?}"
        );
    }
    assert!(validate(&h("Authorization", "Bearer {{secret:k}}"), &names(&["k"])).is_ok());
    assert!(validate(&h("X-API-Key", "{{secret:k}}"), &names(&["k"])).is_ok());
    // A slot placeholder in a header.
    let mut c = h("X-A", "{{x}}");
    c.slots.push(slot("x", SlotType::String, true));
    assert!(has(&c, &["k"], "header_placeholder"));
    // Unknown secret in a header; control characters; too many; too long.
    assert!(has(
        &h("X-A", "{{secret:zz}}"),
        &["k"],
        "placeholder_unknown"
    ));
    assert!(has(&h("X-A", "a\r\nX-B: c"), &["k"], "template_syntax"));
    assert!(has(&h("X-A", &"a".repeat(3000)), &["k"], "too_large"));
    let mut c = h("X-A", "1");
    for i in 0..4 {
        c.headers.push(HeaderTemplate {
            name: format!("X-B{i}"),
            value: "1".into(),
        });
    }
    assert!(has(&c, &["k"], "too_large"));
    // Duplicate header names, any case.
    let mut c = h("X-A", "1");
    c.headers.push(HeaderTemplate {
        name: "x-a".into(),
        value: "2".into(),
    });
    assert!(has(&c, &["k"], "slot_duplicate"));
}

#[test]
fn sizes_and_rates_are_checked() {
    let big = format!(r#"{{"a":"{}"}}"#, "x".repeat(17 * 1024));
    assert!(has(&cfg(ContentType::Json, &big, vec![]), &[], "too_large"));
    let mut c = cfg(ContentType::Json, "{}", vec![]);
    c.description = "x".repeat(1001);
    assert!(has(&c, &[], "too_large"));
    let mut c = cfg(ContentType::Json, "{}", vec![]);
    c.rate.per_minute = Some(601);
    c.rate.per_day = Some(100_001);
    let k = codes(&c, &[]);
    assert!(k.contains(&("/rate/per_minute".into(), "rate_above_ceiling")));
    assert!(k.contains(&("/rate/per_day".into(), "rate_above_ceiling")));
    c.rate = Rate {
        per_minute: Some(600),
        per_day: Some(100_000),
    };
    assert!(validate(&c, &[]).is_ok());
    // The whole configuration is bounded as well.
    let mut c = cfg(ContentType::Json, "{}", vec![]);
    c.preset = Some("x".repeat(33 * 1024));
    assert!(has(&c, &[], "too_large"));
}

#[test]
fn config_json_is_strict() {
    let ok = json!({"version":1,"method":"POST","content_type":"application/json","template":"{}"});
    assert!(serde_json::from_value::<WebhookConfig>(ok.clone()).is_ok());
    let mut bad = ok.clone();
    bad["version"] = json!(2);
    assert!(serde_json::from_value::<WebhookConfig>(bad).is_err());
    let mut bad = ok.clone();
    bad["extra"] = json!(1);
    assert!(serde_json::from_value::<WebhookConfig>(bad).is_err());
    let mut bad = ok;
    bad["method"] = json!("GET");
    assert!(serde_json::from_value::<WebhookConfig>(bad).is_err());
}

// ---- values: each slot type accepted and refused per rule ------------------------------------

fn one(s: Slot) -> CompiledConfig {
    compiled(&cfg(ContentType::Json, r#"{"a":{{v}}}"#, vec![s]), &[])
}

fn rule(c: &CompiledConfig, v: Value) -> Option<Rule> {
    check_values(c, &json!({ "v": v })).err().map(|e| e.rule)
}

#[test]
fn string_slot() {
    let mut s = slot("v", SlotType::String, true);
    s.max_length = Some(3);
    let c = one(s);
    assert_eq!(rule(&c, json!("abc")), None);
    assert_eq!(rule(&c, json!("abcd")), Some(Rule::MaxLength));
    assert_eq!(rule(&c, json!(1)), Some(Rule::Type));
    // Counted in Unicode scalar values, not bytes or UTF-16 units.
    assert_eq!(rule(&c, json!("😀😀😀")), None);
    assert_eq!(rule(&c, json!("😀😀😀😀")), Some(Rule::MaxLength));
    // A null is a left-out value: a required slot is then missing.
    assert_eq!(rule(&c, json!(null)), Some(Rule::Missing));
}

#[test]
fn number_integer_boolean_slots() {
    let c = one(slot("v", SlotType::Integer, true));
    assert_eq!(rule(&c, json!(3)), None);
    assert_eq!(rule(&c, json!(-3)), None);
    assert_eq!(rule(&c, json!(3.5)), Some(Rule::Type));
    assert_eq!(rule(&c, json!(3.0)), Some(Rule::Type));
    assert_eq!(rule(&c, json!("3")), Some(Rule::Type));
    let c = one(slot("v", SlotType::Number, true));
    assert_eq!(rule(&c, json!(3.5)), None);
    assert_eq!(rule(&c, json!(3)), None);
    assert_eq!(rule(&c, json!("3")), Some(Rule::Type));
    let c = one(slot("v", SlotType::Boolean, true));
    assert_eq!(rule(&c, json!(true)), None);
    assert_eq!(rule(&c, json!(0)), Some(Rule::Type));
}

#[test]
fn string_list_slot() {
    let mut s = slot("v", SlotType::StringList, true);
    s.max_items = Some(2);
    s.max_length = Some(2);
    let c = one(s);
    assert_eq!(rule(&c, json!(["a", "bb"])), None);
    assert_eq!(rule(&c, json!([])), None);
    assert_eq!(rule(&c, json!(["a", "b", "c"])), Some(Rule::MaxItems));
    assert_eq!(rule(&c, json!(["abc"])), Some(Rule::MaxLength));
    assert_eq!(rule(&c, json!([1])), Some(Rule::Type));
    assert_eq!(rule(&c, json!("a")), Some(Rule::Type));
}

#[test]
fn enum_slot() {
    let mut s = slot("v", SlotType::Enum, true);
    s.r#enum = Some(names(&["low", "high"]));
    let c = one(s);
    assert_eq!(rule(&c, json!("low")), None);
    assert_eq!(rule(&c, json!("medium")), Some(Rule::Enum));
    assert_eq!(rule(&c, json!(1)), Some(Rule::Type));
}

#[test]
fn json_slot_with_and_without_a_schema() {
    let c = one(slot("v", SlotType::Json, true));
    for v in [
        json!(1),
        json!("a"),
        json!({"a": [1, {"b": null}]}),
        json!([]),
        json!(false),
    ] {
        assert_eq!(rule(&c, v), None);
    }
    let mut s = slot("v", SlotType::Json, true);
    s.schema = Some(json!({
        "type": "object",
        "properties": {
            "n": {"type": "integer", "minimum": 1, "maximum": 5},
            "t": {"type": "string", "maxLength": 3},
            "k": {"enum": ["a", "b"]},
            "l": {"type": "array", "items": {"type": "string"}}
        },
        "required": ["n"],
        "additionalProperties": false
    }));
    let c = one(s);
    assert_eq!(
        rule(&c, json!({"n": 2, "t": "abc", "k": "a", "l": ["x"]})),
        None
    );
    for bad in [
        json!({}),
        json!({"n": 0}),
        json!({"n": 6}),
        json!({"n": 1.5}),
        json!({"n": 1, "t": "abcd"}),
        json!({"n": 1, "k": "z"}),
        json!({"n": 1, "l": [1]}),
        json!({"n": 1, "extra": 1}),
        json!([1]),
        json!("x"),
    ] {
        assert_eq!(rule(&c, bad.clone()), Some(Rule::Schema), "{bad}");
    }
}

#[test]
fn missing_unknown_and_non_object_values() {
    let c = compiled(
        &cfg(
            ContentType::Json,
            r#"{"a":{{x}},"b":{{y}}}"#,
            vec![
                slot("x", SlotType::String, true),
                slot("y", SlotType::String, false),
            ],
        ),
        &[],
    );
    let e = check_values(&c, &json!({})).unwrap_err();
    assert_eq!((e.slot.as_str(), e.rule), ("x", Rule::Missing));
    let e = check_values(&c, &json!({"x": "1", "z": 1})).unwrap_err();
    assert_eq!((e.slot.as_str(), e.rule), ("z", Rule::Unknown));
    let e = check_values(&c, &json!(["x"])).unwrap_err();
    assert_eq!((e.slot.as_str(), e.rule), ("", Rule::Type));
    let e = check_values(&c, &json!({"x": null})).unwrap_err();
    assert_eq!(e.rule, Rule::Missing);
    // A null optional is left out.
    let ok = check_values(&c, &json!({"x": "1", "y": null})).unwrap();
    assert_eq!(ok.given(), vec!["x"]);
}

// ---- rendering --------------------------------------------------------------------------------

/// Strings that must come out literally and correctly escaped, whatever the place.
fn nasty() -> Vec<String> {
    [
        "{{secret:a}}",
        "{{x}}",
        "\"",
        "\\",
        "}",
        "]",
        "&",
        "=",
        "%",
        "%41",
        "a&b=c",
        "\r\n",
        "line1\nline2\r\n",
        "\0",
        "😀 𝄞 \u{10FFFF}",
        "\u{2028}\u{2029}",
        "\"},\"admin\":true,\"x\":\"",
        "</script>",
        "",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

#[test]
fn json_values_are_inserted_literally_in_and_out_of_string_literals() {
    let c = cfg(
        ContentType::Json,
        r#"{"fixed":"Aurora","whole":{{x}},"in":"pre {{x}} post","secret":"{{secret:a}}","s2":{{secret:a}}}"#,
        vec![slot("x", SlotType::String, true)],
    );
    for v in nasty() {
        let r = render_with(&c, json!({ "x": v }), &[("a", "SEC\"RET")]);
        let parsed: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(parsed["fixed"], "Aurora");
        assert_eq!(parsed["whole"], v, "{v:?}");
        assert_eq!(parsed["in"], format!("pre {v} post"), "{v:?}");
        // The secret is inserted, and the agent's `{{secret:a}}` is not substituted.
        assert_eq!(parsed["secret"], "SEC\"RET");
        assert_eq!(parsed["s2"], "SEC\"RET");
        assert_eq!(parsed.as_object().unwrap().len(), 5, "{v:?}");
        if v == "{{secret:a}}" {
            assert!(
                !String::from_utf8(r.body.clone())
                    .unwrap()
                    .contains("SEC\\\"RET\",\"s2\":\"SEC\\\"RET\"}\"{{")
            );
            assert_eq!(parsed["whole"], "{{secret:a}}");
        }
    }
}

#[test]
fn json_scalar_slots_in_and_out_of_strings() {
    let c = cfg(
        ContentType::Json,
        r#"{"i":{{i}},"n":{{n}},"b":{{b}},"e":{{e}},"l":{{l}},"j":{{j}},"s":"{{i}}-{{n}}-{{b}}-{{e}}"}"#,
        vec![
            slot("i", SlotType::Integer, true),
            slot("n", SlotType::Number, true),
            slot("b", SlotType::Boolean, true),
            {
                let mut s = slot("e", SlotType::Enum, true);
                s.r#enum = Some(names(&["a\"b"]));
                s
            },
            slot("l", SlotType::StringList, true),
            slot("j", SlotType::Json, true),
        ],
    );
    let r = render_with(
        &c,
        json!({"i": 42, "n": 1.5, "b": true, "e": "a\"b", "l": ["x", "}\"{{i}}"], "j": {"k": [1, null]}}),
        &[],
    );
    let p: Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(
        p,
        json!({"i": 42, "n": 1.5, "b": true, "e": "a\"b", "l": ["x", "}\"{{i}}"], "j": {"k": [1, null]}, "s": "42-1.5-true-a\"b"})
    );
    assert_eq!(r.content_type, "application/json");
}

#[test]
fn optional_slots_left_out_in_each_position() {
    let c = cfg(
        ContentType::Json,
        r#"{"a":1,"o1":{{o1}},"in":"x{{o2}}y","arr":[{{o3}},2,{{o4}}],"n":{"o5":{{o5}}},"last":{{o6}}}"#,
        vec![
            slot("o1", SlotType::String, false),
            slot("o2", SlotType::String, false),
            slot("o3", SlotType::Integer, false),
            slot("o4", SlotType::Integer, false),
            slot("o5", SlotType::Json, false),
            slot("o6", SlotType::StringList, false),
        ],
    );
    assert_eq!(body(&c, json!({})), r#"{"a":1,"in":"xy","arr":[2],"n":{}}"#);
    assert_eq!(
        body(
            &c,
            json!({"o1": "A", "o2": "B", "o3": 1, "o4": 3, "o5": [1], "o6": []})
        ),
        r#"{"a":1,"o1":"A","in":"xBy","arr":[1,2,3],"n":{"o5":[1]},"last":[]}"#
    );
    // Only the first member left: no stray comma.
    let c = cfg(
        ContentType::Json,
        r#"{"o":{{o}},"k":{{k}}}"#,
        vec![
            slot("o", SlotType::String, false),
            slot("k", SlotType::String, true),
        ],
    );
    assert_eq!(body(&c, json!({"k": "v"})), r#"{"k":"v"}"#);
}

#[test]
fn form_values_are_encoded_and_lists_repeat_the_key() {
    let c = cfg(
        ContentType::Form,
        "topic=alerts&msg={{msg}}&pre=a{{msg}}b&tag={{tags}}&opt={{opt}}&tok={{secret:k}}&n={{n}}",
        vec![
            slot("msg", SlotType::String, true),
            slot("tags", SlotType::StringList, false),
            slot("opt", SlotType::String, false),
            slot("n", SlotType::Integer, false),
        ],
    );
    for v in nasty() {
        let r = render_with(
            &c,
            json!({ "msg": v, "tags": ["a b", &v] }),
            &[("k", "s&=")],
        );
        assert_eq!(r.content_type, "application/x-www-form-urlencoded");
        let s = String::from_utf8(r.body).unwrap();
        // Parse back like a receiver: split on `&`, then `=`, then percent-decode.
        let pairs: Vec<(String, String)> = s
            .split('&')
            .map(|p| {
                let (k, val) = p.split_once('=').unwrap();
                (k.to_owned(), form_decode(val))
            })
            .collect();
        let get = |k: &str| -> Vec<String> {
            pairs
                .iter()
                .filter(|(a, _)| a == k)
                .map(|(_, b)| b.clone())
                .collect()
        };
        assert_eq!(get("topic"), vec!["alerts"]);
        assert_eq!(get("msg"), vec![v.clone()], "{v:?}");
        assert_eq!(get("pre"), vec![format!("a{v}b")]);
        assert_eq!(get("tag"), vec!["a b".to_owned(), v.clone()]);
        assert_eq!(get("tok"), vec!["s&="]);
        // The left-out optionals drop their pair.
        assert!(get("opt").is_empty() && get("n").is_empty());
        assert_eq!(pairs.len(), 3 + 2 + 1, "{s}");
    }
}

fn form_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' => {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out).unwrap()
}

#[test]
fn text_values_are_inserted_as_utf8() {
    let c = cfg(
        ContentType::Text,
        "{{a}} | {{b}} | {{n}} | {{opt}}|{{secret:k}}",
        vec![
            slot("a", SlotType::String, true),
            slot("b", SlotType::Boolean, true),
            slot("n", SlotType::Number, true),
            slot("opt", SlotType::String, false),
        ],
    );
    for v in nasty() {
        let r = render_with(&c, json!({"a": v, "b": false, "n": 2.5}), &[("k", "K")]);
        assert_eq!(r.content_type, "text/plain");
        assert_eq!(
            String::from_utf8(r.body).unwrap(),
            format!("{v} | false | 2.5 | |K")
        );
    }
}

#[test]
fn a_value_is_never_read_as_a_template() {
    // One pass: a value that looks like a placeholder, in every content type.
    for (ct, t) in [
        (ContentType::Json, r#"{"a":{{x}},"b":"{{x}}"}"#),
        (ContentType::Form, "a={{x}}"),
        (ContentType::Text, "{{x}}"),
    ] {
        let mut c = cfg(ct, t, vec![slot("x", SlotType::String, true)]);
        // The secret exists (used by a header); only the agent's text names it in the body.
        c.headers.push(HeaderTemplate {
            name: "X-Key".into(),
            value: "{{secret:k}}".into(),
        });
        let r = render_with(&c, json!({"x": "{{secret:k}}"}), &[("k", "TOPSECRET")]);
        let s = String::from_utf8(r.body).unwrap();
        assert!(!s.contains("TOPSECRET"), "{ct:?}: {s}");
        assert_eq!(r.headers[0].1, "TOPSECRET");
    }
}

#[test]
fn headers_carry_secrets_and_refuse_control_characters() {
    let mut c = cfg(ContentType::Json, r#"{"a":1}"#, vec![]);
    c.headers.push(HeaderTemplate {
        name: "Authorization".into(),
        value: "Bearer {{secret:t}}".into(),
    });
    let r = render_with(&c, json!({}), &[("t", "abc")]);
    assert_eq!(
        r.headers,
        vec![("Authorization".to_owned(), "Bearer abc".to_owned())]
    );
    let cc = compiled(&c, &["t"]);
    let ck = check_values(&cc, &json!({})).unwrap();
    let bad = SecretValues([("t".to_owned(), "a\r\nX-Evil: 1".to_owned())].into());
    assert_eq!(
        render(&cc, &ck, &bad),
        Err(RenderError::HeaderValue("Authorization".into()))
    );
    let missing = SecretValues::default();
    assert_eq!(
        render(&cc, &ck, &missing),
        Err(RenderError::MissingSecret("t".into()))
    );
}

#[test]
fn masked_render_shows_no_secret() {
    let mut c = cfg(
        ContentType::Json,
        r#"{"topic":"{{secret:topic}}","t2":{{secret:topic}},"m":{{m}}}"#,
        vec![slot("m", SlotType::String, true)],
    );
    c.headers.push(HeaderTemplate {
        name: "X-API-Key".into(),
        value: "{{secret:topic}}".into(),
    });
    let cc = compiled(&c, &["topic"]);
    let ck = check_values(&cc, &json!({"m": "hi"})).unwrap();
    let r = render_masked(&cc, &ck).unwrap();
    let p: Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(
        p,
        json!({"topic": "«secret:topic»", "t2": "«secret:topic»", "m": "hi"})
    );
    assert_eq!(r.headers[0].1, "«secret:topic»");
    // The real one differs.
    let real = render(
        &cc,
        &ck,
        &SecretValues([("topic".to_owned(), "T0P".to_owned())].into()),
    )
    .unwrap();
    assert!(String::from_utf8(real.body).unwrap().contains("T0P"));
}

#[test]
fn the_rendered_body_is_bounded() {
    let c = cfg(
        ContentType::Json,
        r#"{"a":{{l}}}"#,
        vec![{
            let mut s = slot("l", SlotType::StringList, true);
            s.max_length = Some(65_536);
            s
        }],
    );
    let cc = compiled(&c, &[]);
    let big = "x".repeat(65_536);
    let ck = check_values(&cc, &json!({ "l": [&big, &big, &big, &big, &big] })).unwrap();
    assert_eq!(render_masked(&cc, &ck), Err(RenderError::TooLarge));
}

#[test]
fn secret_values_and_checked_values_do_not_print() {
    let s = SecretValues([("t".to_owned(), "TOPSECRET".to_owned())].into());
    assert!(!format!("{s:?}").contains("TOPSECRET"));
    let c = compiled(
        &cfg(
            ContentType::Json,
            r#"{"a":{{x}}}"#,
            vec![slot("x", SlotType::String, true)],
        ),
        &[],
    );
    let ck = check_values(&c, &json!({"x": "PRIVATE"})).unwrap();
    assert!(!format!("{ck:?}").contains("PRIVATE"));
}

// ---- signing --------------------------------------------------------------------------------

#[test]
fn signature_vectors() {
    // The vectors the setup guide's Python and Node snippets reproduce
    // (`printf '%s' '<t>.<body>' | openssl dgst -sha256 -hmac '<secret>'`).
    assert_eq!(
        sign(b"whsec_test", 1_700_000_000, br#"{"text":"hello"}"#),
        "t=1700000000,v1=4de4482f0d79eb97df8f0e9ece4238de4e613a5ad29c846b70bc7dd6e1389e80"
    );
    // An empty body, and a key that is not text.
    assert_eq!(
        sign(b"k", 1_700_000_000, b""),
        "t=1700000000,v1=5c2999a43333a6877f49c8003f235c4f6de40f85b1b7414b70bd470a8db53d97"
    );
}

// ---- property: random strings in every slot of a JSON template ------------------------------

fn any_text() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<String>(),
        proptest::collection::vec(
            prop_oneof![
                Just("\"".to_owned()),
                Just("\\".to_owned()),
                Just("{{secret:a}}".to_owned()),
                Just("{{x}}".to_owned()),
                Just("\r\n".to_owned()),
                Just("\0".to_owned()),
                Just("😀".to_owned()),
                Just("}]".to_owned()),
                "[ -~]{0,6}",
            ],
            0..8
        )
        .prop_map(|v| v.concat()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn random_strings_always_render_valid_json_and_round_trip(
        x in any_text(), y in any_text(), z in proptest::collection::vec(any_text(), 0..4), j in any_text(), sec in any_text()
    ) {
        let c = cfg(
            ContentType::Json,
            r#"{"w":{{x}},"s":"<{{y}}>","l":{{z}},"j":{{j}},"k":"{{secret:a}}","m":{"o":{{opt}}}}"#,
            vec![
                slot("x", SlotType::String, true),
                slot("y", SlotType::String, true),
                slot("z", SlotType::StringList, true),
                slot("j", SlotType::Json, true),
                slot("opt", SlotType::String, false),
            ],
        );
        // Control characters in a secret are fine in a body (only headers refuse them).
        let r = render_with(&c, json!({"x": x, "y": y, "z": z, "j": {"v": j}}), &[("a", &sec)]);
        let p: Value = serde_json::from_slice(&r.body).unwrap();
        prop_assert_eq!(&p["w"], &json!(x));
        prop_assert_eq!(p["s"].as_str().unwrap(), format!("<{y}>"));
        prop_assert_eq!(&p["l"], &json!(z));
        prop_assert_eq!(&p["j"], &json!({"v": j}));
        prop_assert_eq!(p["k"].as_str().unwrap(), sec.as_str());
        prop_assert_eq!(p["m"].as_object().unwrap().len(), 0);
        prop_assert_eq!(p.as_object().unwrap().len(), 6);
    }

    #[test]
    fn random_strings_round_trip_in_form(x in any_text(), l in proptest::collection::vec(any_text(), 0..4)) {
        let c = cfg(
            ContentType::Form,
            "a={{x}}&l={{l}}",
            vec![slot("x", SlotType::String, true), slot("l", SlotType::StringList, true)],
        );
        let r = render_with(&c, json!({"x": x, "l": l}), &[]);
        let s = String::from_utf8(r.body).unwrap();
        let mut got_l = Vec::new();
        let mut got_a = None;
        for p in s.split('&') {
            let (k, v) = p.split_once('=').unwrap();
            if k == "a" { got_a = Some(form_decode(v)); } else { got_l.push(form_decode(v)); }
        }
        prop_assert_eq!(got_a, Some(x));
        prop_assert_eq!(got_l, l);
    }
}
