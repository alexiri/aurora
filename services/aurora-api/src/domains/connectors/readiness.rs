//! (P5-REFINE-6, C-1068, D-407) One readiness for a chat platform. `GET /v1/connector-availability`
//! (the organization's Chat platforms) and `GET /v1/admin/connectors` (Administration) both read
//! a connector's state through [`connector_readiness`], from the same facts, so the two pages
//! cannot disagree (UX review 5, org-1 and admin-5).

use super::dto::{ConnectorNextStep, ConnectorSetupState};
use super::managed::CatalogType;
use aurora_db::repo::connectors::ConnectorRow;

/// What the platform knows about a connector besides its row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Facts {
    /// A live connector host announces the connector's type.
    pub announced: bool,
    /// A live host reports this connector's instance as running.
    pub host_reports_running: bool,
    /// The connector has a live (recently seen, not retired) replica.
    pub live_replica: bool,
}

/// Where a connector stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Readiness {
    pub setup_state: ConnectorSetupState,
    /// Something runs it: a live host announces its type or reports it running, or (a self-hosted
    /// connector) it has a live replica.
    pub host_running: bool,
    /// Organization owners and admins can connect it now (`setup_state = active`).
    pub can_connect: bool,
}

/// Who reads the next step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Viewer {
    /// The Administration page (instance admins): once usable, an organization owner connects it.
    Administration,
    /// An organization owner or admin on the organization's Chat platforms page.
    OrgAdmin,
    /// Anyone else (a member): an organization owner connects it.
    Other,
}

/// Does a live host announce this row's type? (`id` or `type_id` equal to an announced type.)
#[must_use]
pub fn announced(row: &ConnectorRow, types: &[CatalogType]) -> bool {
    types
        .iter()
        .any(|t| t.type_id == row.id || row.type_id.as_deref() == Some(&t.type_id))
}

/// The readiness of a connector (`None`: not added, only for an announced type).
///
/// `awaiting_registration` is `registering` while a live host announces the type (its first
/// registration is pending and resolves within one host poll) and `waiting_for_host` only when no
/// live host announces it.
#[must_use]
pub fn connector_readiness(row: Option<&ConnectorRow>, f: Facts) -> Readiness {
    let setup_state = match row.map(|r| r.status.as_str()) {
        None => ConnectorSetupState::NotAdded,
        Some("active") => ConnectorSetupState::Active,
        Some("disabled") => ConnectorSetupState::Disabled,
        Some("pending_approval") => ConnectorSetupState::NeedsApproval,
        Some(_) if f.announced => ConnectorSetupState::Registering,
        Some(_) => ConnectorSetupState::WaitingForHost,
    };
    Readiness {
        setup_state,
        host_running: f.announced || f.host_reports_running || f.live_replica,
        can_connect: setup_state == ConnectorSetupState::Active,
    }
}

/// The next thing a person does (C-1068). A `registering` connector becomes `active` within one
/// host poll, so its next step is already the one of `active`.
#[must_use]
pub fn next_step(state: ConnectorSetupState, viewer: Viewer) -> ConnectorNextStep {
    match state {
        ConnectorSetupState::NotAdded => ConnectorNextStep::Add,
        ConnectorSetupState::Disabled => ConnectorNextStep::Enable,
        ConnectorSetupState::NeedsApproval => ConnectorNextStep::Approve,
        ConnectorSetupState::WaitingForHost => ConnectorNextStep::StartHost,
        ConnectorSetupState::Active | ConnectorSetupState::Registering => match viewer {
            Viewer::OrgAdmin => ConnectorNextStep::Connect,
            Viewer::Administration | Viewer::Other => ConnectorNextStep::OwnerConnects,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ConnectorSetupState as S;

    fn row(status: &str) -> ConnectorRow {
        ConnectorRow {
            id: "mattermost".into(),
            display_name: "Mattermost".into(),
            version: None,
            contract: None,
            manifest: None,
            manifest_digest: None,
            approved_manifest_digest: None,
            status: status.to_owned(),
            approved_by: None,
            approved_at: None,
            created_at: chrono::Utc::now(),
            managed: true,
            type_id: Some("mattermost".into()),
            settings: None,
            secret_owner: uuid::Uuid::nil(),
            secret_refs: serde_json::json!({}),
            secret_updated_at: serde_json::json!({}),
        }
    }

    #[test]
    fn every_state_and_its_next_step() {
        let live = Facts {
            announced: true,
            ..Facts::default()
        };
        let none = Facts::default();
        let cases = [
            (None, live, S::NotAdded, true),
            (Some("disabled"), live, S::Disabled, true),
            (Some("pending_approval"), live, S::NeedsApproval, true),
            (Some("awaiting_registration"), live, S::Registering, true),
            (Some("active"), live, S::Active, true),
            (Some("awaiting_registration"), none, S::WaitingForHost, false),
            (Some("active"), none, S::Active, false),
        ];
        for (status, f, want, running) in cases {
            let r = status.map(row);
            let got = connector_readiness(r.as_ref(), f);
            assert_eq!(got.setup_state, want, "{status:?}");
            assert_eq!(got.host_running, running, "{status:?}");
            assert_eq!(got.can_connect, want == S::Active, "{status:?}");
        }
        // A host report or a live replica also counts as running.
        let r = row("active");
        for f in [
            Facts {
                host_reports_running: true,
                ..Facts::default()
            },
            Facts {
                live_replica: true,
                ..Facts::default()
            },
        ] {
            assert!(connector_readiness(Some(&r), f).host_running);
        }
        assert_eq!(
            next_step(S::Active, Viewer::OrgAdmin),
            ConnectorNextStep::Connect
        );
        assert_eq!(
            next_step(S::Active, Viewer::Administration),
            ConnectorNextStep::OwnerConnects
        );
        assert_eq!(
            next_step(S::Registering, Viewer::Other),
            ConnectorNextStep::OwnerConnects
        );
        assert_eq!(
            next_step(S::WaitingForHost, Viewer::OrgAdmin),
            ConnectorNextStep::StartHost
        );
        assert_eq!(
            next_step(S::NotAdded, Viewer::Administration),
            ConnectorNextStep::Add
        );
        assert_eq!(
            next_step(S::NeedsApproval, Viewer::OrgAdmin),
            ConnectorNextStep::Approve
        );
        assert_eq!(
            next_step(S::Disabled, Viewer::OrgAdmin),
            ConnectorNextStep::Enable
        );
    }
}
