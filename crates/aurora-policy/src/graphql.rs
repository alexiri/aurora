//! GraphQL operation-kind classification, fail-closed (07 §Matching primitives).
use graphql_parser::query::{Definition, OperationDefinition, parse_query};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphqlKind {
    Query,
    Mutation,
}

/// `Query` only if every operation that could execute is a query. Missing,
/// truncated or unparsable body, subscriptions, or any doubt => `Mutation`.
pub fn classify(body_prefix: &[u8], truncated: bool) -> GraphqlKind {
    if truncated || !is_query_body(body_prefix) {
        GraphqlKind::Mutation
    } else {
        GraphqlKind::Query
    }
}

fn is_query_body(body: &[u8]) -> bool {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Array(items)) => !items.is_empty() && items.iter().all(is_query_request),
        Ok(v) => is_query_request(&v),
        Err(_) => false,
    }
}

fn is_query_request(v: &Value) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };
    let Some(query) = obj.get("query").and_then(Value::as_str) else {
        return false;
    };
    let name = match obj.get("operationName") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.as_str()),
        Some(_) => return false,
    };
    let Ok(doc) = parse_query::<String>(query) else {
        return false;
    };
    let mut seen = 0usize;
    for def in &doc.definitions {
        let Definition::Operation(op) = def else {
            continue;
        };
        let (op_name, is_query) = match op {
            OperationDefinition::SelectionSet(_) => (None, true),
            OperationDefinition::Query(q) => (q.name.as_deref(), true),
            OperationDefinition::Mutation(m) => (m.name.as_deref(), false),
            OperationDefinition::Subscription(s) => (s.name.as_deref(), false),
        };
        match name {
            // Without operationName any operation might be chosen: all must be queries.
            None => {
                seen += 1;
                if !is_query {
                    return false;
                }
            }
            Some(n) => {
                if op_name == Some(n) {
                    seen += 1;
                    if !is_query {
                        return false;
                    }
                }
            }
        }
    }
    seen > 0
}

#[cfg(test)]
mod tests {
    use super::GraphqlKind::{Mutation, Query};
    use super::*;

    fn c(body: &str) -> GraphqlKind {
        classify(body.as_bytes(), false)
    }

    #[test]
    fn table() {
        assert_eq!(c(r#"{"query":"{ viewer { id } }"}"#), Query);
        assert_eq!(c(r#"{"query":"query Q { a }"}"#), Query);
        assert_eq!(c(r#"{"query":"mutation M { a }"}"#), Mutation);
        assert_eq!(c(r#"{"query":"subscription S { a }"}"#), Mutation);
        assert_eq!(c(r#"[{"query":"{a}"},{"query":"{b}"}]"#), Query);
        assert_eq!(
            c(r#"[{"query":"{a}"},{"query":"mutation { b }"}]"#),
            Mutation
        );
        assert_eq!(c("[]"), Mutation);
        // operationName selection
        let doc = "query Q { a } mutation M { b }";
        assert_eq!(
            c(&format!(r#"{{"query":"{doc}","operationName":"Q"}}"#)),
            Query
        );
        assert_eq!(
            c(&format!(r#"{{"query":"{doc}","operationName":"M"}}"#)),
            Mutation
        );
        assert_eq!(c(&format!(r#"{{"query":"{doc}"}}"#)), Mutation);
        assert_eq!(
            c(&format!(r#"{{"query":"{doc}","operationName":"Nope"}}"#)),
            Mutation
        );
        assert_eq!(c(r#"{"query":"{a}","operationName":5}"#), Mutation);
        assert_eq!(c(r#"{"query":"{a}","operationName":null}"#), Query);
        // doubt
        assert_eq!(classify(br#"{"query":"{ a }"}"#, true), Mutation);
        assert_eq!(c("not json"), Mutation);
        assert_eq!(c(""), Mutation);
        assert_eq!(c(r#"{"operationName":"x"}"#), Mutation);
        assert_eq!(c(r#"{"query":5}"#), Mutation);
        assert_eq!(c(r#"{"query":"{ a "}"#), Mutation);
        assert_eq!(c(r#"{"query":"fragment F on T { a }"}"#), Mutation);
        assert_eq!(c(r#""query""#), Mutation);
    }
}
