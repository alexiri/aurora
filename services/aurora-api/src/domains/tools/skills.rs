//! Skill tools (docs/12 Phase 0 catalogue): `skill_list/get/create/update/delete`. Agent tier
//! only: an agent edits its own skills; organization skills are readable, never writable.
use super::registry::{PlatformTool, ToolCallContext, ToolContext, ToolError};
use crate::domains::skills::app_skills::{self, AppSkill};
use crate::domains::skills::service::Owner;
use crate::domains::skills::{NewSkill, Patch, SkillView, service};
use aurora_core::{ApiError, limits};
use aurora_db::repo::agents;
use aurora_protocol::tools::{JsonContent, ToolContent};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn tools() -> Vec<Box<dyn PlatformTool>> {
    vec![
        Box::new(List),
        Box::new(Get),
        Box::new(Create),
        Box::new(Update),
        Box::new(Delete),
    ]
}

fn out(value: Value) -> Result<Vec<ToolContent>, ToolError> {
    Ok(vec![ToolContent::Json(JsonContent { value })])
}

fn parse<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))
}

fn fail(e: ApiError) -> ToolError {
    if e.status().is_client_error() {
        ToolError::Failed(e.message().to_owned())
    } else {
        tracing::error!(error = %e.message(), "skill tool error");
        ToolError::Failed("internal error".into())
    }
}

/// (C-1133) An app skill as `skill_list` and `skill_get` show it: tier `app`, never editable.
fn app_summary(s: &AppSkill) -> Value {
    json!({"name": s.skill.name, "title": Value::Null, "tier": "app", "scope": "app",
        "app_id": s.app_id,
        "description": s.skill.description, "enabled": true,
        "files": s.skill.files.iter().map(|f| json!({"path": f.path, "size": f.size,
            "executable": f.executable})).collect::<Vec<_>>(),
        "total_size": s.skill.size + s.skill.files.iter().map(|f| f.size).sum::<u64>(),
        "source": Value::Null, "shadows": json!([])})
}

fn summary(v: &SkillView) -> Value {
    let tier = service::tier_of(&v.row);
    json!({"name": v.row.name, "title": v.row.title, "tier": tier,
        "scope": if tier == "org" { "organization" } else { tier },
        "description": v.row.description, "enabled": v.row.enabled,
        "files": v.files.iter().map(|f| json!({"path": f.path, "size": f.size,
            "executable": f.executable})).collect::<Vec<_>>(),
        "total_size": v.total_size,
        // (phase 4, P4-GSL) a library skill is pinned and read-only; people change it.
        "source": crate::domains::skills::dto::source_of(&v.row, v.update_pending)
            .map(|s| json!({"kind": s.kind, "repo": s.repo, "path": s.path,
                "commit_sha": s.commit_sha, "detached": s.detached}))})
}

/// The list is a skill index like the one the harness builds: held to the same size, so long
/// descriptions never push other skills past the result limit. `skill_get` gives the whole one.
fn fold_index(skills: &mut [Value]) {
    use aurora_registry::skill_format::{fold_description, index_cap};
    let lengths: Vec<usize> = skills
        .iter()
        .map(|s| s["description"].as_str().map_or(0, |d| d.chars().count()))
        .collect();
    let cap = index_cap(&lengths);
    for s in skills {
        if let Some(d) = s["description"].as_str() {
            s["description"] = json!(fold_description(d, cap));
        }
    }
}

/// `library_skill_readonly`: only people change what a library pinned (docs/10 §Git skill
/// library). The same code as the API's 409.
fn library_readonly(v: &SkillView) -> Result<(), ToolError> {
    if v.row.library_id.is_some() {
        return Err(ToolError::Failed(
            "library_skill_readonly: this skill is managed by a skill library; ask a person to \
             detach it or update it from the library"
                .into(),
        ));
    }
    Ok(())
}

fn name_schema() -> Value {
    json!({"type": "string", "description": "Skill name (kebab-case, at most 64 characters; \
        the aurora- prefix is reserved)."})
}

fn title_schema() -> Value {
    json!({"type": "string", "maxLength": limits::SKILL_TITLE_MAX,
        "description": "Optional display name people see (one line); the folder name stays. \
            An empty string removes it."})
}

async fn agent_of(ctx: &ToolCallContext) -> Result<agents::AgentRow, ToolError> {
    agents::get(&ctx.agent.pool, ctx.agent.agent_id)
        .await?
        .ok_or_else(|| ToolError::Failed("Unknown agent.".into()))
}

/// The agent's own skill of this name, or a read-only error when the name only exists in the
/// workspace or organization tier (those tiers are written by humans, SEC-56).
async fn own_skill(ctx: &ToolCallContext, name: &str) -> Result<SkillView, ToolError> {
    let pool = &ctx.agent.pool;
    match service::get_agent_named(pool, ctx.agent.agent_id, name).await {
        Ok(v) => Ok(v),
        Err(e) if e.status() == http::StatusCode::NOT_FOUND => {
            let agent = agent_of(ctx).await?;
            let eff = service::effective(pool, &agent).await.map_err(fail)?;
            let other = eff
                .items
                .iter()
                .map(|i| &i.view)
                .chain(eff.shadowed.iter().map(|i| &i.view))
                .find(|v| v.row.name == name);
            match other.map(|v| service::tier_of(&v.row)) {
                Some("workspace") => Err(ToolError::Failed(format!(
                    "{name} is a workspace skill and is read-only; ask an owner"
                ))),
                Some("org") => Err(ToolError::Failed(format!(
                    "{name} is an organization skill and is read-only; ask an owner"
                ))),
                _ => Err(fail(e)),
            }
        }
        Err(e) => Err(fail(e)),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameArgs {
    name: String,
}

struct List;

#[async_trait::async_trait]
impl PlatformTool for List {
    fn name(&self) -> &'static str {
        "skill_list"
    }
    fn family(&self) -> &'static str {
        "skills"
    }
    fn description(&self, _: &ToolContext) -> String {
        "List the skills you receive (your own, your workspace's, the organization's and the \
         skills of your apps; all but your own are read-only for you) with their tier and the \
         tiers your own skills shadow, plus your own disabled skills. A long description may be \
         cut here, ending in …; skill_get gives all of it."
            .into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        match &args {
            Value::Null => {}
            Value::Object(m) if m.is_empty() => {}
            _ => {
                return Err(ToolError::InvalidArguments(
                    "This tool takes no arguments.".into(),
                ));
            }
        }
        let agent = agent_of(ctx).await?;
        let eff = service::effective(&ctx.agent.pool, &agent)
            .await
            .map_err(fail)?;
        let mut skills: Vec<Value> = Vec::new();
        for i in &eff.items {
            let mut s = summary(&i.view);
            // `shadows` names the tiers an agent skill overrides; empty otherwise.
            s["shadows"] = if i.view.row.agent_id.is_some() {
                json!(i.shadows.iter().map(|(_, t)| *t).collect::<Vec<_>>())
            } else {
                json!([])
            };
            skills.push(s);
        }
        // The agent's own disabled skills stay findable so it can re-enable them.
        for i in &eff.shadowed {
            if i.view.row.agent_id.is_some() && !i.enabled {
                let mut s = summary(&i.view);
                s["shadows"] = json!([]);
                skills.push(s);
            }
        }
        // The lowest tier: the skills of the apps the agent can use (never one a higher tier holds).
        let apps = app_skills::effective_of(&ctx.state, &agent, &eff)
            .await
            .map_err(fail)?;
        for a in &apps.items {
            skills.push(app_summary(a));
        }
        skills.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        fold_index(&mut skills);
        out(json!({"skills": skills}))
    }
}

struct Get;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetArgs {
    name: String,
    /// Optional file to read instead of the skill body.
    path: Option<String>,
}

#[async_trait::async_trait]
impl PlatformTool for Get {
    fn name(&self) -> &'static str {
        "skill_get"
    }
    fn family(&self) -> &'static str {
        "skills"
    }
    fn description(&self, _: &ToolContext) -> String {
        "Read a skill from the set you receive (agent > workspace > organization > app): body \
         and file list, or one text file when `path` is given."
            .into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false, "required": ["name"],
            "properties": {"name": name_schema(),
                "path": {"type": "string", "description": "Skill file to read."}}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: GetArgs = parse(args)?;
        let pool = &ctx.agent.pool;
        let v = match service::get_agent_named(pool, ctx.agent.agent_id, &a.name).await {
            // The agent's own skill, even a disabled one (so it can find and re-enable it).
            Ok(v) => v,
            Err(e) if e.status() == http::StatusCode::NOT_FOUND => {
                let agent = agent_of(ctx).await?;
                let eff = service::effective(pool, &agent).await.map_err(fail)?;
                let apps = app_skills::effective_of(&ctx.state, &agent, &eff)
                    .await
                    .map_err(fail)?;
                match eff.items.into_iter().find(|i| i.view.row.name == a.name) {
                    Some(i) => i.view,
                    None => {
                        // (C-1133) The lowest tier: an app skill, read from its stored blobs.
                        let sk = apps
                            .items
                            .iter()
                            .find(|s| s.skill.name == a.name)
                            .ok_or_else(|| ToolError::Failed("Skill not found.".into()))?;
                        let mut conn = pool
                            .acquire()
                            .await
                            .map_err(|e| fail(aurora_db::DbError::from(e).into()))?;
                        if let Some(p) = a.path {
                            let f =
                                sk.skill.files.iter().find(|f| f.path == p).ok_or_else(|| {
                                    ToolError::Failed("Skill file not found.".into())
                                })?;
                            let bytes =
                                aurora_db::repo::plugins::get_agent_blob(&mut *conn, &f.sha256)
                                    .await
                                    .map_err(|e| fail(e.into()))?
                                    .ok_or_else(|| {
                                        ToolError::Failed("Skill file not found.".into())
                                    })?;
                            let content = String::from_utf8(bytes)
                                .map_err(|_| ToolError::Failed("Skill file is not text.".into()))?;
                            return out(
                                json!({"name": sk.skill.name, "path": p, "content": content}),
                            );
                        }
                        let mut s = app_summary(sk);
                        s["body"] = json!(
                            app_skills::body(&mut conn, sk)
                                .await
                                .map_err(fail)?
                                .unwrap_or_default()
                        );
                        return out(s);
                    }
                }
            }
            Err(e) => return Err(fail(e)),
        };
        if let Some(p) = a.path {
            let content = service::read_file(pool, v.row.id, &p).await.map_err(fail)?;
            return out(json!({"name": v.row.name, "path": p, "content": content}));
        }
        let mut s = summary(&v);
        s["body"] = json!(v.row.body);
        // (C-1175) Reading a skill's instructions is a use of it; reading one of its files is
        // not, and neither is reading a skill that does not apply (an own skill switched off).
        record_use(ctx, &v).await;
        out(s)
    }
}

/// Counts the read as a use of the skill by this agent in this turn. A failure to count never
/// fails the read.
async fn record_use(ctx: &ToolCallContext, v: &SkillView) {
    let recorded = async {
        let agent = agent_of(ctx).await.ok()?;
        let mut conn = ctx.agent.pool.acquire().await.ok()?;
        crate::domains::skills::usage::record_loaded(
            &mut conn,
            &agent,
            ctx.conversation_id,
            ctx.turn_id,
            &v.row.name,
            Some(v.row.id),
        )
        .await
        .map_err(|e| tracing::warn!(error = %e, "a skill use was not recorded"))
        .ok()
    };
    let _ = recorded.await;
}

struct Create;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    name: String,
    title: Option<String>,
    description: String,
    body: String,
}

#[async_trait::async_trait]
impl PlatformTool for Create {
    fn name(&self) -> &'static str {
        "skill_create"
    }
    fn family(&self) -> &'static str {
        "skills"
    }
    fn description(&self, _: &ToolContext) -> String {
        format!(
            "Create one of your own skills: a reusable instruction bundle projected into your \
             skills directory (body at most {} characters).",
            limits::SKILL_BODY_MAX
        )
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false,
            "required": ["name", "description", "body"],
            "properties": {"name": name_schema(), "title": title_schema(),
                "description": {"type": "string", "maxLength": limits::SKILL_DESCRIPTION_MAX,
                    "description": "One line saying when to use the skill."},
                "body": {"type": "string", "description": "Markdown instructions."}}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: CreateArgs = parse(args)?;
        let org = agent_of(ctx).await?.org_id;
        let n = NewSkill {
            name: &a.name,
            title: a.title.as_deref(),
            description: &a.description,
            body: &a.body,
            enabled: true,
        };
        let owner = Owner {
            agent: Some(ctx.agent.agent_id),
            workspace: None,
        };
        // A name taken in another tier is fine: the new agent skill shadows it.
        let v = service::create(&ctx.agent.pool, org, owner, &n, "agent")
            .await
            .map_err(fail)?;
        out(summary(&v))
    }
}

struct Update;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    name: String,
    title: Option<String>,
    description: Option<String>,
    body: Option<String>,
    enabled: Option<bool>,
}

#[async_trait::async_trait]
impl PlatformTool for Update {
    fn name(&self) -> &'static str {
        "skill_update"
    }
    fn family(&self) -> &'static str {
        "skills"
    }
    fn description(&self, _: &ToolContext) -> String {
        "Update the display name, description, body or enabled flag of one of your own skills."
            .into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false, "required": ["name"],
            "properties": {"name": name_schema(), "title": title_schema(),
                "description": {"type": "string", "maxLength": limits::SKILL_DESCRIPTION_MAX},
                "body": {"type": "string"}, "enabled": {"type": "boolean"}}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: UpdateArgs = parse(args)?;
        let cur = own_skill(ctx, &a.name).await?;
        library_readonly(&cur)?;
        let p = Patch {
            title: a.title.as_deref(),
            description: a.description.as_deref(),
            body: a.body.as_deref(),
            enabled: a.enabled,
        };
        let v = service::update(&ctx.agent.pool, cur.row.id, &p)
            .await
            .map_err(fail)?;
        out(summary(&v))
    }
}

struct Delete;

#[async_trait::async_trait]
impl PlatformTool for Delete {
    fn name(&self) -> &'static str {
        "skill_delete"
    }
    fn family(&self) -> &'static str {
        "skills"
    }
    fn description(&self, _: &ToolContext) -> String {
        "Delete one of your own skills and its files.".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false, "required": ["name"],
            "properties": {"name": name_schema()}})
    }
    fn available(&self, _: &ToolContext) -> bool {
        true
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: NameArgs = parse(args)?;
        let cur = own_skill(ctx, &a.name).await?;
        library_readonly(&cur)?;
        service::delete(&ctx.agent.pool, cur.row.id)
            .await
            .map_err(fail)?;
        out(json!({"deleted": a.name}))
    }
}
