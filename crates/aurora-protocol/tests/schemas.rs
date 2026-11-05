//! Schema drift check. Run `UPDATE_SCHEMAS=1 cargo test -p aurora-protocol` to regenerate.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

use schemars::Schema;

type TestResult = Result<(), Box<dyn Error>>;

fn schemas_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas")
}

/// Sorts object keys recursively so output is deterministic.
fn canonical(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<String, serde_json::Value> =
                map.into_iter().map(|(k, v)| (k, canonical(v))).collect();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonical).collect())
        }
        other => other,
    }
}

fn render(schema: &Schema) -> Result<String, Box<dyn Error>> {
    let value = canonical(serde_json::to_value(schema)?);
    let mut out = serde_json::to_string_pretty(&value)?;
    out.push('\n');
    Ok(out)
}

#[test]
fn checked_in_schemas_match_generated() -> TestResult {
    let dir = schemas_dir();
    let update = std::env::var_os("UPDATE_SCHEMAS").is_some();
    let registered = aurora_protocol::schema::all();

    if update {
        fs::create_dir_all(&dir)?;
    }

    for (name, schema) in &registered {
        let rendered = render(schema)?;
        let path = dir.join(format!("{name}.json"));
        if update {
            fs::write(&path, &rendered)?;
            continue;
        }
        let on_disk = fs::read_to_string(&path).map_err(|e| {
            format!(
                "{}: {e} (run UPDATE_SCHEMAS=1 cargo test -p aurora-protocol)",
                path.display()
            )
        })?;
        if on_disk != rendered {
            return Err(format!(
                "{} is out of date; run UPDATE_SCHEMAS=1 cargo test -p aurora-protocol",
                path.display()
            )
            .into());
        }
    }

    if !update {
        let known: Vec<String> = registered
            .iter()
            .map(|(n, _)| format!("{n}.json"))
            .collect();
        for entry in fs::read_dir(&dir)? {
            let file = entry?.file_name().to_string_lossy().into_owned();
            if file.ends_with(".json") && !known.contains(&file) {
                return Err(format!("stale schema file {file} is not in the registry").into());
            }
        }
    }
    Ok(())
}
