//! The restricted JSON path of `09 §3.4 library` as `aurora-agent` needs it: `$`, then any number
//! of `.name`, `['name']`, `["name"]` and `[index]` steps. Anchored, no recursive descent, no
//! filters, no wildcards (those are refused by the linter at ingest; here they simply do not
//! evaluate).
use serde_json::Value;

enum Step {
    Key(String),
    Index(usize),
}

fn parse(path: &str) -> Option<Vec<Step>> {
    let rest = path.strip_prefix('$')?;
    let b: Vec<char> = rest.chars().collect();
    let mut i = 0;
    let mut steps = Vec::new();
    while i < b.len() {
        match b[i] {
            '.' => {
                i += 1;
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], '_' | '-')) {
                    i += 1;
                }
                if start == i {
                    return None;
                }
                steps.push(Step::Key(b[start..i].iter().collect()));
            }
            '[' => {
                i += 1;
                if i < b.len() && (b[i] == '\'' || b[i] == '"') {
                    let q = b[i];
                    i += 1;
                    let start = i;
                    while i < b.len() && b[i] != q {
                        i += 1;
                    }
                    if i >= b.len() {
                        return None;
                    }
                    steps.push(Step::Key(b[start..i].iter().collect()));
                    i += 1;
                } else {
                    let start = i;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                    let n: usize = b[start..i].iter().collect::<String>().parse().ok()?;
                    steps.push(Step::Index(n));
                }
                if b.get(i) != Some(&']') {
                    return None;
                }
                i += 1;
            }
            _ => return None,
        }
    }
    Some(steps)
}

/// The plain `.name` keys of `path`, or `None` when the path has any other step (`[index]`, a
/// bracketed name the linter would have refused).
#[must_use]
pub fn keys(path: &str) -> Option<Vec<String>> {
    parse(path)?
        .into_iter()
        .map(|s| match s {
            Step::Key(k) => Some(k),
            Step::Index(_) => None,
        })
        .collect()
}

/// The value at `path`, or `None` when the path is not in the grammar or does not exist.
#[must_use]
pub fn get<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for s in parse(path)? {
        cur = match s {
            Step::Key(k) => cur.get(k)?,
            Step::Index(n) => cur.get(n)?,
        };
    }
    Some(cur)
}

/// The value at `path` mutably, or `None` when the path is not in the grammar or does not exist.
#[must_use]
pub fn get_mut<'a>(v: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    let mut cur = v;
    for s in parse(path)? {
        cur = match s {
            Step::Key(k) => cur.get_mut(k.as_str())?,
            Step::Index(n) => cur.get_mut(n)?,
        };
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn evaluates_the_restricted_grammar() {
        let v = json!({"data": [{"id": 1}], "meta": {"next-page": "abc"}});
        assert_eq!(get(&v, "$"), Some(&v));
        assert_eq!(get(&v, "$.data[0].id"), Some(&json!(1)));
        assert_eq!(get(&v, "$['meta']['next-page']"), Some(&json!("abc")));
        assert_eq!(get(&v, "$.nope"), None);
        assert_eq!(get(&v, "$..id"), None);
        assert_eq!(get(&v, "$.data[*]"), None);
        assert_eq!(get(&v, "data"), None);
    }

    #[test]
    fn keys_are_the_plain_name_steps() {
        assert_eq!(keys("$.a.b"), Some(vec!["a".into(), "b".into()]));
        assert_eq!(keys("$.a[0]"), None);
        assert_eq!(keys("$"), Some(vec![]));
        assert_eq!(keys("$..a"), None);
    }

    #[test]
    fn get_mut_writes_through_the_path() {
        let mut v = json!({"data": [{"id": 1}]});
        *get_mut(&mut v, "$.data[0].id").unwrap() = json!(2);
        assert_eq!(get(&v, "$.data[0].id"), Some(&json!(2)));
        assert!(get_mut(&mut v, "$.nope").is_none());
    }
}
