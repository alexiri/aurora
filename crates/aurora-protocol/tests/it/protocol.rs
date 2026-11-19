//! Version header and open-vocabulary behaviour.

use std::error::Error;

use aurora_protocol::schema;
use aurora_protocol::version::{HEADER, Plane, ProtocolVersion, VersionError, accepts_range};
use aurora_protocol::vocab::{BoundedString, ExampleKind, OPEN_VOCAB_MAX};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn header_name_is_lowercase() {
    assert_eq!(HEADER, "x-aurora-protocol");
}

#[test]
fn header_round_trip_for_every_plane() -> TestResult {
    for plane in [
        Plane::Runner,
        Plane::Supervisor,
        Plane::Connector,
        Plane::Hookhost,
    ] {
        let text = format!("{}/{}", plane.as_str(), plane.current());
        let parsed = ProtocolVersion::parse(&text)?;
        assert_eq!(
            parsed,
            ProtocolVersion {
                plane,
                n: plane.current()
            }
        );
        assert_eq!(parsed.to_string(), text);
        parsed.check()?;
    }
    Ok(())
}

#[test]
fn n_minus_one_is_accepted_and_window_is_exact() {
    assert!(accepts_range(2, 2));
    assert!(accepts_range(2, 1), "N-1 must be accepted");
    assert!(!accepts_range(2, 0));
    assert!(!accepts_range(2, 3));
    assert!(!accepts_range(1, 0));
}

#[test]
fn unknown_future_version_is_rejected_with_range() -> TestResult {
    let future = ProtocolVersion::parse("runner/2")?;
    match future.check() {
        Err(VersionError::Unsupported { plane, n, min, max }) => {
            assert_eq!((plane, n, min, max), (Plane::Runner, 2, 1, 1));
        }
        other => return Err(format!("expected Unsupported, got {other:?}").into()),
    }
    Ok(())
}

#[test]
fn malformed_and_unknown_values_are_rejected() {
    assert_eq!(
        ProtocolVersion::parse("runner"),
        Err(VersionError::Malformed)
    );
    assert_eq!(
        ProtocolVersion::parse("runner/"),
        Err(VersionError::Malformed)
    );
    assert_eq!(
        ProtocolVersion::parse("runner/01"),
        Err(VersionError::Malformed)
    );
    assert_eq!(
        ProtocolVersion::parse("runner/-1"),
        Err(VersionError::Malformed)
    );
    assert_eq!(
        ProtocolVersion::parse("bogus/1"),
        Err(VersionError::UnknownPlane)
    );
    // Overflow is well-formed but unsupported.
    let huge = ProtocolVersion::parse("runner/99999999999999").ok();
    assert!(matches!(
        huge.map(|v| v.check()),
        Some(Err(VersionError::Unsupported { .. }))
    ));
}

#[test]
fn version_serde_is_plain_string() -> TestResult {
    let v = ProtocolVersion::parse("connector/1")?;
    assert_eq!(serde_json::to_string(&v)?, "\"connector/1\"");
    let back: ProtocolVersion = serde_json::from_str("\"connector/1\"")?;
    assert_eq!(back, v);
    assert!(serde_json::from_str::<ProtocolVersion>("\"connector/9\"").is_ok());
    Ok(())
}

#[test]
fn open_vocab_known_value_maps_to_variant() -> TestResult {
    let v: ExampleKind = serde_json::from_str("\"alpha\"")?;
    assert_eq!(v, ExampleKind::Alpha);
    assert_eq!(serde_json::to_string(&v)?, "\"alpha\"");
    Ok(())
}

#[test]
fn open_vocab_unknown_value_becomes_other_and_round_trips() -> TestResult {
    let v: ExampleKind = serde_json::from_str("\"zeta\"")?;
    assert!(matches!(&v, ExampleKind::Other(s) if s.as_str() == "zeta"));
    assert_eq!(serde_json::to_string(&v)?, "\"zeta\"");
    Ok(())
}

#[test]
fn open_vocab_over_long_unknown_is_rejected() -> TestResult {
    let at_limit = "a".repeat(OPEN_VOCAB_MAX);
    let ok: ExampleKind = serde_json::from_str(&format!("\"{at_limit}\""))?;
    assert!(matches!(ok, ExampleKind::Other(_)));
    let too_long = "a".repeat(OPEN_VOCAB_MAX + 1);
    assert!(serde_json::from_str::<ExampleKind>(&format!("\"{too_long}\"")).is_err());
    Ok(())
}

#[test]
fn bounded_string_enforces_limit() {
    assert!(BoundedString::<3>::new("abc").is_ok());
    assert!(BoundedString::<3>::new("abcd").is_err());
}

/// (P5-GUIDE-7, C-654) Rolling upgrade, both ways: a descriptor from an older API (no `family`,
/// no `exposure`) parses and means on-demand; one with neither field set serializes exactly as
/// before; an older peer's descriptor shape (no such fields) reads the new one, extra keys ignored.
#[test]
fn descriptor_family_and_exposure_are_optional_both_ways() -> TestResult {
    use aurora_protocol::tools::{Exposure, ToolDescriptor};
    let old = r#"{"name":"memory_save","description":"d","input_schema":{"type":"object"}}"#;
    let d: ToolDescriptor = serde_json::from_str(old)?;
    assert_eq!(d.family, None);
    assert_eq!(d.exposure.unwrap_or_default(), Exposure::OnDemand);
    assert_eq!(serde_json::to_string(&d)?, old, "nothing new is emitted");

    let new = ToolDescriptor {
        family: Some("memory".into()),
        exposure: Some(Exposure::Core),
        ..d
    };
    let v = serde_json::to_value(&new)?;
    assert_eq!(v["family"], "memory");
    assert_eq!(v["exposure"], "core");
    assert_eq!(
        serde_json::to_value(Exposure::OnDemand)?,
        serde_json::json!("on_demand")
    );

    // What an older supervisor does: its descriptor type has neither field.
    #[derive(serde::Deserialize)]
    struct OldDescriptor {
        name: String,
        input_schema: serde_json::Value,
    }
    let o: OldDescriptor = serde_json::from_value(v)?;
    assert_eq!(o.name, "memory_save");
    assert_eq!(o.input_schema["type"], "object");
    Ok(())
}

#[test]
fn registry_names_are_unique() {
    let all = schema::all();
    let mut names: Vec<&str> = all.iter().map(|(n, _)| *n).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all.len());
}
