//! SCIM wire representations (`16 §W15`): `ListResponse`, `meta`, the User and Group
//! resources with exactly the attributes of the subset. Unknown attributes are dropped on
//! output; `roles`, `entitlements`, `password`, `x509Certificates` and `groups` on *input*
//! are refused by the service.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const USER_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
pub const GROUP_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";
pub const PATCH_OP_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";
pub const ERROR_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:Error";
pub const LIST_RESPONSE_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:ListResponse";

/// `meta` of every resource.
#[derive(Debug, Serialize)]
pub struct Meta {
    #[serde(rename = "resourceType")]
    pub resource_type: &'static str,
    pub created: String,
    pub last_modified: String,
    pub location: String,
}

/// One read-only group reference in a User resource.
#[derive(Debug, Serialize)]
pub struct GroupRef {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "$ref")]
    pub ref_: Option<String>,
}

/// One e-mail of a User resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Email {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub type_: Option<String>,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Name {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "givenName")]
    pub given_name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "familyName"
    )]
    pub family_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formatted: Option<String>,
}

/// The User resource as received (input) — every field optional; the service validates.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserInput {
    #[serde(default, rename = "userName")]
    pub user_name: Option<String>,
    #[serde(default, rename = "externalId")]
    pub external_id: Option<String>,
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub name: Option<Name>,
    #[serde(default)]
    pub emails: Option<Vec<Email>>,
    /// Boolean or the strings `"True"`/`"False"` (Entra).
    #[serde(default)]
    pub active: Option<Active>,
    /// Raw copy of the whole body: the service refuses unsupported attributes.
    #[serde(default)]
    pub raw: Value,
}

/// `active` accepts a boolean or the strings `"True"` / `"False"` (16 §W15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Active(pub bool);

impl Serialize for Active {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bool(self.0)
    }
}

impl<'de> Deserialize<'de> for Active {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        match v {
            Value::Bool(b) => Ok(Self(b)),
            Value::String(s) if s.eq_ignore_ascii_case("true") => Ok(Self(true)),
            Value::String(s) if s.eq_ignore_ascii_case("false") => Ok(Self(false)),
            _ => Err(serde::de::Error::custom("active must be true or false")),
        }
    }
}

/// The User resource as returned.
#[derive(Debug, Serialize)]
pub struct UserResource {
    pub schemas: [&'static str; 1],
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "externalId")]
    pub external_id: Option<String>,
    #[serde(rename = "userName")]
    pub user_name: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<Name>,
    pub emails: Vec<Email>,
    pub active: bool,
    /// Read-only: the SCIM groups of this org the user is in.
    pub groups: Vec<GroupRef>,
    pub meta: Meta,
}

/// A group member as received (`value` = SCIM user id; `type` ignored).
#[derive(Debug, Clone, Deserialize)]
pub struct MemberInput {
    pub value: String,
    #[serde(default)]
    #[serde(rename = "type")]
    pub type_: Option<String>,
    #[serde(default)]
    pub display: Option<String>,
}

/// The Group resource as received.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GroupInput {
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(default, rename = "externalId")]
    pub external_id: Option<String>,
    #[serde(default)]
    pub members: Option<Vec<MemberInput>>,
    #[serde(default)]
    pub raw: Value,
}

/// The Group resource as returned.
#[derive(Debug, Serialize)]
pub struct GroupResource {
    pub schemas: [&'static str; 1],
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "externalId")]
    pub external_id: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: String,
    /// Omitted from lists unless `?attributes=members` (16 §W15).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<GroupMemberRef>>,
    pub meta: Meta,
}

#[derive(Debug, Serialize)]
pub struct GroupMemberRef {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "$ref")]
    pub ref_: Option<String>,
}

/// `ListResponse` (`16 §W15`): `count = 0` returns only `totalResults` (`Resources: []`).
#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub schemas: [&'static str; 1],
    #[serde(rename = "totalResults")]
    pub total_results: u64,
    #[serde(rename = "startIndex")]
    pub start_index: u64,
    #[serde(rename = "itemsPerPage")]
    pub items_per_page: u64,
    #[serde(rename = "Resources")]
    pub resources: Vec<Value>,
}

/// One PATCH operation.
#[derive(Debug, Clone, Deserialize)]
pub struct PatchOp {
    /// `add` | `replace` | `remove`, case-insensitive.
    pub op: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchInput {
    #[serde(default)]
    pub schemas: Vec<String>,
    #[serde(rename = "Operations")]
    pub operations: Vec<PatchOp>,
}

/// `PUT` / `PATCH` share the query parameters of the list endpoints.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default, rename = "startIndex")]
    pub start_index: Option<u64>,
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub attributes: Option<String>,
}

pub fn location(external_url: &str, kind: &str, id: Uuid) -> String {
    format!(
        "{}/scim/v2/{}/{}",
        external_url.trim_end_matches('/'),
        kind,
        id
    )
}
