//! Classification corpus: every `tests/corpus/*.yaml` file is a list of
//! `{ sql, class | refused }` cases (`refused` is `forbidden` or `unparsable`).

use aurora_sqlclass::{Class, Refusal, classify};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    sql: String,
    class: Option<String>,
    refused: Option<String>,
    statements: Option<usize>,
}

#[test]
fn corpus() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("corpus dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
        .collect();
    files.sort();
    assert!(files.len() >= 7, "corpus files missing");
    let mut failures = Vec::new();
    let mut total = 0;
    for f in files {
        let cases: Vec<Case> =
            serde_yaml::from_str(&std::fs::read_to_string(&f).expect("read")).expect("yaml");
        for c in cases {
            total += 1;
            let got = classify(&c.sql);
            let ok = match (&c.class, &c.refused, &got) {
                (Some(want), None, Ok(g)) => {
                    Class::parse(want) == Some(g.class)
                        && c.statements.is_none_or(|n| n == g.statements)
                }
                (None, Some(r), Err(e)) => matches!(
                    (r.as_str(), e),
                    ("forbidden", Refusal::Forbidden(_))
                        | ("unparsable", Refusal::Unparsable(_))
                        | ("too_large", Refusal::TooLarge)
                ),
                _ => false,
            };
            if !ok {
                failures.push(format!(
                    "{}: {:?} -> expected {:?}{:?}, got {:?}",
                    f.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                    c.sql,
                    c.class,
                    c.refused,
                    got
                ));
            }
        }
    }
    assert!(total > 100, "corpus too small: {total}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
