//! (P5-REFINE-8, D-417, C-1080) The one rule for the level that resolves a request. Feeds, lists,
//! declines and counts all call [`of`]; nothing stores the result.
//!
//! - `app`: `administration` when what the app still needs (the request's `needs`/`unavailable`, kept
//!   current by `reevaluate` on every readiness change and every 10 minutes) can be fixed only by an
//!   instance admin; else `organization`. [`need_fixer`] is the table: today every need of C-1029 is
//!   fixed by an organization owner or admin (an organization-level sign-in app, an organization-level
//!   server, admin material, switching the app on), the same role `setup.rs` puts in `who_can`.
//! - `chat_platform`: `administration` when no connector of the asked type (any, for none) is `active`
//!   on the instance (only an instance admin can add one); else `organization`.
//! - `ai_key`: `organization`.
//!
//! A declined request stays at the level that declined it (`closed_by_role`), so its history does not
//! move when the facts change later.

use super::dto::{ClosedByRole, RequestLevel};
use crate::domains::apps::setup::SetupNeed;
use crate::http::who_can::WhoCanRole;
use aurora_db::repo::app_setup_requests::RequestRow;

/// Who can fix one need of C-1029 (mirrors the `who_can` that `apps::setup::Facts::methods` builds).
pub fn need_fixer(n: SetupNeed) -> WhoCanRole {
    match n {
        // An organization-level sign-in app (`register_org_client`), an organization-level server
        // (`register_instance` on the organization's page) and admin material are the organization's.
        SetupNeed::OauthClient | SetupNeed::Instance | SetupNeed::AdminMaterial => {
            WhoCanRole::OrgAdmin
        }
    }
}

/// The role that switches on an app the organization switched off.
pub const UNAVAILABLE_FIXER: WhoCanRole = WhoCanRole::OrgAdmin;

fn level_of_role(r: WhoCanRole) -> RequestLevel {
    if r == WhoCanRole::InstanceAdmin {
        RequestLevel::Administration
    } else {
        RequestLevel::Organization
    }
}

/// The level of an app with these needs: the first fixer decides, as `readiness` picks the first
/// blocking method's `who_can`.
pub fn of_app(needs: &[SetupNeed], unavailable: bool) -> RequestLevel {
    if unavailable {
        return level_of_role(UNAVAILABLE_FIXER);
    }
    needs.first().map_or(RequestLevel::Organization, |n| {
        level_of_role(need_fixer(*n))
    })
}

/// The level of a request (see the module).
pub fn of_parts(
    subject: &str,
    needs: &[SetupNeed],
    unavailable: bool,
    connector_active: bool,
    closed_by_role: Option<ClosedByRole>,
) -> RequestLevel {
    match closed_by_role {
        Some(ClosedByRole::InstanceAdmin) => return RequestLevel::Administration,
        Some(ClosedByRole::OrgAdmin) => return RequestLevel::Organization,
        _ => {}
    }
    match subject {
        "chat_platform" if !connector_active => RequestLevel::Administration,
        "chat_platform" | "ai_key" => RequestLevel::Organization,
        _ => of_app(needs, unavailable),
    }
}

pub fn of(r: &RequestRow) -> RequestLevel {
    of_parts(
        &r.subject,
        &super::service::needs_of(&r.needs),
        r.unavailable,
        r.connector_active,
        r.closed_by_role.as_deref().and_then(ClosedByRole::parse),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_rule() {
        use RequestLevel::{Administration as A, Organization as O};
        // Apps: every need of C-1029 is the organization's today.
        for n in [
            SetupNeed::OauthClient,
            SetupNeed::Instance,
            SetupNeed::AdminMaterial,
        ] {
            assert_eq!(of_parts("app", &[n], false, false, None), O);
        }
        assert_eq!(of_parts("app", &[], true, false, None), O);
        // Chat platforms: only an instance admin can add a connector.
        assert_eq!(of_parts("chat_platform", &[], false, false, None), A);
        assert_eq!(of_parts("chat_platform", &[], false, true, None), O);
        assert_eq!(of_parts("ai_key", &[], false, false, None), O);
        // A decline keeps the level that declined it.
        assert_eq!(
            of_parts(
                "chat_platform",
                &[],
                false,
                true,
                Some(ClosedByRole::InstanceAdmin)
            ),
            A
        );
        assert_eq!(
            of_parts(
                "chat_platform",
                &[],
                false,
                false,
                Some(ClosedByRole::OrgAdmin)
            ),
            O
        );
        // A resolution does not pin the level.
        assert_eq!(
            of_parts(
                "chat_platform",
                &[],
                false,
                false,
                Some(ClosedByRole::System)
            ),
            A
        );
    }

    #[test]
    fn an_instance_only_fixer_moves_an_app_to_administration() {
        assert_eq!(
            level_of_role(WhoCanRole::InstanceAdmin),
            RequestLevel::Administration
        );
        assert_eq!(
            level_of_role(WhoCanRole::OrgOwner),
            RequestLevel::Organization
        );
    }
}
