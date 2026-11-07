//! Absolute-form `http://` requests.

use super::{Proxy, Target};
use crate::guard::normalize_host;
use crate::pipeline::Scheme;
use crate::{Body, guidance};
use http::{Request, Response};
use hyper::body::Incoming;

impl Proxy {
    pub(super) async fn handle_absolute(
        &self,
        req: Request<Incoming>,
        client: std::net::IpAddr,
    ) -> Response<Body> {
        let uri = req.uri();
        let (Some(auth), Some("http")) = (uri.authority(), uri.scheme_str()) else {
            return guidance::response(&guidance::not_a_proxy_request());
        };
        let host = normalize_host(auth.host());
        let port = auth.port_u16().unwrap_or(80);
        let (identity, lane) = match self.authenticate(req.headers()).await {
            Ok(x) => x,
            Err(e) => return guidance::response(&e),
        };
        // (RFE-025) BYO admission applies to absolute-form requests too.
        if let Err(e) = self.byo_gate(&identity, client, req.method().as_str(), &host) {
            return guidance::response(&e);
        }
        let t = Target {
            identity,
            lane,
            scheme: Scheme::Http,
            host,
            port,
            kill: None,
        };
        self.process(&t, req).await
    }
}
