//! The optional `redis` backend (`15 §Shared rate-limit counters`): standalone Redis or a
//! primary behind Sentinel failover, **not Cluster**. Every operation is one atomic Lua script
//! (`EVALSHA`, loaded on first use and on `NOSCRIPT` by [`redis::Script`]); the clock is the
//! Redis `TIME`; keys expire with `PEXPIRE`/`PX`.

use crate::address::{RedisAddr, check_key_prefix, parse_address};
use crate::{
    BucketTake, CounterError, CounterStore, Result, Verdict, WindowProbe, WindowUnit, check_key,
    interval_ms, normalize_takes, normalize_units,
};
use async_trait::async_trait;
use redis::aio::MultiplexedConnection;
use redis::{IntoConnectionInfo, RedisConnectionInfo, Script, TlsCertificates};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::Mutex;

/// Window keys: `<prefix>w:<key>:<period s>:<window start ms>`.
const PEEK: &str = r"
local function num(v, dflt)
  if not v then return dflt end
  local n = tonumber(v)
  if not n then error('ERR counter value is not numeric') end
  return n
end
local t = redis.call('TIME')
local now = t[1] * 1000 + math.floor(t[2] / 1000)
local prefix = ARGV[1]
local out = {}
for i = 1, tonumber(ARGV[2]) do
  local key = ARGV[1 + (i - 1) * 2 + 2]
  local per = tonumber(ARGV[1 + (i - 1) * 2 + 3])
  local ws = now - (now % per)
  out[i] = num(redis.call('GET', prefix .. 'w:' .. key .. ':' .. per .. ':' .. ws), 0)
end
return out
";

const CONSUME: &str = r"
local function num(v, dflt)
  if not v then return dflt end
  local n = tonumber(v)
  if not n then error('ERR counter value is not numeric') end
  return n
end
local t = redis.call('TIME')
local now = t[1] * 1000 + math.floor(t[2] / 1000)
local prefix = ARGV[1]
local n = tonumber(ARGV[2])
local pend, ttl = {}, {}
local worst_i, worst_ms = 0, 0
for i = 1, n do
  local b = 2 + (i - 1) * 4
  local key, per = ARGV[b + 1], tonumber(ARGV[b + 2])
  local limit, cost = tonumber(ARGV[b + 3]), tonumber(ARGV[b + 4])
  local ws = now - (now % per)
  local k = prefix .. 'w:' .. key .. ':' .. per .. ':' .. ws
  local cur = num(redis.call('GET', k), 0) + (pend[k] or 0)
  if cur + cost > limit then
    local ms = ws + per - now
    if ms > worst_ms then worst_ms = ms; worst_i = i end
  else
    pend[k] = (pend[k] or 0) + cost
    ttl[k] = per + 60000
  end
end
if worst_i > 0 then return {0, worst_i, worst_ms} end
for k, c in pairs(pend) do
  redis.call('INCRBY', k, c)
  redis.call('PEXPIRE', k, ttl[k])
end
return {1, 0, 0}
";

/// GCRA: allowed iff `max(tat, now) + T - now <= burst * T`.
const TAKE: &str = r"
local function num(v, dflt)
  if not v then return dflt end
  local n = tonumber(v)
  if not n then error('ERR counter value is not numeric') end
  return n
end
local t = redis.call('TIME')
local now = t[1] * 1000 + math.floor(t[2] / 1000)
local prefix = ARGV[1]
local n = tonumber(ARGV[2])
local pend = {}
local worst_i, worst_ms = 0, 0
for i = 1, n do
  local b = 2 + (i - 1) * 3
  local key, tms, burst = ARGV[b + 1], tonumber(ARGV[b + 2]), tonumber(ARGV[b + 3])
  local k = prefix .. 'b:' .. key
  local tat = pend[k] or num(redis.call('GET', k), now)
  if tat < now then tat = now end
  local new = tat + tms
  local allow_at = new - burst * tms
  if allow_at > now then
    local ms = allow_at - now
    if ms > worst_ms then worst_ms = ms; worst_i = i end
  else
    pend[k] = new
  end
end
if worst_i > 0 then return {0, worst_i, worst_ms} end
for k, v in pairs(pend) do
  redis.call('SET', k, v, 'PX', (v - now) + 60000)
end
return {1, 0, 0}
";

/// Connection settings; the credentials come from environment variables the caller read, never
/// from the URL (`08 §4`).
#[derive(Clone)]
pub struct RedisOptions {
    pub address: String,
    pub allow_plaintext: bool,
    pub username: Option<String>,
    pub password: Option<String>,
    pub ca_file: Option<PathBuf>,
    pub db: i64,
    pub key_prefix: String,
}

impl std::fmt::Debug for RedisOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisOptions")
            .field("address", &self.address)
            .field("db", &self.db)
            .field("key_prefix", &self.key_prefix)
            .finish_non_exhaustive()
    }
}

pub struct RedisStore {
    client: redis::Client,
    prefix: String,
    conn: Mutex<Option<MultiplexedConnection>>,
    peek: Script,
    consume: Script,
    take: Script,
    pub addr: RedisAddr,
}

fn unavailable(e: &redis::RedisError) -> CounterError {
    CounterError::Unavailable(format!("redis: {:?}", e.kind()))
}

impl RedisStore {
    /// Validate the options and build the client (no connection yet; the first call connects).
    pub fn new(o: &RedisOptions) -> std::result::Result<Self, String> {
        let addr = parse_address(&o.address, o.allow_plaintext)?;
        check_key_prefix(&o.key_prefix)?;
        let base = format!("redis://{}:{}", addr.host, addr.port);
        let mut info = base
            .as_str()
            .into_connection_info()
            .map_err(|e| format!("invalid address: {e}"))?;
        let mut settings = RedisConnectionInfo::default().set_db(o.db);
        if let Some(u) = &o.username {
            settings = settings.set_username(u);
        }
        if let Some(p) = &o.password {
            settings = settings.set_password(p);
        }
        info = info.set_redis_settings(settings);
        if addr.tls {
            info = info.set_addr(redis::ConnectionAddr::TcpTls {
                host: addr.host.trim_matches(['[', ']']).to_owned(),
                port: addr.port,
                insecure: false,
                tls_params: None,
            });
        }
        let client = match &o.ca_file {
            Some(p) => {
                let pem = std::fs::read(p)
                    .map_err(|e| format!("cannot read ca_file {}: {e}", p.display()))?;
                redis::Client::build_with_tls(
                    info,
                    TlsCertificates {
                        client_tls: None,
                        root_cert: Some(pem),
                    },
                )
                .map_err(|e| format!("invalid ca_file: {e}"))?
            }
            None => redis::Client::open(info).map_err(|e| format!("invalid address: {e}"))?,
        };
        Ok(Self {
            client,
            prefix: o.key_prefix.clone(),
            conn: Mutex::new(None),
            peek: Script::new(PEEK),
            consume: Script::new(CONSUME),
            take: Script::new(TAKE),
            addr,
        })
    }

    async fn conn(&self) -> Result<MultiplexedConnection> {
        let mut g = self.conn.lock().await;
        if let Some(c) = g.as_ref() {
            return Ok(c.clone());
        }
        let c = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| unavailable(&e))?;
        *g = Some(c.clone());
        Ok(c)
    }

    async fn reset(&self) {
        *self.conn.lock().await = None;
    }

    async fn script_call(&self, script: &Script, args: Vec<String>) -> Result<Vec<i64>> {
        let mut c = self.conn().await?;
        let mut inv = script.prepare_invoke();
        for a in &args {
            inv.arg(a.as_str());
        }
        match inv.invoke_async::<Vec<i64>>(&mut c).await {
            Ok(v) => Ok(v),
            Err(e) => {
                self.reset().await;
                Err(unavailable(&e))
            }
        }
    }
}

fn verdict(r: &[i64], keys: &[&str]) -> Result<Verdict> {
    match r {
        [1, ..] => Ok(Verdict::Allowed),
        [0, i, ms] => {
            let idx = usize::try_from(*i).unwrap_or(0).saturating_sub(1);
            let key = keys
                .get(idx)
                .ok_or_else(|| CounterError::Invalid("bad script reply".into()))?;
            Ok(Verdict::Refused {
                key: (*key).to_owned(),
                retry_after: Duration::from_millis(u64::try_from(*ms).unwrap_or(1).max(1)),
            })
        }
        _ => Err(CounterError::Invalid("bad script reply".into())),
    }
}

#[async_trait]
impl CounterStore for RedisStore {
    fn backend(&self) -> &'static str {
        "redis"
    }

    async fn fixed_window_peek(&self, probes: &[WindowProbe]) -> Result<Vec<u64>> {
        for p in probes {
            check_key(&p.key)?;
        }
        let mut args = vec![self.prefix.clone(), probes.len().to_string()];
        for p in probes {
            args.push(p.key.clone());
            args.push((p.period.millis()).to_string());
        }
        let r = self.script_call(&self.peek, args).await?;
        if r.len() != probes.len() {
            return Err(CounterError::Invalid("bad script reply".into()));
        }
        Ok(r.into_iter()
            .map(|n| u64::try_from(n).unwrap_or(0))
            .collect())
    }

    async fn fixed_window_consume(&self, units: &[WindowUnit]) -> Result<Verdict> {
        for u in units {
            check_key(&u.key)?;
        }
        let units = normalize_units(units);
        let mut args = vec![self.prefix.clone(), units.len().to_string()];
        for u in &units {
            args.push(u.key.clone());
            args.push(u.period.millis().to_string());
            args.push(u.limit.to_string());
            args.push(u.cost.to_string());
        }
        let r = self.script_call(&self.consume, args).await?;
        let keys: Vec<&str> = units.iter().map(|u| u.key.as_str()).collect();
        verdict(&r, &keys)
    }

    async fn bucket_take(&self, takes: &[BucketTake]) -> Result<Verdict> {
        for t in takes {
            check_key(&t.key)?;
        }
        let takes = normalize_takes(takes);
        let mut args = vec![self.prefix.clone(), takes.len().to_string()];
        for t in &takes {
            args.push(t.key.clone());
            args.push(interval_ms(t.emission_interval).to_string());
            args.push(t.burst.to_string());
        }
        let r = self.script_call(&self.take, args).await?;
        let keys: Vec<&str> = takes.iter().map(|t| t.key.as_str()).collect();
        verdict(&r, &keys)
    }

    async fn health(&self) -> Result<()> {
        let mut c = self.conn().await?;
        match redis::cmd("PING").query_async::<String>(&mut c).await {
            Ok(_) => Ok(()),
            Err(e) => {
                self.reset().await;
                Err(unavailable(&e))
            }
        }
    }
}
