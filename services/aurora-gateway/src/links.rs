//! (FIX-153, C-891) The one place that builds the dashboard URLs the gateway puts in refusals.
//! Every path must be a real dashboard route; `tests/conformance/dashboard-links.json` lists the
//! templates and a web test checks each one against `web/src/app`.

use aurora_core::dash_paths;
use uuid::Uuid;

/// Dashboard links for one resolved workspace.
#[derive(Debug, Clone, Copy)]
pub struct Links<'a> {
    external_url: &'a str,
    org: &'a str,
    ws: &'a str,
}

/// Percent-encodes everything outside the RFC 3986 unreserved set (ids are `[a-z0-9-]` today;
/// encoded anyway).
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl<'a> Links<'a> {
    pub fn new(external_url: &'a str, org: &'a str, ws: &'a str) -> Self {
        Self {
            external_url,
            org,
            ws,
        }
    }

    /// Joins a path of the shared table (`aurora_core::dash_paths`, C-960) to the external URL. The
    /// slugs and ids are percent-encoded before they reach the table's functions.
    fn url(&self, path: String) -> String {
        format!("{}{path}", self.external_url)
    }

    fn org(&self) -> String {
        enc(self.org)
    }

    fn ws(&self) -> String {
        enc(self.ws)
    }

    /// The journey's deep link for one agent and app (C-959): the agent's Access tab with the
    /// connect journey open. `connect_url` for refusals that know the agent.
    pub fn connect(&self, agent: Uuid, app: &str) -> String {
        self.url(dash_paths::ws_agent_connect(
            &self.org(),
            &self.ws(),
            &agent.to_string(),
            &enc(app),
        ))
    }

    /// The workspace-level connect link (C-959 `connect_url_workspace`, C-842).
    pub fn connect_workspace(&self, app: &str) -> String {
        self.url(dash_paths::ws_connect(&self.org(), &self.ws(), &enc(app)))
    }

    /// The Apps tab, no app chosen.
    pub fn connect_any(&self) -> String {
        self.url(dash_paths::ws_apps(&self.org(), &self.ws(), "apps"))
    }

    pub fn reconnect(&self, connection: Uuid) -> String {
        self.url(dash_paths::ws_reconnect(
            &self.org(),
            &self.ws(),
            &connection.to_string(),
        ))
    }

    /// Where a website's secret is added: the workspace's accounts list.
    pub fn secrets(&self) -> String {
        self.url(dash_paths::ws_accounts(&self.org(), &self.ws()))
    }

    /// The agent's Access tab (the agent page accepts an id, C-894).
    pub fn access(&self, agent: Uuid) -> String {
        self.url(dash_paths::ws_agent_access(
            &self.org(),
            &self.ws(),
            &agent.to_string(),
        ))
    }

    /// The workspace's Internet access page.
    pub fn policy(&self) -> String {
        self.url(dash_paths::ws_internet_access(&self.org(), &self.ws()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/conformance/dashboard-links.json"
    ));

    #[test]
    fn builder_output_equals_the_conformance_fixture() {
        let v: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture json");
        let t = &v["templates"];
        let conn = Uuid::parse_str("00000000-0000-0000-0000-0000000000c1").unwrap();
        let agent = Uuid::parse_str("00000000-0000-0000-0000-0000000000a1").unwrap();
        let l = Links::new("https://aurora.test", "acme", "main");
        let expand = |tpl: &str| {
            tpl.replace("{external}", "https://aurora.test")
                .replace("{org}", "acme")
                .replace("{ws}", "main")
                .replace("{app}", "cern-phonebook")
                .replace("{connection}", &conn.to_string())
                .replace("{agent}", &agent.to_string())
        };
        let get = |k: &str| expand(t[k].as_str().unwrap_or_else(|| panic!("template {k}")));
        assert_eq!(l.connect(agent, "cern-phonebook"), get("connect"));
        assert_eq!(
            l.connect_workspace("cern-phonebook"),
            get("connect_workspace")
        );
        assert_eq!(l.connect_any(), get("connect_any"));
        assert_eq!(l.reconnect(conn), get("reconnect"));
        assert_eq!(l.secrets(), get("secrets"));
        assert_eq!(l.access(agent), get("access"));
        assert_eq!(l.policy(), get("policy"));
    }

    #[test]
    fn ids_are_percent_encoded() {
        let l = Links::new("https://a", "o", "w");
        assert_eq!(
            l.connect_workspace("a b&c"),
            "https://a/o/o/w/w/apps?tab=apps&connect=a%20b%26c"
        );
    }
}
