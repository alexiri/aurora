//! SQL statement classification for the gateway database lane (docs/21 §Phase 2 — Database
//! lane, docs/06 §Database lane).
//!
//! [`classify`] is a pure function: it parses a statement string with the real PostgreSQL parser
//! (libpg_query), then walks the *complete* parse tree. The result is the strictest class of
//! every statement in the string; a statement on the always-refused list, an unparsable string
//! and an over-long string are refusals, never classes. Anything the walker does not recognise
//! is `admin`.
//!
//! CPU and memory are bounded by the 1 MiB cap and by the parser's own recursion limits; the
//! caller runs this off the async runtime.

use serde_json::Value;

/// Statement strings above this size are refused without parsing.
pub const MAX_STATEMENT_BYTES: usize = 1 << 20;

/// Statement classes, ordered from least to most strict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// BEGIN/COMMIT/ROLLBACK/SAVEPOINT and harmless session bookkeeping; always allowed.
    Transaction,
    Read,
    Write,
    Ddl,
    Admin,
}

impl Class {
    /// The path segment of the synthetic policy request (`POST /<class>`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transaction => "transaction",
            Self::Read => "read",
            Self::Write => "write",
            Self::Ddl => "ddl",
            Self::Admin => "admin",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "transaction" => Self::Transaction,
            "read" => Self::Read,
            "write" => Self::Write,
            "ddl" => Self::Ddl,
            "admin" => Self::Admin,
            _ => return None,
        })
    }
}

/// Why a statement string is refused outright (independent of any policy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    TooLarge,
    /// The parser rejected the string (or exceeded a recursion limit).
    Unparsable(String),
    /// On the always-refused list; the label names the rule.
    Forbidden(&'static str),
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Self::TooLarge => format!(
                "The statement is larger than {MAX_STATEMENT_BYTES} bytes and was not sent to the database."
            ),
            Self::Unparsable(m) => {
                format!("The statement could not be parsed ({m}) and was not sent to the database.")
            }
            Self::Forbidden(w) => format!(
                "{w} is never permitted through the Aurora database lane and was not sent to the database."
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classified {
    pub class: Class,
    /// Number of statements in the string (0 = empty query).
    pub statements: usize,
}

/// Settings a session may change freely (client preferences without data or privilege effect).
const SAFE_SESSION_SETTINGS: &[&str] = &[
    "extra_float_digits",
    "application_name",
    "timezone",
    "datestyle",
    "intervalstyle",
    "bytea_output",
    "client_min_messages",
];

/// Settings that change how the *next* statement text is parsed or decoded. Changing them would
/// let the server read a string differently from the classifier.
const PARSER_AFFECTING_SETTINGS: &[&str] = &[
    "standard_conforming_strings",
    "backslash_quote",
    "escape_string_warning",
    "client_encoding",
];

/// Functions (by unqualified name, `pg_catalog` or unqualified only) that are pure or read-only
/// and may appear inside a `read` statement. Any other function makes the statement `admin`.
const PURE_FUNCTIONS: &[&str] = &[
    // aggregates
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "array_agg",
    "string_agg",
    "bool_and",
    "bool_or",
    "every",
    "json_agg",
    "jsonb_agg",
    "json_object_agg",
    "jsonb_object_agg",
    "stddev",
    "stddev_pop",
    "stddev_samp",
    "variance",
    "var_pop",
    "var_samp",
    "percentile_cont",
    "percentile_disc",
    "mode",
    "corr",
    "covar_pop",
    "covar_samp",
    "bit_and",
    "bit_or",
    // window
    "row_number",
    "rank",
    "dense_rank",
    "percent_rank",
    "cume_dist",
    "ntile",
    "lag",
    "lead",
    "first_value",
    "last_value",
    "nth_value",
    // strings
    "lower",
    "upper",
    "length",
    "char_length",
    "character_length",
    "octet_length",
    "bit_length",
    "substr",
    "substring",
    "left",
    "right",
    "concat",
    "concat_ws",
    "format",
    "initcap",
    "trim",
    "btrim",
    "ltrim",
    "rtrim",
    "lpad",
    "rpad",
    "repeat",
    "replace",
    "reverse",
    "position",
    "strpos",
    "split_part",
    "translate",
    "overlay",
    "ascii",
    "chr",
    "md5",
    "encode",
    "decode",
    "regexp_replace",
    "regexp_match",
    "regexp_matches",
    "regexp_split_to_array",
    "regexp_split_to_table",
    "starts_with",
    "quote_ident",
    "quote_literal",
    "quote_nullable",
    "to_char",
    "to_number",
    "to_date",
    "to_timestamp",
    "to_hex",
    "unnest",
    "string_to_array",
    "array_to_string",
    "convert_from",
    "convert_to",
    // numbers
    "abs",
    "ceil",
    "ceiling",
    "floor",
    "round",
    "trunc",
    "sqrt",
    "cbrt",
    "power",
    "pow",
    "exp",
    "ln",
    "log",
    "log10",
    "mod",
    "div",
    "sign",
    "greatest",
    "least",
    "random_normal",
    "width_bucket",
    "gcd",
    "lcm",
    "degrees",
    "radians",
    "pi",
    "sin",
    "cos",
    "tan",
    // time (stable)
    "now",
    "current_timestamp",
    "current_date",
    "current_time",
    "localtime",
    "localtimestamp",
    "clock_timestamp",
    "statement_timestamp",
    "transaction_timestamp",
    "timeofday",
    "date_trunc",
    "date_part",
    "extract",
    "age",
    "make_date",
    "make_time",
    "make_timestamp",
    "make_interval",
    "date_bin",
    "justify_days",
    "justify_hours",
    "justify_interval",
    "isfinite",
    // arrays / json
    "array_length",
    "array_upper",
    "array_lower",
    "array_dims",
    "array_ndims",
    "cardinality",
    "array_append",
    "array_prepend",
    "array_cat",
    "array_remove",
    "array_replace",
    "array_position",
    "array_positions",
    "generate_series",
    "generate_subscripts",
    "json_build_object",
    "jsonb_build_object",
    "json_build_array",
    "jsonb_build_array",
    "to_json",
    "to_jsonb",
    "row_to_json",
    "json_extract_path",
    "jsonb_extract_path",
    "json_extract_path_text",
    "jsonb_extract_path_text",
    "json_array_length",
    "jsonb_array_length",
    "json_each",
    "jsonb_each",
    "json_each_text",
    "jsonb_each_text",
    "json_object_keys",
    "jsonb_object_keys",
    "json_array_elements",
    "jsonb_array_elements",
    "json_array_elements_text",
    "jsonb_array_elements_text",
    "jsonb_pretty",
    "jsonb_typeof",
    "json_typeof",
    "jsonb_path_exists",
    "jsonb_path_query",
    "jsonb_path_query_array",
    "jsonb_path_query_first",
    "jsonb_set",
    "jsonb_insert",
    "jsonb_strip_nulls",
    "json_strip_nulls",
    "jsonb_exists",
    "jsonb_contains",
    // misc
    "coalesce",
    "nullif",
    "isnull",
    "gen_random_uuid",
    "uuid_generate_v4",
    "random",
    "current_setting",
    "current_schema",
    "current_schemas",
    "current_database",
    "current_user",
    "session_user",
    "current_catalog",
    "version",
    "pg_typeof",
    "pg_backend_pid",
    "pg_get_expr",
    "pg_get_userbyid",
    "pg_table_is_visible",
    "pg_type_is_visible",
    "pg_function_is_visible",
    "pg_get_viewdef",
    "pg_get_indexdef",
    "pg_get_constraintdef",
    "pg_get_functiondef",
    "pg_get_triggerdef",
    "pg_get_ruledef",
    "pg_get_serial_sequence",
    "format_type",
    "obj_description",
    "col_description",
    "shobj_description",
    "has_table_privilege",
    "has_schema_privilege",
    "has_database_privilege",
    "has_column_privilege",
    "has_sequence_privilege",
    "has_function_privilege",
    "has_any_column_privilege",
    "pg_relation_size",
    "pg_total_relation_size",
    "pg_table_size",
    "pg_indexes_size",
    "pg_database_size",
    "pg_size_pretty",
    "pg_postmaster_start_time",
    "pg_is_in_recovery",
    "to_regclass",
    "to_regtype",
    "to_regproc",
    "to_regnamespace",
    "to_regrole",
    "int4",
    "int8",
    "float8",
    "numeric",
    "text",
    "varchar",
    "bool",
    "date",
    "timestamp",
    "timestamptz",
    "interval",
    "uuid",
    "bpchar",
    "inet",
    "host",
    "family",
    "masklen",
    "network",
    "netmask",
    "broadcast",
    "abbrev",
    "xmlagg",
    "xmlelement",
    "xmlforest",
    "xpath",
    "unistr",
    "similar_to_escape",
    "hashtext",
    "digest",
    "gen_salt",
    "bit_count",
    "get_bit",
    "get_byte",
    "set_bit",
    "set_byte",
    "array_fill",
    "trim_array",
    "txid_current_if_assigned",
    "pg_my_temp_schema",
    "pg_conf_load_time",
    "int2",
    "float4",
    "money",
    "lower_inc",
    "upper_inc",
    "isempty",
    "range_merge",
    "tsvector",
    "to_tsvector",
    "to_tsquery",
    "plainto_tsquery",
    "phraseto_tsquery",
    "websearch_to_tsquery",
    "ts_rank",
    "ts_rank_cd",
    "ts_headline",
    "setweight",
    "strip",
    "numnode",
    "querytree",
];

/// Functions that are always refused (matched on the unqualified name, any schema). Prefix
/// entries end with `*`.
const FORBIDDEN_FUNCTIONS: &[(&str, &str)] = &[
    ("lo_import", "lo_import"),
    ("lo_export", "lo_export"),
    ("lo_from_bytea", "large object creation from the client"),
    ("lo_put", "large object writes"),
    ("pg_read_*", "pg_read_file and its family"),
    ("pg_ls_*", "directory listing functions"),
    ("pg_stat_file", "pg_stat_file"),
    ("pg_file_*", "server file functions"),
    ("pg_logdir_ls", "pg_logdir_ls"),
    ("pg_reload_conf", "pg_reload_conf"),
    ("pg_rotate_logfile", "pg_rotate_logfile"),
    ("pg_promote", "pg_promote"),
    ("dblink*", "dblink"),
    ("postgres_fdw*", "postgres_fdw"),
    (
        "set_config",
        "set_config (it can change parser-affecting settings)",
    ),
];

const FORBIDDEN_EXTENSIONS: &[&str] = &["dblink", "postgres_fdw", "file_fdw", "adminpack"];

const DDL_VARIANTS: &[&str] = &[
    "RenameStmt",
    "TruncateStmt",
    "CommentStmt",
    "GrantStmt",
    "GrantRoleStmt",
    "DefineStmt",
    "ViewStmt",
    "IndexStmt",
    "CompositeTypeStmt",
    "RefreshMatViewStmt",
    "ReassignOwnedStmt",
    "DropOwnedStmt",
    "SecLabelStmt",
    "RuleStmt",
];

const FORBIDDEN_VARIANTS: &[(&str, &str)] = &[
    ("AlterSystemStmt", "ALTER SYSTEM"),
    ("LoadStmt", "LOAD"),
    ("CreateFdwStmt", "foreign data wrapper definitions"),
    ("AlterFdwStmt", "foreign data wrapper definitions"),
    ("CreateForeignServerStmt", "foreign server definitions"),
    ("AlterForeignServerStmt", "foreign server definitions"),
    ("CreateForeignTableStmt", "foreign tables"),
    ("CreateUserMappingStmt", "user mappings"),
    ("AlterUserMappingStmt", "user mappings"),
    ("DropUserMappingStmt", "user mappings"),
    ("ImportForeignSchemaStmt", "IMPORT FOREIGN SCHEMA"),
    ("CreateEventTrigStmt", "event triggers"),
];

fn forbidden_function(name: &str) -> Option<&'static str> {
    FORBIDDEN_FUNCTIONS.iter().find_map(|(pat, why)| {
        let hit = match pat.strip_suffix('*') {
            Some(p) => name.starts_with(p),
            None => name == *pat,
        };
        hit.then_some(*why)
    })
}

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn int_of(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or(0)
}

fn non_empty(v: &Value, k: &str) -> bool {
    match v.get(k) {
        None | Some(Value::Null) => false,
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// Final component of a `funcname` list and whether the name is unqualified or `pg_catalog`.
fn func_name(call: &Value) -> (String, bool) {
    let parts: Vec<&str> = call
        .get("funcname")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|n| n.get("node")?.get("String")?.get("sval")?.as_str())
                .collect()
        })
        .unwrap_or_default();
    let name = parts.last().copied().unwrap_or("").to_owned();
    let builtin_scope = match parts.len() {
        1 => true,
        2 => parts[0] == "pg_catalog",
        _ => false,
    };
    (name, builtin_scope)
}

/// The `sval`s of a list of `String` nodes (qualified names).
fn string_list(v: &Value) -> Vec<&str> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|n| n.get("node")?.get("String")?.get("sval")?.as_str())
                .collect()
        })
        .unwrap_or_default()
}

/// Types the server ships in `pg_catalog` whose input/output and cast functions are built in.
const BUILTIN_TYPES: &[&str] = &[
    "bool",
    "boolean",
    "bytea",
    "char",
    "name",
    "int8",
    "bigint",
    "int2",
    "smallint",
    "int4",
    "int",
    "integer",
    "text",
    "oid",
    "xid",
    "cid",
    "tid",
    "json",
    "jsonb",
    "jsonpath",
    "xml",
    "float4",
    "real",
    "float8",
    "float",
    "bpchar",
    "varchar",
    "date",
    "time",
    "timetz",
    "timestamp",
    "timestamptz",
    "interval",
    "bit",
    "varbit",
    "numeric",
    "decimal",
    "uuid",
    "inet",
    "cidr",
    "macaddr",
    "macaddr8",
    "money",
    "point",
    "line",
    "lseg",
    "box",
    "path",
    "polygon",
    "circle",
    "tsvector",
    "tsquery",
    "int4range",
    "int8range",
    "numrange",
    "tsrange",
    "tstzrange",
    "daterange",
    "int4multirange",
    "int8multirange",
    "nummultirange",
    "tsmultirange",
    "tstzmultirange",
    "datemultirange",
    "regclass",
    "regtype",
    "regproc",
    "regprocedure",
    "regoper",
    "regoperator",
    "regconfig",
    "regdictionary",
    "regnamespace",
    "regrole",
    "regcollation",
    "pg_lsn",
    "pg_snapshot",
    "txid_snapshot",
];

/// `true` for an unqualified built-in type name or any `pg_catalog`-qualified one.
fn builtin_type(names: &[&str]) -> bool {
    match names {
        [t] => BUILTIN_TYPES.contains(&t.to_ascii_lowercase().as_str()),
        ["pg_catalog", t] => BUILTIN_TYPES.contains(&t.to_ascii_lowercase().as_str()),
        _ => false,
    }
}

fn setting_name(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Whether an EXPLAIN option list asks for ANALYZE.
fn explain_analyzes(options: &Value) -> bool {
    let Some(opts) = options.as_array() else {
        return false;
    };
    opts.iter().any(|o| {
        let Some(d) = o.get("node").and_then(|n| n.get("DefElem")) else {
            return false;
        };
        if !matches!(str_of(d, "defname"), "analyze" | "analyse") {
            return false;
        }
        match d.get("arg").and_then(|a| a.get("node")) {
            None | Some(Value::Null) => true,
            Some(n) => {
                if let Some(b) = n.get("Boolean") {
                    return b.get("boolval").and_then(Value::as_bool).unwrap_or(true);
                }
                if let Some(i) = n.get("Integer") {
                    return i.get("ival").and_then(Value::as_i64).unwrap_or(1) != 0;
                }
                if let Some(s) = n.get("String") {
                    let s = str_of(s, "sval").to_ascii_lowercase();
                    return !matches!(s.as_str(), "false" | "off" | "0" | "f" | "n" | "no");
                }
                if let Some(c) = n.get("AConst") {
                    if let Some(b) = c.get("val").and_then(|v| v.get("Boolval")) {
                        return b.get("boolval").and_then(Value::as_bool).unwrap_or(true);
                    }
                    if let Some(i) = c.get("val").and_then(|v| v.get("Ival")) {
                        return i.get("ival").and_then(Value::as_i64).unwrap_or(1) != 0;
                    }
                    if let Some(s) = c.get("val").and_then(|v| v.get("Sval")) {
                        let s = str_of(s, "sval").to_ascii_lowercase();
                        return !matches!(s.as_str(), "false" | "off" | "0" | "f" | "n" | "no");
                    }
                }
                true
            }
        }
    })
}

struct Walk {
    class: Class,
}

impl Walk {
    fn raise(&mut self, c: Class) {
        if c > self.class {
            self.class = c;
        }
    }
}

/// The class a *top-level* statement variant contributes, or a refusal.
fn top_level(variant: &str, body: &Value, w: &mut Walk) -> Result<(), Refusal> {
    if let Some((_, why)) = FORBIDDEN_VARIANTS.iter().find(|(v, _)| *v == variant) {
        return Err(Refusal::Forbidden(why));
    }
    match variant {
        "SelectStmt" | "InsertStmt" | "UpdateStmt" | "DeleteStmt" | "MergeStmt" => {
            // Classified by the walk (a SELECT is `read` unless it locks, creates a table or
            // contains a data-modifying CTE).
            w.raise(Class::Read);
        }
        "VariableShowStmt" => w.raise(Class::Read),
        "ExplainStmt" => {
            w.raise(Class::Read);
            if explain_analyzes(&body["options"]) {
                // EXPLAIN ANALYZE executes the statement: never `read`.
                w.raise(Class::Admin);
            }
        }
        "CopyStmt" => {
            if body.get("is_program").and_then(Value::as_bool) == Some(true) {
                return Err(Refusal::Forbidden("COPY … PROGRAM"));
            }
            if non_empty(body, "filename") {
                return Err(Refusal::Forbidden("COPY to or from a server-side file"));
            }
            if body.get("is_from").and_then(Value::as_bool) == Some(true) {
                w.raise(Class::Write);
            } else {
                w.raise(Class::Read);
            }
        }
        "TransactionStmt" => match int_of(body, "kind") {
            1..=7 => w.raise(Class::Transaction),
            _ => w.raise(Class::Admin), // two-phase commit
        },
        "DeallocateStmt" | "ClosePortalStmt" | "FetchStmt" | "ConstraintsSetStmt" => {
            w.raise(Class::Transaction);
        }
        "DeclareCursorStmt" => w.raise(Class::Transaction),
        "PrepareStmt" => w.raise(Class::Transaction),
        "VariableSetStmt" => variable_set(body, w)?,
        "CreateExtensionStmt" => {
            let n = str_of(body, "extname").to_ascii_lowercase();
            if FORBIDDEN_EXTENSIONS.contains(&n.as_str()) {
                return Err(Refusal::Forbidden("this extension"));
            }
            w.raise(Class::Ddl);
        }
        v if v.starts_with("Create") || v.starts_with("Alter") || v.starts_with("Drop") => {
            w.raise(Class::Ddl);
        }
        v if DDL_VARIANTS.contains(&v) => w.raise(Class::Ddl),
        // VacuumStmt, ListenStmt, UnlistenStmt, NotifyStmt, DoStmt, CallStmt, ExecuteStmt,
        // LockStmt, ClusterStmt, ReindexStmt, CheckPointStmt, DiscardStmt and anything unknown.
        _ => w.raise(Class::Admin),
    }
    Ok(())
}

fn variable_set(body: &Value, w: &mut Walk) -> Result<(), Refusal> {
    let name = setting_name(str_of(body, "name"));
    let kind = int_of(body, "kind");
    if name == "role" {
        return Err(Refusal::Forbidden("SET ROLE"));
    }
    if name == "session_authorization" {
        return Err(Refusal::Forbidden("SET SESSION AUTHORIZATION"));
    }
    if PARSER_AFFECTING_SETTINGS.contains(&name.as_str()) {
        return Err(Refusal::Forbidden(
            "changing a setting that alters how statements are parsed",
        ));
    }
    match kind {
        // SET TRANSACTION characteristics (this transaction only)
        4 if name == "transaction" => w.raise(Class::Transaction),
        // RESET ALL
        6 => w.raise(Class::Admin),
        _ if SAFE_SESSION_SETTINGS.contains(&name.as_str()) => w.raise(Class::Transaction),
        _ => w.raise(Class::Admin),
    }
    Ok(())
}

/// Walks one statement tree. `executing` is false below a plain `EXPLAIN` (the inner statement
/// is only planned): the always-refused rules still apply, class contributions do not.
fn walk(root: &Value, w: &mut Walk) -> Result<(), Refusal> {
    let mut stack: Vec<(&Value, bool)> = vec![(root, true)];
    while let Some((v, executing)) = stack.pop() {
        match v {
            Value::Array(a) => stack.extend(a.iter().map(|x| (x, executing))),
            Value::Object(m) => {
                for (k, child) in m {
                    let mut child_exec = executing;
                    match k.as_str() {
                        "FuncCall" => {
                            let (name, builtin) = func_name(child);
                            if let Some(why) = forbidden_function(&name) {
                                return Err(Refusal::Forbidden(why));
                            }
                            if executing && !(builtin && PURE_FUNCTIONS.contains(&name.as_str())) {
                                w.raise(Class::Admin);
                            }
                        }
                        // (FIX-118, docs/21 §Database lane: "anything unknown → admin") An operator
                        // spelled with a schema other than `pg_catalog` runs a user-defined
                        // function (`OPERATOR(public.!!)`); so does a cast to a type outside the
                        // built-in catalogue (its I/O functions are user code).
                        "AExpr" if executing => {
                            let names = string_list(&child["name"]);
                            if names.len() > 1 && names[0] != "pg_catalog" {
                                w.raise(Class::Admin);
                            }
                        }
                        "TypeCast" if executing => {
                            if !builtin_type(&string_list(&child["type_name"]["names"])) {
                                w.raise(Class::Admin);
                            }
                        }
                        "SelectStmt" if executing => {
                            w.raise(Class::Read);
                            if non_empty(child, "locking_clause") {
                                w.raise(Class::Write);
                            }
                            if non_empty(child, "into_clause") {
                                w.raise(Class::Ddl);
                            }
                        }
                        "InsertStmt" | "UpdateStmt" | "DeleteStmt" | "MergeStmt" if executing => {
                            w.raise(Class::Write);
                        }
                        "ExplainStmt" => {
                            if !explain_analyzes(&child["options"]) {
                                child_exec = false;
                            } else {
                                w.raise(Class::Admin);
                            }
                        }
                        "VariableSetStmt" | "CopyStmt" | "CreateExtensionStmt" => {
                            // Nested utility statements (rule actions): same refusal rules.
                            let mut inner = Walk {
                                class: Class::Transaction,
                            };
                            top_level(k, child, &mut inner)?;
                            if executing {
                                w.raise(inner.class);
                            }
                        }
                        v if FORBIDDEN_VARIANTS.iter().any(|(n, _)| *n == v) => {
                            top_level(
                                v,
                                child,
                                &mut Walk {
                                    class: Class::Transaction,
                                },
                            )?;
                        }
                        _ => {}
                    }
                    stack.push((child, child_exec));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Classifies a statement string (one or more statements). See the module documentation.
pub fn classify(sql: &str) -> Result<Classified, Refusal> {
    if sql.len() > MAX_STATEMENT_BYTES {
        return Err(Refusal::TooLarge);
    }
    let parsed = pg_query::parse(sql, 0).map_err(|e| {
        let m = e.to_string();
        let m = m.strip_prefix("Invalid statement: ").unwrap_or(&m);
        Refusal::Unparsable(m.chars().take(200).collect())
    })?;
    let tree = serde_json::to_value(&parsed.protobuf)
        .map_err(|_| Refusal::Unparsable("parse tree too deep".into()))?;
    let stmts = tree
        .get("stmts")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut w = Walk {
        class: Class::Transaction,
    };
    let mut count = 0;
    for raw in stmts {
        let Some((variant, body)) = raw
            .get("stmt")
            .and_then(|s| s.get("node"))
            .and_then(Value::as_object)
            .and_then(|m| m.iter().next())
        else {
            continue;
        };
        count += 1;
        top_level(variant, body, &mut w)?;
        // Walk the wrapper so the statement's own variant key is visited too.
        if let Some(node) = raw.get("stmt") {
            walk(node, &mut w)?;
        }
    }
    Ok(Classified {
        class: w.class,
        statements: count,
    })
}

/// A statement text safe to show to a reviewer and to store: string, number, dollar-quoted and
/// bit-string literals become `?`, comments are dropped, whitespace is collapsed, and the result
/// is clamped to `max_chars` (with an ellipsis). Purely lexical, so it also covers utility
/// statements whose literals a parser-based normaliser would keep (`CREATE ROLE … PASSWORD '…'`).
pub fn redact(sql: &str, max_chars: usize) -> String {
    let b = sql.as_bytes();
    let mut out = String::with_capacity(sql.len().min(max_chars * 4 + 8));
    let mut i = 0;
    let push_ws = |out: &mut String| {
        if !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
    };
    while i < b.len() {
        let c = b[i];
        match c {
            b'\'' => {
                i = skip_quoted(b, i, b'\'', false);
                out.push('?');
            }
            b'"' => {
                // quoted identifier: kept
                let end = skip_quoted(b, i, b'"', false);
                out.push_str(&sql[i..end]);
                i = end;
            }
            b'$' => {
                if let Some(end) = dollar_end(b, i) {
                    i = end;
                    out.push('?');
                } else if b.get(i + 1).is_some_and(u8::is_ascii_digit) {
                    // $1 parameter placeholder: kept
                    let mut j = i + 1;
                    while b.get(j).is_some_and(u8::is_ascii_digit) {
                        j += 1;
                    }
                    out.push_str(&sql[i..j]);
                    i = j;
                } else {
                    out.push('$');
                    i += 1;
                }
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                push_ws(&mut out);
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let mut depth = 0usize;
                while i < b.len() {
                    if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                push_ws(&mut out);
            }
            b'0'..=b'9' => {
                // numeric literal (but not the digits of an identifier such as t1)
                let prev_ident = out
                    .chars()
                    .last()
                    .is_some_and(|p| p.is_alphanumeric() || p == '_' || p == '$');
                if prev_ident {
                    out.push(c as char);
                    i += 1;
                } else {
                    while i < b.len()
                        && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_')
                    {
                        i += 1;
                    }
                    out.push('?');
                }
            }
            c if c.is_ascii_whitespace() => {
                push_ws(&mut out);
                i += 1;
            }
            c if c == b'E'
                || c == b'e'
                || c == b'B'
                || c == b'b'
                || c == b'X'
                || c == b'x'
                || c == b'N'
                || c == b'n'
                || c == b'U'
                || c == b'u' =>
            {
                // E'..' / B'..' / X'..' / N'..' / U&'..' prefixed string literals
                let ident_before = out
                    .chars()
                    .last()
                    .is_some_and(|p| p.is_alphanumeric() || p == '_' || p == '$');
                let (quote_at, escapes) = if !ident_before && b.get(i + 1) == Some(&b'\'') {
                    (Some(i + 1), c == b'E' || c == b'e')
                } else if !ident_before
                    && (c == b'U' || c == b'u')
                    && b.get(i + 1) == Some(&b'&')
                    && b.get(i + 2) == Some(&b'\'')
                {
                    (Some(i + 2), false)
                } else {
                    (None, false)
                };
                if let Some(q) = quote_at {
                    i = skip_quoted(b, q, b'\'', escapes);
                    out.push('?');
                } else {
                    push_char(sql, &mut i, &mut out);
                }
            }
            _ => push_char(sql, &mut i, &mut out),
        }
        if out.chars().count() > max_chars + 16 {
            break;
        }
    }
    let t = out.trim();
    if t.chars().count() > max_chars {
        let mut s: String = t.chars().take(max_chars).collect();
        s.push('…');
        s
    } else {
        t.to_owned()
    }
}

fn push_char(sql: &str, i: &mut usize, out: &mut String) {
    // `i` is always at a char boundary: every other branch advances over whole ASCII tokens.
    if let Some(ch) = sql[*i..].chars().next() {
        out.push(ch);
        *i += ch.len_utf8();
    } else {
        *i += 1;
    }
}

/// Index just past the closing quote of the literal starting at `start` (end of input if
/// unterminated: the rest is swallowed, i.e. redacted).
fn skip_quoted(b: &[u8], start: usize, q: u8, backslash: bool) -> usize {
    let mut i = start + 1;
    while i < b.len() {
        if backslash && b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == q {
            if b.get(i + 1) == Some(&q) {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    b.len()
}

/// End of a dollar-quoted string starting at `start`, if one starts there.
fn dollar_end(b: &[u8], start: usize) -> Option<usize> {
    let mut j = start + 1;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
        j += 1;
    }
    if b.get(j) != Some(&b'$') {
        return None;
    }
    // `$1$` style is not a tag (tags may not start with a digit)
    if b.get(start + 1).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let tag = &b[start..=j];
    let mut k = j + 1;
    while k + tag.len() <= b.len() {
        if &b[k..k + tag.len()] == tag {
            return Some(k + tag.len());
        }
        k += 1;
    }
    Some(b.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_literals_and_comments() {
        assert_eq!(
            redact(
                "select * from t1 where a = 'x''y' and b = 42 -- hi\n and c = $1",
                200
            ),
            "select * from t1 where a = ? and b = ? and c = $1"
        );
        assert_eq!(
            redact("create role r password E'a\\'b' /* c */ login", 200),
            "create role r password ? login"
        );
        assert_eq!(redact("do $f$ secret $f$", 200), "do ?");
        assert_eq!(redact("select 1.5e3, \"Col1\"", 200), "select ?, \"Col1\"");
        assert_eq!(redact("select 'unterminated", 200), "select ?");
        assert_eq!(
            redact(&"select 1 ".repeat(100), 20),
            "select ? select ? se…"
        );
    }

    #[test]
    fn empty_string_is_an_empty_query() {
        assert_eq!(
            classify("").unwrap(),
            Classified {
                class: Class::Transaction,
                statements: 0
            }
        );
        assert_eq!(classify(" ; ").unwrap().statements, 0);
    }

    #[test]
    fn size_cap() {
        let big = format!("select '{}'", "a".repeat(MAX_STATEMENT_BYTES));
        assert_eq!(classify(&big), Err(Refusal::TooLarge));
    }

    #[test]
    fn deep_nesting_is_refused_not_crashed() {
        // Runs on a 2 MiB stack, like a tokio blocking-pool thread.
        let r = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(|| {
                let sql = format!("select {}1{}", "(select ".repeat(3000), ")".repeat(3000));
                classify(&sql)
            })
            .unwrap()
            .join()
            .expect("no stack overflow");
        assert!(matches!(r, Err(Refusal::Unparsable(_))), "{r:?}");
    }
}
