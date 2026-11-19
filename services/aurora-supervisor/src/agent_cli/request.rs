//! (P5-GUIDE-6, `docs/24 §10a` *App targets*, SEC-118) Building one request from an operation and
//! the agent's arguments: coercion to the declared types, validation before anything is sent, and
//! path building where a value is always **one percent-encoded segment** (it can never add a
//! segment, `..`, `?` or `#`). Everything here is help, never enforcement: the gateway decides.
use serde_json::{Map, Value};

use super::api::{Body, Operation, Param};
use super::text::pct;

/// One `name=value` or `name:=<json>` argument.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Plain(String, String),
    Raw(String, Value),
}

impl Arg {
    fn name(&self) -> &str {
        match self {
            Self::Plain(n, _) | Self::Raw(n, _) => n,
        }
    }
}

pub fn parse_args(items: &[String]) -> Result<Vec<Arg>, String> {
    items
        .iter()
        .map(|s| {
            let Some(eq) = s.find('=') else {
                return Err(format!(
                    "argument {s:?}: expected name=value or name:=<json>"
                ));
            };
            let (head, value) = (&s[..eq], &s[eq + 1..]);
            if let Some(name) = head.strip_suffix(':') {
                if name.is_empty() {
                    return Err(format!("argument {s:?}: empty name"));
                }
                let v: Value = serde_json::from_str(value)
                    .map_err(|e| format!("argument {name}:=…: not valid JSON: {e}"))?;
                Ok(Arg::Raw(name.to_owned(), v))
            } else if head.is_empty() {
                Err(format!("argument {s:?}: empty name"))
            } else {
                Ok(Arg::Plain(head.to_owned(), value.to_owned()))
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltBody {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub method: String,
    /// Starts with `/`, every segment already encoded.
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Option<BuiltBody>,
}

/// Headers an operation may never set, whatever its file says (defence in depth: the linter
/// refuses them at ingest, SEC-118).
fn header_forbidden(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
    ) || n.starts_with("x-aurora-")
        || n.starts_with("proxy-")
}

fn scalar_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn coerce_one(name: &str, ty: &str, raw: &str) -> Result<Value, String> {
    match ty {
        "integer" => raw
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| format!("{name}: {raw:?} is not an integer")),
        "number" => raw
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .and_then(|f| {
                if raw.contains(['.', 'e', 'E']) {
                    serde_json::Number::from_f64(f).map(Value::Number)
                } else {
                    raw.parse::<i64>().ok().map(Value::from)
                }
            })
            .ok_or_else(|| format!("{name}: {raw:?} is not a number")),
        "boolean" => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(format!("{name}: {raw:?} is not true or false")),
        },
        _ => Ok(Value::String(raw.to_owned())),
    }
}

/// The value of one declared parameter or body field from the arguments: `None` when absent and
/// without a default. Arrays take repeated names; every other type takes exactly one.
fn value_for(p: &Param, args: &[Arg], allow_raw: bool) -> Result<Option<Value>, String> {
    let mut plain: Vec<&str> = Vec::new();
    let mut raw: Option<&Value> = None;
    for a in args.iter().filter(|a| a.name() == p.name) {
        match a {
            Arg::Plain(_, v) => plain.push(v),
            Arg::Raw(_, v) => {
                if !allow_raw {
                    return Err(format!(
                        "{}: name:=<json> is for body fields; use {}=<value>",
                        p.name, p.name
                    ));
                }
                if raw.replace(v).is_some() {
                    return Err(format!("{} given twice", p.name));
                }
            }
        }
    }
    if raw.is_some() && !plain.is_empty() {
        return Err(format!("{} given twice", p.name));
    }
    let value = if let Some(v) = raw {
        Some(v.clone())
    } else if p.ty == "array" {
        if plain.is_empty() {
            None
        } else {
            Some(Value::Array(
                plain
                    .iter()
                    .map(|s| Value::String((*s).to_owned()))
                    .collect(),
            ))
        }
    } else {
        match plain.as_slice() {
            [] => None,
            [one] => Some(coerce_one(&p.name, &p.ty, one)?),
            _ => return Err(format!("{} given more than once", p.name)),
        }
    };
    let value = match value {
        None if p.is_required() => p.default.clone(),
        other => other,
    };
    if let (Some(v), Some(allowed)) = (&value, &p.values) {
        let items: Vec<&Value> = match v {
            Value::Array(a) => a.iter().collect(),
            one => vec![one],
        };
        for item in items {
            if !allowed
                .iter()
                .any(|a| scalar_string(a) == scalar_string(item))
            {
                let list: Vec<String> = allowed.iter().map(scalar_string).collect();
                return Err(format!(
                    "{}: {} is not one of {}",
                    p.name,
                    scalar_string(item),
                    list.join(", ")
                ));
            }
        }
    }
    if value.is_none() && p.is_required() {
        return Err(format!("missing required {} ({})", p.name, p.ty));
    }
    Ok(value)
}

fn as_pairs(name: &str, v: &Value) -> Result<Vec<(String, String)>, String> {
    match v {
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::Array(_) | Value::Object(_) => Err(format!(
                    "{name}: nested values cannot go in a query or form"
                )),
                s => Ok((name.to_owned(), scalar_string(s))),
            })
            .collect(),
        Value::Object(_) => Err(format!("{name}: an object cannot go in a query or form")),
        Value::Null => Ok(Vec::new()),
        s => Ok(vec![(name.to_owned(), scalar_string(s))]),
    }
}

fn no_unknown(declared: &[&str], args: &[Arg], what: &str) -> Result<(), String> {
    for a in args {
        if !declared.contains(&a.name()) {
            let known = if declared.is_empty() {
                "none".to_owned()
            } else {
                declared.join(", ")
            };
            return Err(format!("unknown {what} {:?} (known: {known})", a.name()));
        }
    }
    Ok(())
}

/// A path template with every `{name}` segment replaced by the percent-encoded value.
fn fill_path(template: &str, params: &[(&str, String)]) -> Result<String, String> {
    let mut out = String::new();
    for seg in template.split('/').skip(1) {
        out.push('/');
        if let Some(name) = seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            let Some((_, v)) = params.iter().find(|(n, _)| *n == name) else {
                return Err(format!("missing path parameter {name}"));
            };
            if v.is_empty() || v == "." || v == ".." {
                return Err(format!("{name}: {v:?} is not a usable path segment"));
            }
            out.push_str(&pct(v));
        } else {
            out.push_str(seg);
        }
    }
    Ok(if out.is_empty() { "/".to_owned() } else { out })
}

fn encode_body(b: &Body, args: &[Arg]) -> Result<Option<BuiltBody>, String> {
    let mut fields = Map::new();
    for f in &b.fields {
        if let Some(v) = value_for(f, args, true)? {
            fields.insert(f.name.clone(), v);
        }
    }
    if fields.is_empty() {
        return Ok(None);
    }
    let ct = b.content_type.as_str();
    match ct {
        "application/json" => Ok(Some(BuiltBody {
            content_type: ct.to_owned(),
            bytes: serde_json::to_vec(&Value::Object(fields)).map_err(|e| e.to_string())?,
        })),
        "application/x-www-form-urlencoded" => {
            let mut pairs = Vec::new();
            for (k, v) in &fields {
                pairs.extend(as_pairs(k, v)?);
            }
            Ok(Some(BuiltBody {
                content_type: ct.to_owned(),
                bytes: pairs
                    .iter()
                    .map(|(k, v)| format!("{}={}", pct(k), pct(v)))
                    .collect::<Vec<_>>()
                    .join("&")
                    .into_bytes(),
            }))
        }
        other => Err(format!(
            "{other} bodies are not built by aurora-agent; use --body @file with a prepared body"
        )),
    }
}

/// Builds the request of a package operation. `whole_body` (from `--body`) replaces the field
/// arguments: then `name=value` covers parameters only.
pub fn build_operation(
    op: &Operation,
    args: &[Arg],
    whole_body: Option<Vec<u8>>,
) -> Result<Built, String> {
    let mut declared: Vec<&str> = op.params.iter().map(|p| p.name.as_str()).collect();
    if whole_body.is_none()
        && let Some(b) = &op.body
    {
        declared.extend(b.fields.iter().map(|f| f.name.as_str()));
    }
    no_unknown(&declared, args, "argument")?;
    let mut path_values: Vec<(&str, String)> = Vec::new();
    let mut query = Vec::new();
    let mut headers = Vec::new();
    for p in &op.params {
        let Some(v) = value_for(p, args, false)? else {
            continue;
        };
        match p.location.as_deref() {
            Some("path") => path_values.push((p.name.as_str(), scalar_string(&v))),
            Some("query") => query.extend(as_pairs(&p.name, &v)?),
            Some("header") => {
                if header_forbidden(&p.name) {
                    return Err(format!("header {} cannot be set from here", p.name));
                }
                headers.push((p.name.clone(), scalar_string(&v)));
            }
            other => return Err(format!("{}: unknown parameter location {other:?}", p.name)),
        }
    }
    let path = fill_path(&op.path, &path_values)?;
    let body = match whole_body {
        Some(bytes) => Some(BuiltBody {
            content_type: op
                .body
                .as_ref()
                .map_or("application/json", |b| b.content_type.as_str())
                .to_owned(),
            bytes,
        }),
        None => match &op.body {
            Some(b) => encode_body(b, args)?,
            None => None,
        },
    };
    Ok(Built {
        method: op.method.to_ascii_uppercase(),
        path,
        query,
        headers,
        body,
    })
}

/// `--path` is sent to the account's host only: a path, never a URL, never a query string.
pub fn validate_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/') || path.starts_with("//") {
        return Err("--path must start with a single /".into());
    }
    let b = path.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let ok = c.is_ascii_alphanumeric()
            || matches!(
                c,
                b'/' | b'-'
                    | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
            );
        if c == b'%' {
            if i + 2 >= b.len() || !(b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit()) {
                return Err("--path: % must be followed by two hex digits".into());
            }
            i += 3;
            continue;
        }
        if !ok {
            return Err(format!(
                "--path: character {:?} is not allowed (no query string, fragment, space or backslash; percent-encode it)",
                char::from(c)
            ));
        }
        i += 1;
    }
    for seg in path.split('/').skip(1) {
        let l = seg.to_ascii_lowercase();
        if matches!(l.as_str(), "." | ".." | "%2e" | "%2e%2e" | ".%2e" | "%2e.") {
            return Err("--path: a . or .. segment is not allowed".into());
        }
    }
    Ok(())
}

/// A request to an arbitrary path (a catalogue entry or anything not listed): `name=value` are
/// query parameters, `name:=<json>` are JSON body fields, `--body` is the whole body.
pub fn build_adhoc(
    path: &str,
    method: &str,
    args: &[Arg],
    whole_body: Option<Vec<u8>>,
) -> Result<Built, String> {
    validate_path(path)?;
    let mut query = Vec::new();
    let mut fields = Map::new();
    for a in args {
        match a {
            Arg::Plain(n, v) => query.push((n.clone(), v.clone())),
            Arg::Raw(n, v) => {
                fields.insert(n.clone(), v.clone());
            }
        }
    }
    if whole_body.is_some() && !fields.is_empty() {
        return Err("name:=<json> cannot be combined with --body".into());
    }
    let body = match whole_body {
        Some(bytes) => Some(BuiltBody {
            content_type: "application/json".into(),
            bytes,
        }),
        None if fields.is_empty() => None,
        None => Some(BuiltBody {
            content_type: "application/json".into(),
            bytes: serde_json::to_vec(&Value::Object(fields)).map_err(|e| e.to_string())?,
        }),
    };
    Ok(Built {
        method: method.to_ascii_uppercase(),
        path: path.to_owned(),
        query,
        headers: Vec::new(),
        body,
    })
}

/// `?a=1&b=2` (empty string without parameters), names and values percent-encoded.
#[must_use]
pub fn query_string(query: &[(String, String)]) -> String {
    if query.is_empty() {
        return String::new();
    }
    format!(
        "?{}",
        query
            .iter()
            .map(|(k, v)| format!("{}={}", pct(k), pct(v)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(json: &str) -> Operation {
        serde_json::from_str(json).unwrap()
    }

    fn args(v: &[&str]) -> Vec<Arg> {
        parse_args(&v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()).unwrap()
    }

    const GET: &str = r#"{"id":"get_user","method":"GET","path":"/users/{id}/repos","summary":"s",
      "params":[
        {"name":"id","in":"path","required":true,"type":"string"},
        {"name":"limit","in":"query","type":"integer"},
        {"name":"fork","in":"query","type":"boolean"},
        {"name":"kind","in":"query","type":"string","enum":["a","b"]},
        {"name":"tag","in":"query","type":"array"},
        {"name":"X-Trace","in":"header","type":"string"},
        {"name":"depth","in":"query","required":true,"type":"integer","default":2}]}"#;

    #[test]
    fn a_path_value_is_one_encoded_segment() {
        let o = op(GET);
        let b = build_operation(&o, &args(&["id=a/b?c#d e", "limit=5"]), None).unwrap();
        assert_eq!(b.path, "/users/a%2Fb%3Fc%23d%20e/repos");
        for bad in [".", "..", ""] {
            let e = build_operation(&o, &args(&[&format!("id={bad}")]), None);
            assert!(e.is_err(), "{bad:?}");
        }
        // A value cannot add a segment, however it is spelled.
        let b = build_operation(&o, &args(&["id=../../etc/passwd"]), None).unwrap();
        assert_eq!(b.path, "/users/..%2F..%2Fetc%2Fpasswd/repos");
    }

    #[test]
    fn arguments_are_coerced_and_checked_before_anything_is_sent() {
        let o = op(GET);
        let b = build_operation(
            &o,
            &args(&[
                "id=7",
                "limit=5",
                "fork=true",
                "kind=b",
                "tag=x",
                "tag=y z",
                "X-Trace=t1",
            ]),
            None,
        )
        .unwrap();
        assert_eq!(
            query_string(&b.query),
            "?limit=5&fork=true&kind=b&tag=x&tag=y%20z&depth=2"
        );
        assert_eq!(b.headers, vec![("X-Trace".to_owned(), "t1".to_owned())]);
        for (bad, why) in [
            (vec!["id=1", "limit=five"], "integer"),
            (vec!["id=1", "fork=yes"], "true or false"),
            (vec!["id=1", "kind=c"], "not one of a, b"),
            (vec!["id=1", "nope=1"], "unknown argument"),
            (vec!["limit=1"], "missing required id"),
            (vec!["id=1", "limit=1", "limit=2"], "more than once"),
            (vec!["id=1", "limit:=3"], "body fields"),
        ] {
            let e = build_operation(&o, &args(&bad), None).unwrap_err();
            assert!(e.contains(why), "{bad:?}: {e}");
        }
    }

    #[test]
    fn credential_headers_are_never_built() {
        let o = op(r#"{"id":"x","method":"GET","path":"/x","summary":"s",
            "params":[{"name":"Authorization","in":"header","type":"string"}]}"#);
        let e = build_operation(&o, &args(&["Authorization=Bearer abc"]), None).unwrap_err();
        assert!(e.contains("cannot be set"), "{e}");
        for h in ["Cookie", "X-Aurora-Connection-Id", "Proxy-Foo", "Host"] {
            assert!(header_forbidden(h), "{h}");
        }
    }

    #[test]
    fn bodies_are_built_from_top_level_fields_or_taken_whole() {
        let o = op(
            r#"{"id":"mk","method":"POST","path":"/things","summary":"s",
            "body":{"content_type":"application/json","fields":[
              {"name":"name","type":"string","required":true},
              {"name":"size","type":"integer"},
              {"name":"meta","type":"string"}]}}"#,
        );
        let b =
            build_operation(&o, &args(&["name=n", "size=3", "meta:={\"a\":[1]}"]), None).unwrap();
        let v: Value = serde_json::from_slice(&b.body.unwrap().bytes).unwrap();
        assert_eq!(v, serde_json::json!({"name":"n","size":3,"meta":{"a":[1]}}));
        // `--body` replaces the fields; field names are then unknown.
        let w = build_operation(&o, &[], Some(b"{\"x\":1}".to_vec())).unwrap();
        assert_eq!(w.body.unwrap().bytes, b"{\"x\":1}");
        assert!(build_operation(&o, &args(&["name=n"]), Some(vec![])).is_err());
        assert!(build_operation(&o, &args(&["size=3"]), None).is_err());
    }

    #[test]
    fn a_form_body_is_encoded_and_multipart_is_refused() {
        let f = op(r#"{"id":"f","method":"POST","path":"/f","summary":"s",
            "body":{"content_type":"application/x-www-form-urlencoded","fields":[
              {"name":"a b","type":"string"}]}}"#);
        let b = build_operation(&f, &args(&["a b=c&d"]), None).unwrap();
        assert_eq!(b.body.unwrap().bytes, b"a%20b=c%26d");
        let m = op(r#"{"id":"m","method":"POST","path":"/m","summary":"s",
            "body":{"content_type":"multipart/form-data","fields":[{"name":"a","type":"string"}]}}"#);
        assert!(build_operation(&m, &args(&["a=1"]), None).is_err());
    }

    #[test]
    fn path_option_refuses_anything_that_is_not_a_path() {
        for ok in ["/", "/a/b", "/a%20b/c;d=e:f@g", "/v1/things/*"] {
            validate_path(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in [
            "a/b",
            "//evil.test/x",
            "https://evil.test/x",
            "/a?b=1",
            "/a#f",
            "/a b",
            "/a\\b",
            "/../x",
            "/a/./b",
            "/a/%2e%2E/b",
            "/a%zz",
            "/a%2",
            "/a\n",
        ] {
            assert!(validate_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn adhoc_arguments_are_query_or_json_fields() {
        let b = build_adhoc("/things", "post", &args(&["q=a b", "n:=3"]), None).unwrap();
        assert_eq!(b.method, "POST");
        assert_eq!(query_string(&b.query), "?q=a%20b");
        assert_eq!(b.body.unwrap().bytes, b"{\"n\":3}");
    }

    #[test]
    fn argument_syntax() {
        assert_eq!(
            parse_args(&["a=b=c".into(), "k:={\"x\":1}".into()]).unwrap(),
            vec![
                Arg::Plain("a".into(), "b=c".into()),
                Arg::Raw("k".into(), serde_json::json!({"x":1}))
            ]
        );
        assert!(parse_args(&["novalue".into()]).is_err());
        assert!(parse_args(&["=x".into()]).is_err());
        assert!(parse_args(&["k:={".into()]).is_err());
    }
}
