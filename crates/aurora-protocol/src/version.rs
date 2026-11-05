//! Machine-plane protocol versions (`X-Aurora-Protocol: <plane>/<n>`).
//! See `docs/16-wire-protocols.md` §Cross-cutting conventions.
//!
//! The API accepts version N and N−1 of each plane; anything else is
//! rejected (HTTP 426 with the supported range).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// Header name carrying the protocol version (lowercase for HTTP/2 compatibility).
pub const HEADER: &str = "x-aurora-protocol";

/// Pattern for the header value; mirrors [`ProtocolVersion::parse`].
const VERSION_PATTERN: &str = r"^(runner|supervisor|connector|hookhost)/[1-9][0-9]*$";

/// Machine planes that carry the versioned protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Plane {
    Runner,
    Supervisor,
    Connector,
    Hookhost,
}

impl Plane {
    /// Wire name of the plane.
    pub const fn as_str(self) -> &'static str {
        match self {
            Plane::Runner => "runner",
            Plane::Supervisor => "supervisor",
            Plane::Connector => "connector",
            Plane::Hookhost => "hookhost",
        }
    }

    /// Current (latest) protocol version of this plane.
    pub const fn current(self) -> u32 {
        match self {
            Plane::Runner => 1,
            Plane::Supervisor => 1,
            Plane::Connector => 1,
            Plane::Hookhost => 1,
        }
    }

    /// Whether this plane accepts version `n` (current and current−1).
    pub const fn accepts(self, n: u32) -> bool {
        accepts_range(self.current(), n)
    }

    /// Inclusive range of accepted versions, `(min, max)`.
    pub const fn supported_range(self) -> (u32, u32) {
        supported_range(self.current())
    }
}

impl fmt::Display for Plane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Plane {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "runner" => Ok(Plane::Runner),
            "supervisor" => Ok(Plane::Supervisor),
            "connector" => Ok(Plane::Connector),
            "hookhost" => Ok(Plane::Hookhost),
            _ => Err(VersionError::UnknownPlane),
        }
    }
}

/// Returns whether `n` is `current` or `current − 1` (and at least 1).
pub const fn accepts_range(current: u32, n: u32) -> bool {
    n >= 1 && n <= current && n.saturating_add(1) >= current
}

/// Inclusive supported range for a plane whose current version is `current`.
pub const fn supported_range(current: u32) -> (u32, u32) {
    let min = if current > 1 { current - 1 } else { 1 };
    (min, current)
}

/// A parsed `<plane>/<n>` protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProtocolVersion {
    pub plane: Plane,
    pub n: u32,
}

/// Errors from parsing or checking a protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum VersionError {
    /// The value is not of the form `<plane>/<n>` with a canonical decimal `n`.
    #[error("malformed protocol version")]
    Malformed,
    /// The plane name is not one of the known machine planes.
    #[error("unknown protocol plane")]
    UnknownPlane,
    /// The version is outside the accepted window (answer HTTP 426).
    #[error("unsupported {plane} protocol version {n}; supported {min}..={max}")]
    Unsupported {
        plane: Plane,
        n: u32,
        min: u32,
        max: u32,
    },
}

impl ProtocolVersion {
    /// Parses a header value such as `runner/1`. Input is not echoed in errors.
    pub fn parse(s: &str) -> Result<Self, VersionError> {
        let (plane_s, n_s) = s.split_once('/').ok_or(VersionError::Malformed)?;
        let plane = plane_s.parse::<Plane>()?;
        let digits = !n_s.is_empty() && n_s.bytes().all(|b| b.is_ascii_digit());
        if !digits || (n_s.len() > 1 && n_s.starts_with('0')) {
            return Err(VersionError::Malformed);
        }
        // Overflow is a well-formed but unsupported version, not a malformed one.
        let n = n_s.parse::<u32>().unwrap_or(u32::MAX);
        Ok(ProtocolVersion { plane, n })
    }

    /// Returns `Ok(())` if this version is accepted, else `Unsupported`.
    pub fn check(&self) -> Result<(), VersionError> {
        if self.plane.accepts(self.n) {
            return Ok(());
        }
        let (min, max) = self.plane.supported_range();
        Err(VersionError::Unsupported {
            plane: self.plane,
            n: self.n,
            min,
            max,
        })
    }
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.plane, self.n)
    }
}

impl FromStr for ProtocolVersion {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for ProtocolVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ProtocolVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ProtocolVersion {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("ProtocolVersion")
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type": "string", "pattern": VERSION_PATTERN})
    }
}
