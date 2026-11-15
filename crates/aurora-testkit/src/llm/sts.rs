//! In-process mock of AWS STS (`AssumeRole`, `GetCallerIdentity`) in one region: it verifies the SigV4 signature of the
//! **source** key (service `sts`), records what `AssumeRole` asked for (`RoleArn`, `RoleSessionName`, `ExternalId`,
//! `DurationSeconds`), hands out session credentials of the assumed role (`ASIA…` key, its own secret and token) and can
//! answer `AccessDenied`. Tests register the session with the Bedrock mock so inference is verified against it.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::aws::{self, Refusal};
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use std::sync::{Arc, Mutex};

/// One `AssumeRole` call as STS received it.
#[derive(Debug, Clone, Default)]
pub struct AssumeSeen {
    /// The access key id that signed the call (the source key).
    pub signed_by: String,
    pub role_arn: String,
    pub role_session_name: String,
    pub external_id: String,
    pub duration_seconds: String,
}

type SessionHook = Box<dyn Fn(&str, &str, &str) + Send>;

struct Shared {
    source_key: String,
    source_secret: String,
    /// The account of the assumed role and of the key (`GetCallerIdentity`).
    account: String,
    deny: Mutex<bool>,
    assumed: Mutex<Vec<AssumeSeen>>,
    calls: Mutex<usize>,
    counter: Mutex<usize>,
    /// Session credentials handed out: (access key, secret, token).
    sessions: Mutex<Vec<(String, String, String)>>,
    /// Called with each new session so a test can register it with the Bedrock mock.
    on_session: Mutex<Option<SessionHook>>,
}

#[derive(Clone)]
pub struct MockSts {
    st: Arc<Shared>,
    base: String,
}

fn form(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| {
            (
                k.to_owned(),
                percent_encoding::percent_decode_str(v)
                    .decode_utf8_lossy()
                    .into_owned(),
            )
        })
        .collect()
}

fn xml(status: StatusCode, body: String) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/xml")
        .body(Body::from(body))
        .unwrap()
}

fn error(status: StatusCode, code: &str) -> Response {
    xml(
        status,
        format!(
            "<ErrorResponse><Error><Type>Sender</Type><Code>{code}</Code><Message>{code}</Message></Error></ErrorResponse>"
        ),
    )
}

async fn handle(State(st): State<Arc<Shared>>, req: Request) -> Response {
    *st.calls.lock().unwrap() += 1;
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20)
        .await
        .unwrap_or_default();
    let headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_owned()))
        .collect();
    let verified = aws::verify(
        parts.method.as_str(),
        parts.uri.path(),
        parts.uri.query(),
        &headers,
        &bytes,
        |ak| (ak == st.source_key).then(|| (st.source_secret.clone(), None)),
    );
    let signer = match verified {
        Ok(v) if v.service == "sts" => v.access_key,
        Ok(_) => return error(StatusCode::FORBIDDEN, "SignatureDoesNotMatch"),
        Err(Refusal::UnknownKey(_) | Refusal::Unsigned | Refusal::BadToken) => {
            return error(StatusCode::FORBIDDEN, "InvalidClientTokenId");
        }
        Err(Refusal::BadSignature) => {
            return error(StatusCode::FORBIDDEN, "SignatureDoesNotMatch");
        }
    };
    let f = form(&String::from_utf8_lossy(&bytes));
    let get = |k: &str| {
        f.iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    match get("Action").as_str() {
        "GetCallerIdentity" => xml(
            StatusCode::OK,
            format!(
                "<GetCallerIdentityResponse><GetCallerIdentityResult><Arn>arn:aws:iam::{0}:user/x</Arn><UserId>AIDA</UserId><Account>{0}</Account></GetCallerIdentityResult></GetCallerIdentityResponse>",
                st.account
            ),
        ),
        "AssumeRole" => {
            st.assumed.lock().unwrap().push(AssumeSeen {
                signed_by: signer,
                role_arn: get("RoleArn"),
                role_session_name: get("RoleSessionName"),
                external_id: get("ExternalId"),
                duration_seconds: get("DurationSeconds"),
            });
            if *st.deny.lock().unwrap() {
                return error(StatusCode::FORBIDDEN, "AccessDenied");
            }
            let n = {
                let mut c = st.counter.lock().unwrap();
                *c += 1;
                *c
            };
            let (ak, sk, tok) = (
                format!("ASIAMOCKSESSION{n:05}"),
                format!("session-secret-{n}"),
                format!("session-token-{n}"),
            );
            st.sessions
                .lock()
                .unwrap()
                .push((ak.clone(), sk.clone(), tok.clone()));
            if let Some(cb) = st.on_session.lock().unwrap().as_ref() {
                cb(&ak, &sk, &tok);
            }
            let role = get("RoleArn");
            let name = role.rsplit('/').next().unwrap_or("role").to_owned();
            xml(
                StatusCode::OK,
                format!(
                    "<AssumeRoleResponse><AssumeRoleResult><Credentials><AccessKeyId>{ak}</AccessKeyId><SecretAccessKey>{sk}</SecretAccessKey><SessionToken>{tok}</SessionToken><Expiration>2099-01-01T00:00:00Z</Expiration></Credentials><AssumedRoleUser><AssumedRoleId>AROAMOCK:{}</AssumedRoleId><Arn>arn:aws:sts::{}:assumed-role/{name}/{}</Arn></AssumedRoleUser></AssumeRoleResult></AssumeRoleResponse>",
                    get("RoleSessionName"),
                    st.account,
                    get("RoleSessionName"),
                ),
            )
        }
        _ => error(StatusCode::BAD_REQUEST, "InvalidAction"),
    }
}

impl MockSts {
    /// An STS that knows one source key; roles it assumes (and keys it reports) belong to `account`.
    pub async fn start(source_key: &str, source_secret: &str, account: &str) -> Self {
        let st = Arc::new(Shared {
            source_key: source_key.to_owned(),
            source_secret: source_secret.to_owned(),
            account: account.to_owned(),
            deny: Mutex::new(false),
            assumed: Mutex::new(vec![]),
            calls: Mutex::new(0),
            counter: Mutex::new(0),
            sessions: Mutex::new(vec![]),
            on_session: Mutex::new(None),
        });
        let app = Router::new().fallback(handle).with_state(st.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(l, app).await;
        });
        Self { st, base }
    }

    /// `http://127.0.0.1:PORT`: the override target of `https://sts.<region>.amazonaws.com`.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Answer `AssumeRole` with `AccessDenied` (the role's trust policy refuses the source key or external id).
    pub fn deny_assume(&self, on: bool) {
        *self.st.deny.lock().unwrap() = on;
    }

    /// Run `f(access_key, secret, token)` for every session handed out (register it with the Bedrock mock).
    pub fn on_session(&self, f: impl Fn(&str, &str, &str) + Send + 'static) {
        *self.st.on_session.lock().unwrap() = Some(Box::new(f));
    }

    pub fn assume_calls(&self) -> Vec<AssumeSeen> {
        self.st.assumed.lock().unwrap().clone()
    }

    /// Every request, signed or not.
    pub fn calls(&self) -> usize {
        *self.st.calls.lock().unwrap()
    }

    /// The sessions handed out so far: (access key id, secret, token).
    pub fn sessions(&self) -> Vec<(String, String, String)> {
        self.st.sessions.lock().unwrap().clone()
    }
}
