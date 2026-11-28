//! GraphQL operation-kind classification, fail-closed (07 §Matching primitives).
use crate::json::{self, Lookup, Node};
use crate::params;
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
/// truncated or unparsable body, subscriptions, a URL that carries an operation of its own, or
/// any doubt => `Mutation`.
pub fn classify(body_prefix: &[u8], truncated: bool, url_query: Option<&str>) -> GraphqlKind {
    if truncated || url_query.is_some_and(url_may_override) || !is_query_body(body_prefix) {
        GraphqlKind::Mutation
    } else {
        GraphqlKind::Query
    }
}

/// The request parameters of a GraphQL operation. Some servers let the URL's override the
/// body's.
const OPERATION_PARAMETERS: [&str; 3] = ["query", "operationName", "extensions"];

/// Whether the URL may carry one of [`OPERATION_PARAMETERS`], read generously: pairs split at
/// `&` and `;`, names decoded, compared without case and without a `[…]` suffix. A name that
/// cannot be decoded may be any of them.
fn url_may_override(query: &str) -> bool {
    query.split(['&', ';']).any(|pair| {
        let name = pair.split('=').next().unwrap_or_default();
        let Some(name) = params::decode(name.as_bytes(), true) else {
            return true;
        };
        let name = name.split('[').next().unwrap_or_default().trim();
        OPERATION_PARAMETERS
            .iter()
            .any(|p| json::fold(name).eq(json::fold(p)))
    })
}

fn is_query_body(body: &[u8]) -> bool {
    // The kind is decided here and the server gets the bytes: a `query` or `operationName`
    // with more than one reading is doubt (`crate::json`).
    match json::parse(body) {
        Some(Node::Array(items)) => !items.is_empty() && items.iter().all(is_query_request),
        Some(v) => is_query_request(&v),
        None => false,
    }
}

fn is_query_request(v: &Node) -> bool {
    let Lookup::Found(Node::Scalar(Value::String(query))) = v.get("query") else {
        return false;
    };
    let name = match v.get("operationName") {
        Lookup::Missing | Lookup::Found(Node::Scalar(Value::Null)) => None,
        Lookup::Found(Node::Scalar(Value::String(s))) => Some(s.as_str()),
        Lookup::Found(_) | Lookup::Ambiguous => return false,
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
        classify(body.as_bytes(), false, None)
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
        assert_eq!(classify(br#"{"query":"{ a }"}"#, true, None), Mutation);
        assert_eq!(c("not json"), Mutation);
        assert_eq!(c(""), Mutation);
        assert_eq!(c(r#"{"operationName":"x"}"#), Mutation);
        assert_eq!(c(r#"{"query":5}"#), Mutation);
        assert_eq!(c(r#"{"query":"{ a "}"#), Mutation);
        assert_eq!(c(r#"{"query":"fragment F on T { a }"}"#), Mutation);
        assert_eq!(c(r#""query""#), Mutation);
    }
}
