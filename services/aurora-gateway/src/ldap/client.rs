//! The lane's LDAP client: LDAPS only, the server certificate verified against the configured
//! name, an anonymous simple bind, one search, an unbind. Nothing else can be sent.
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_rustls::TlsConnector;

use super::ber::{self, MAX_MESSAGE, Tlv};
use crate::guard::{DialError, Guard};
use crate::upstream::{EgressProxy, client_config, dial};

const MAX_VALUE_BYTES: usize = 4096;
const MAX_VALUES: usize = 100;

pub struct Lane {
    guard: Arc<Guard>,
    egress: Option<EgressProxy>,
    tls: TlsConnector,
    timeout: Duration,
}

pub struct Target<'a> {
    pub server: &'a str,
    pub port: u16,
}

/// A validated search: `base` is confined and `filter` is BER already (see `filter::encode`).
pub struct Query {
    pub base: String,
    /// 0 base, 1 one level, 2 subtree.
    pub scope: i64,
    pub filter: Vec<u8>,
    pub attributes: Vec<String>,
    pub limit: u32,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct Entry {
    pub dn: String,
    pub attributes: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum LdapError {
    #[error(transparent)]
    Dial(#[from] DialError),
    #[error("tls: {0}")]
    Tls(String),
    #[error("the directory did not answer in time")]
    Timeout,
    #[error("protocol: {0}")]
    Protocol(&'static str),
    #[error("the directory refused the search (result {0})")]
    Server(i64),
}

impl Lane {
    /// # Errors
    /// A bad extra CA bundle.
    pub fn new(
        guard: Arc<Guard>,
        egress: Option<EgressProxy>,
        extra_ca_pem: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<Self, String> {
        Ok(Self {
            guard,
            egress,
            tls: TlsConnector::from(client_config(extra_ca_pem, &[])?),
            timeout,
        })
    }

    /// # Errors
    /// Dial refusals, TLS failures, protocol errors, a server refusal or the lane's deadline.
    pub async fn search(&self, t: &Target<'_>, q: &Query) -> Result<Found, LdapError> {
        tokio::time::timeout(self.timeout, self.search_inner(t, q))
            .await
            .map_err(|_| LdapError::Timeout)?
    }

    async fn search_inner(&self, t: &Target<'_>, q: &Query) -> Result<Found, LdapError> {
        let tcp = dial(&self.guard, self.egress.as_ref(), t.server, t.port).await?;
        let name =
            ServerName::try_from(t.server.to_owned()).map_err(|e| LdapError::Tls(e.to_string()))?;
        let mut io = self
            .tls
            .connect(name, tcp)
            .await
            .map_err(|e| LdapError::Tls(e.to_string()))?;
        let found = exchange(&mut io, q).await;
        // Best effort: tell the server we are done whatever happened.
        let _ = io.write_all(&unbind(3)).await;
        let _ = io.shutdown().await;
        found
    }
}

fn message(id: i64, op: &[u8]) -> Vec<u8> {
    ber::tlv(
        ber::SEQUENCE,
        &ber::concat(&[ber::int(ber::INTEGER, id), op.to_vec()]),
    )
}

#[must_use]
pub fn bind_anonymous(id: i64) -> Vec<u8> {
    let op = ber::tlv(
        ber::OP_BIND_REQUEST,
        &ber::concat(&[
            ber::int(ber::INTEGER, 3),
            ber::octets(b""),
            ber::tlv(0x80, b""),
        ]),
    );
    message(id, &op)
}

#[must_use]
pub fn unbind(id: i64) -> Vec<u8> {
    message(id, &ber::tlv(ber::OP_UNBIND_REQUEST, b""))
}

#[must_use]
pub fn search_request(id: i64, q: &Query) -> Vec<u8> {
    let attrs: Vec<Vec<u8>> = q
        .attributes
        .iter()
        .map(|a| ber::octets(a.as_bytes()))
        .collect();
    let op = ber::tlv(
        ber::OP_SEARCH_REQUEST,
        &ber::concat(&[
            ber::octets(q.base.as_bytes()),
            ber::int(ber::ENUMERATED, q.scope),
            // derefAliases: never — an alias cannot lead a search out of the base.
            ber::int(ber::ENUMERATED, 0),
            // One more than the cap tells "exactly the cap" from "more than the cap".
            ber::int(ber::INTEGER, i64::from(q.limit) + 1),
            ber::int(ber::INTEGER, 10),
            ber::tlv(ber::BOOLEAN, &[0]),
            q.filter.clone(),
            ber::tlv(ber::SEQUENCE, &ber::concat(&attrs)),
        ]),
    );
    message(id, &op)
}

async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    io: &mut S,
    q: &Query,
) -> Result<Found, LdapError> {
    io.write_all(&bind_anonymous(1))
        .await
        .map_err(|_| LdapError::Protocol("write failed"))?;
    let m = read_message(io).await?;
    let (id, op) = split(&m)?;
    if id != 1 || op.tag != ber::OP_BIND_RESPONSE {
        return Err(LdapError::Protocol("unexpected answer to the bind"));
    }
    let code = result_code(&op)?;
    if code != 0 {
        return Err(LdapError::Server(code));
    }
    io.write_all(&search_request(2, q))
        .await
        .map_err(|_| LdapError::Protocol("write failed"))?;
    let mut found = Found::default();
    loop {
        let m = read_message(io).await?;
        let (id, op) = split(&m)?;
        if id != 2 {
            return Err(LdapError::Protocol("an answer for another message"));
        }
        match op.tag {
            ber::OP_SEARCH_ENTRY => {
                if found.entries.len() >= q.limit as usize {
                    found.truncated = true;
                    return Ok(found);
                }
                found.entries.push(parse_entry(&op, &q.attributes)?);
            }
            // Referrals are never followed.
            ber::OP_SEARCH_REFERENCE => {}
            ber::OP_SEARCH_DONE => {
                return match result_code(&op)? {
                    0 | 32 => Ok(found),
                    4 => {
                        found.truncated = true;
                        Ok(found)
                    }
                    c => Err(LdapError::Server(c)),
                };
            }
            _ => return Err(LdapError::Protocol("unexpected operation")),
        }
    }
}

fn split(m: &[u8]) -> Result<(i64, Tlv<'_>), LdapError> {
    let bad = LdapError::Protocol("malformed message");
    let (outer, rest) = ber::read(m).ok_or(LdapError::Protocol("malformed message"))?;
    if outer.tag != ber::SEQUENCE || !rest.is_empty() {
        return Err(bad);
    }
    let (idt, rest) = ber::read(outer.content).ok_or(LdapError::Protocol("malformed message"))?;
    let id = ber::as_int(&idt).ok_or(LdapError::Protocol("malformed message"))?;
    let (op, _controls) = ber::read(rest).ok_or(LdapError::Protocol("malformed message"))?;
    Ok((id, op))
}

fn result_code(op: &Tlv<'_>) -> Result<i64, LdapError> {
    let (c, _) = ber::read(op.content).ok_or(LdapError::Protocol("malformed result"))?;
    ber::as_int(&c).ok_or(LdapError::Protocol("malformed result"))
}

fn parse_entry(op: &Tlv<'_>, allowed: &[String]) -> Result<Entry, LdapError> {
    let bad = || LdapError::Protocol("malformed entry");
    let parts = ber::children(op.content).ok_or_else(bad)?;
    let [dn, attrs] = parts.as_slice() else {
        return Err(bad());
    };
    let mut out = BTreeMap::new();
    for a in ber::children(attrs.content).ok_or_else(bad)? {
        let pair = ber::children(a.content).ok_or_else(bad)?;
        let [name, vals] = pair.as_slice() else {
            return Err(bad());
        };
        let name = String::from_utf8_lossy(name.content);
        // Only attributes the manifest lists leave the lane, spelled as the manifest spells them.
        let Some(spelled) = allowed.iter().find(|x| x.eq_ignore_ascii_case(&name)) else {
            continue;
        };
        let values: Vec<String> = ber::children(vals.content)
            .ok_or_else(bad)?
            .into_iter()
            .filter(|v| v.content.len() <= MAX_VALUE_BYTES)
            .filter_map(|v| std::str::from_utf8(v.content).ok().map(str::to_owned))
            .take(MAX_VALUES)
            .collect();
        out.entry(spelled.clone())
            .or_insert_with(Vec::new)
            .extend(values);
    }
    Ok(Entry {
        dn: String::from_utf8_lossy(dn.content).into_owned(),
        attributes: out,
    })
}

async fn read_message<R: AsyncRead + Unpin>(r: &mut R) -> Result<Vec<u8>, LdapError> {
    let eof = |_| LdapError::Protocol("the directory closed the connection");
    let mut head = vec![0u8; 2];
    r.read_exact(&mut head).await.map_err(eof)?;
    let len = if head[1] < 0x80 {
        usize::from(head[1])
    } else {
        let n = usize::from(head[1] & 0x7f);
        if n == 0 || n > 4 {
            return Err(LdapError::Protocol("bad length"));
        }
        let mut lb = vec![0u8; n];
        r.read_exact(&mut lb).await.map_err(eof)?;
        head.extend_from_slice(&lb);
        lb.iter().fold(0usize, |a, b| (a << 8) | usize::from(*b))
    };
    if len > MAX_MESSAGE {
        return Err(LdapError::Protocol("message too large"));
    }
    let at = head.len();
    head.resize(at + len, 0);
    r.read_exact(&mut head[at..]).await.map_err(eof)?;
    Ok(head)
}
