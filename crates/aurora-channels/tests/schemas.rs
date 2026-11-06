//! Drift check. Regenerate with `UPDATE_SCHEMAS=1 cargo test -p aurora-channels`.
use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;

fn canonical(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => serde_json::Value::Object(
            m.into_iter()
                .map(|(k, v)| (k, canonical(v)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.into_iter().map(canonical).collect())
        }
        o => o,
    }
}

#[test]
fn checked_in_schemas_match_generated() -> Result<(), Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let update = std::env::var_os("UPDATE_SCHEMAS").is_some();
    for (name, schema) in aurora_channels::schema::all() {
        let mut out = serde_json::to_string_pretty(&canonical(serde_json::to_value(&schema)?))?;
        out.push('\n');
        let path = dir.join(format!("{name}.json"));
        if update {
            std::fs::write(&path, &out)?;
        } else {
            let disk = std::fs::read_to_string(&path)
                .map_err(|e| format!("{}: {e} (UPDATE_SCHEMAS=1)", path.display()))?;
            if disk != out {
                return Err(format!("{} out of date (UPDATE_SCHEMAS=1)", path.display()).into());
            }
        }
    }
    Ok(())
}
