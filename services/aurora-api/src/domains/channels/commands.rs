//! Platform-neutral commands (docs/13 §5.4): `help`, `agents`, `use`, `link`, `unlink`, `bind`,
//! `unbind`, `approve`, `deny`. Replies are private (ephemeral where the platform has it).
//! Every command resolves the caller through their user link, so the platform never
//! authorizes (§5.5); `approve`/`deny` need `can_approve` (phase 2, RFE-012) on the approval's workspace.

use super::inbox::send::{self, Ctx};
use super::inbox::{Retry, approval_code_matches, ignored, outcome};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::connectors::inbox::InboxError;
use aurora_core::sanitize;
use aurora_db::authz;
use aurora_db::repo::channel_inbox::InboxRow;
use aurora_db::repo::channel_presences::{self, PresenceAgent};
use aurora_db::repo::{
    approvals, dm_defaults, reach_grants, space_bindings, thread_links, user_links,
};
use aurora_protocol::connector::{CommandInvoked, Sender};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

pub const COMMANDS: [&str; 9] = [
    "help", "agents", "use", "link", "unlink", "bind", "unbind", "approve", "deny",
];

struct Cmd<'a> {
    state: &'a AppState,
    row: &'a InboxRow,
    ctx: &'a Ctx,
    c: &'a CommandInvoked,
    sender: Sender,
}

pub async fn handle(
    state: &AppState,
    row: &InboxRow,
    ctx: &Ctx,
    c: CommandInvoked,
) -> Result<Value, InboxError> {
    let all = channel_presences::active_with_agents(&state.pool, ctx.id())
        .await
        .retry()?;
    // Replies come from the shared bot, else from the first active dedicated bot.
    let sender = if ctx.shared_ref().is_some() {
        Sender::shared()
    } else if let Some(p) = all.first() {
        Sender::Presence {
            presence_id: p.presence_id,
        }
    } else {
        return Ok(ignored("no_sender"));
    };
    let cmd = Cmd {
        state,
        row,
        ctx,
        c: &c,
        sender,
    };
    let name = c.name.trim().trim_start_matches('/').to_lowercase();
    let args = c.args.trim();
    match name.as_str() {
        "help" => cmd.help().await,
        "agents" => cmd.agents(&all).await,
        "use" => cmd.use_agent(&all, args).await,
        "link" => cmd.link().await,
        "unlink" => cmd.unlink().await,
        "bind" => cmd.bind(&all, args).await,
        "unbind" => cmd.unbind().await,
        "approve" => cmd.decide(args, true).await,
        "deny" => cmd.decide(args, false).await,
        _ => {
            cmd.reply("I don't know that command. Try `help`.").await?;
            Ok(outcome("unknown_command"))
        }
    }
}

fn handle_of(args: &str) -> String {
    args.trim().trim_start_matches('@').to_lowercase()
}

impl Cmd<'_> {
    fn thread(&self) -> &str {
        self.ctx
            .caps
            .thread_ref(self.c.thread_ref.as_deref().unwrap_or(""))
    }

    async fn conn(&self) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, InboxError> {
        self.state.pool.acquire().await.retry()
    }

    async fn reply(&self, text: &str) -> Result<(), InboxError> {
        let mut conn = self.conn().await?;
        send::private(
            &mut conn,
            self.ctx,
            &self.sender,
            &self.c.space.reference,
            self.thread(),
            &self.c.user_ref,
            text,
            format!("cmd:{}", self.row.event_id),
        )
        .await
        .retry()?;
        Ok(())
    }

    /// The caller's Aurora user, or a link prompt (and `None`).
    async fn user(&self) -> Result<Option<Uuid>, InboxError> {
        if let Some(l) =
            user_links::user_link_for(&self.state.pool, self.ctx.id(), &self.c.user_ref)
                .await
                .retry()?
        {
            return Ok(Some(l.user_id));
        }
        let mut conn = self.conn().await?;
        link_prompt(self, &mut conn).await?;
        Ok(None)
    }

    async fn help(&self) -> Result<Value, InboxError> {
        self.reply(
            "Commands: `help`, `agents` (agents you can reach here), `use <handle>` (default agent for your DMs), \
             `link` / `unlink` (your Aurora account), `bind <handle>` / `unbind` (agent of this space, workspace owners), \
             `approve <code>` / `deny <code>` (pending approvals).",
        )
        .await?;
        Ok(outcome("help"))
    }

    async fn reachable<'a>(
        &self,
        user: Uuid,
        all: &'a [PresenceAgent],
    ) -> Result<Vec<&'a PresenceAgent>, InboxError> {
        let pool = &self.state.pool;
        let mut out: Vec<&PresenceAgent> = Vec::new();
        for p in all {
            if out
                .iter()
                .any(|o| o.agent_id == p.agent_id && o.mode == p.mode)
            {
                continue;
            }
            let ok = authz::can_use(pool, user, p.workspace_id).await.retry()?
                || reach_grants::approved_person_reach(
                    pool,
                    p.agent_id,
                    self.ctx.id(),
                    std::slice::from_ref(&self.c.user_ref),
                )
                .await
                .retry()?;
            if ok {
                out.push(p);
            }
        }
        Ok(out)
    }

    async fn agents(&self, all: &[PresenceAgent]) -> Result<Value, InboxError> {
        let Some(user) = self.user().await? else {
            return Ok(outcome("link_prompt"));
        };
        let list = self.reachable(user, all).await?;
        if list.is_empty() {
            self.reply("No agents are available to you here.").await?;
        } else {
            let lines: Vec<String> = list
                .iter()
                .map(|p| {
                    format!(
                        "- `{}` ({})",
                        sanitize::label(&p.agent_handle),
                        sanitize::label(&p.agent_name)
                    )
                })
                .collect();
            self.reply(&format!("Agents you can reach here:\n{}", lines.join("\n")))
                .await?;
        }
        Ok(outcome("agents"))
    }

    async fn use_agent(&self, all: &[PresenceAgent], args: &str) -> Result<Value, InboxError> {
        let Some(user) = self.user().await? else {
            return Ok(outcome("link_prompt"));
        };
        let want = handle_of(args);
        let list = self.reachable(user, all).await?;
        let found = list
            .iter()
            .find(|p| p.mode == "shared" && p.agent_handle.to_lowercase() == want);
        let Some(p) = found.filter(|_| !want.is_empty()) else {
            self.reply("I can't find an agent with that handle that you can reach. Try `agents`.")
                .await?;
            return Ok(outcome("use_not_found"));
        };
        dm_defaults::upsert(
            &self.state.pool,
            self.ctx.id(),
            &self.c.user_ref,
            p.agent_id,
        )
        .await
        .retry()?;
        self.reply(&format!(
            "Done: your direct messages now go to `{}`.",
            sanitize::label(&p.agent_handle)
        ))
        .await?;
        Ok(outcome("use"))
    }

    async fn link(&self) -> Result<Value, InboxError> {
        if user_links::user_link_for(&self.state.pool, self.ctx.id(), &self.c.user_ref)
            .await
            .retry()?
            .is_some()
        {
            self.reply("Your account is already linked.").await?;
            return Ok(outcome("already_linked"));
        }
        let mut conn = self.conn().await?;
        link_prompt(self, &mut conn).await?;
        Ok(outcome("link_prompt"))
    }

    async fn unlink(&self) -> Result<Value, InboxError> {
        let pool = &self.state.pool;
        let Some(l) = user_links::user_link_for(pool, self.ctx.id(), &self.c.user_ref)
            .await
            .retry()?
        else {
            self.reply("Your account isn't linked.").await?;
            return Ok(outcome("not_linked"));
        };
        let mut tx = pool
            .begin()
            .await
            .map_err(aurora_db::DbError::from)
            .retry()?;
        user_links::delete(&mut *tx, l.id).await.retry()?;
        dm_defaults::delete(&mut *tx, l.integration_id, &l.platform_user_ref)
            .await
            .retry()?;
        tx.commit()
            .await
            .map_err(aurora_db::DbError::from)
            .retry()?;
        audit::record(
            self.state,
            AuditEvent::new(
                AuditActor::system(self.ctx.integ.connector_id.clone()).with_source("channel"),
                "user_link.delete",
                Outcome::Success,
            )
            .org(l.org_id)
            .target("user_link", l.id)
            .metadata(
                json!({"integration_id": l.integration_id, "user_id": l.user_id, "via": "command"}),
            ),
        );
        self.reply("Unlinked. You'll be asked to link again next time you write to an agent.")
            .await?;
        Ok(outcome("unlinked"))
    }

    async fn bind(&self, all: &[PresenceAgent], args: &str) -> Result<Value, InboxError> {
        let pool = &self.state.pool;
        if self.c.space.kind.as_str() == "dm" {
            self.reply("Bindings are for channels. Use `use <handle>` to choose your DM agent.")
                .await?;
            return Ok(outcome("bind_in_dm"));
        }
        let Some(user) = self.user().await? else {
            return Ok(outcome("link_prompt"));
        };
        let want = handle_of(args);
        let mut candidates = Vec::new();
        for p in all.iter().filter(|p| p.mode == "shared") {
            if p.agent_handle.to_lowercase() == want
                && authz::can_own(pool, user, p.workspace_id).await.retry()?
            {
                candidates.push(p);
            }
        }
        let Some(p) = candidates.first().filter(|_| candidates.len() == 1) else {
            self.reply("I can't find exactly one agent with that handle that you own and that is served by the shared bot.")
                .await?;
            return Ok(outcome("bind_not_found"));
        };
        let res = space_bindings::insert(
            pool,
            &space_bindings::NewBinding {
                id: Uuid::now_v7(),
                integration_id: self.ctx.id(),
                space_ref: &self.c.space.reference,
                space_label: &sanitize::label(&self.c.space.label),
                agent_id: p.agent_id,
                listen: "mentions",
                bound_by_user_id: Some(user),
            },
        )
        .await;
        match res {
            Ok(()) => {}
            Err(aurora_db::DbError::UniqueViolation { .. }) => {
                self.reply("This space already has an agent. Use `unbind` first.")
                    .await?;
                return Ok(outcome("already_bound"));
            }
            Err(e) => return Err(InboxError::Retry(e.to_string())),
        }
        audit::record(
            self.state,
            AuditEvent::new(
                AuditActor::system(self.ctx.integ.connector_id.clone()).with_source("channel"),
                "space_binding.create",
                Outcome::Success,
            )
            .org(self.ctx.integ.org_id)
            .workspace(p.workspace_id)
            .target("space_binding", p.agent_id)
            .metadata(json!({"integration_id": self.ctx.id(), "space_ref": self.c.space.reference, "user_id": user})),
        );
        let mut conn = self.conn().await?;
        send::say(
            &mut conn,
            self.ctx,
            &Sender::shared(),
            &self.c.space.reference,
            "",
            &format!(
                "{} is now answering in this space. Mention me to talk.",
                sanitize::label(&p.agent_name)
            ),
            format!("bound:{}", self.row.event_id),
        )
        .await
        .retry()?;
        Ok(outcome("bound"))
    }

    async fn unbind(&self) -> Result<Value, InboxError> {
        let pool = &self.state.pool;
        let Some(user) = self.user().await? else {
            return Ok(outcome("link_prompt"));
        };
        let Some(b) = space_bindings::active_binding(pool, self.ctx.id(), &self.c.space.reference)
            .await
            .retry()?
        else {
            self.reply("This space has no agent bound.").await?;
            return Ok(outcome("not_bound"));
        };
        let ws = aurora_db::repo::agents::get(pool, b.agent_id)
            .await
            .retry()?
            .map(|a| a.workspace_id);
        let owns = match ws {
            Some(w) => authz::can_own(pool, user, w).await.retry()?,
            None => false,
        };
        if !owns {
            self.reply("Only an owner of the agent's workspace can unbind it.")
                .await?;
            return Ok(outcome("unbind_forbidden"));
        }
        space_bindings::deactivate(pool, b.id).await.retry()?;
        self.reply("Unbound. Re-binding needs an owner again.")
            .await?;
        Ok(outcome("unbound"))
    }

    /// `approve <code>` / `deny <code>`: the code is `cards::approval_code` (S-12: base32 of the
    /// first 40 bits of sha256(id)); only approvals of conversations that live in this space are candidates,
    /// and the caller needs a user link and `can_approve` (the access law; phase 2, RFE-012).
    async fn decide(&self, args: &str, approve: bool) -> Result<Value, InboxError> {
        let pool = &self.state.pool;
        let Some(user) = self.user().await? else {
            return Ok(outcome("link_prompt"));
        };
        let code = args.trim();
        if code.is_empty() {
            self.reply("Which approval? Use `approve <code>` or `deny <code>`.")
                .await?;
            return Ok(outcome("code_missing"));
        }
        let mut matches = Vec::new();
        for a in approvals::list_pending_for(pool, user, None, None, 200)
            .await
            .retry()?
        {
            let Some(conv) = a.conversation_id else {
                continue;
            };
            if approval_code_matches(a.id, code)
                && thread_links::exists_in_space(pool, conv, self.ctx.id(), &self.c.space.reference)
                    .await
                    .retry()?
            {
                matches.push(a);
            }
        }
        if matches.is_empty() {
            // Phase 2, RFE-012: a plain member who can see the approval is told why nothing happened.
            for a in approvals::list_pending_undecidable_for(pool, user, 200)
                .await
                .retry()?
            {
                let Some(conv) = a.conversation_id else {
                    continue;
                };
                if approval_code_matches(a.id, code)
                    && thread_links::exists_in_space(
                        pool,
                        conv,
                        self.ctx.id(),
                        &self.c.space.reference,
                    )
                    .await
                    .retry()?
                {
                    self.reply("Only approvers can decide this.").await?;
                    return Ok(outcome("approver_required"));
                }
            }
        }
        let [a] = matches.as_slice() else {
            let msg = if matches.is_empty() {
                "I found no pending approval with that code that you may decide here."
            } else {
                "That code matches several approvals; please decide them in the dashboard."
            };
            self.reply(msg).await?;
            return Ok(outcome(if matches.is_empty() {
                "approval_not_found"
            } else {
                "approval_ambiguous"
            }));
        };
        let done = approvals::decide(pool, a.id, approve, user, "channel")
            .await
            .retry()?;
        let Some(done) = done else {
            self.reply("That approval has already been resolved.")
                .await?;
            return Ok(outcome("already_decided"));
        };
        audit::record(
            self.state,
            AuditEvent::new(
                AuditActor::system(self.ctx.integ.connector_id.clone()).with_source("channel"),
                "approval.decide",
                Outcome::Success,
            )
            .org(done.org_id)
            .workspace(done.workspace_id)
            .target("approval", done.id)
            .metadata(json!({"decision": if approve {"approved"} else {"denied"}, "via": "channel", "user_id": user})),
        );
        self.reply(if approve { "Approved." } else { "Denied." })
            .await?;
        Ok(outcome(if approve { "approved" } else { "denied" }))
    }
}

async fn link_prompt(cmd: &Cmd<'_>, conn: &mut PgConnection) -> Result<(), InboxError> {
    super::inbox::send_link_prompt(
        cmd.state,
        cmd.ctx,
        conn,
        &cmd.sender,
        &cmd.c.space.reference,
        cmd.thread(),
        &cmd.c.user_ref,
        None,
        format!("link:{}", cmd.row.event_id),
    )
    .await
}
