//! Settings in words (phase 5, P5-IA-5, D-341, C-972): the label, help, choices, default (in words),
//! unit and operator-doc reference of every movable key, from one registry next to the section
//! registry in [`crate::settings`]. The dashboard renders from it; nothing derives a label from a
//! key. A unit test keeps it complete against the settings registry
//! (`deploy/helm/aurora/movable-keys.txt`).

use crate::settings::registry;
#[cfg(test)]
use crate::settings::{SettingDef, SettingType};

/// How the dashboard renders a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Bool,
    Int,
    /// A decimal number.
    Float,
    /// A number of seconds, edited in `unit` (`minutes`, `hours` or `days`).
    Duration,
    /// One of `choices`.
    Enum,
    /// A list of text entries (or, with `choices`, a subset of them).
    List,
    Text,
    /// A write-only value.
    Secret,
    /// Entries with their own editor (providers, endpoints, stores, rate classes).
    KeyedList,
}

impl FieldKind {
    /// The wire name (C-972).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Float => "float",
            Self::Duration => "duration",
            Self::Enum => "enum",
            Self::List => "list",
            Self::Text => "text",
            Self::Secret => "secret",
            Self::KeyedList => "keyed_list",
        }
    }
}

/// One choice of an enumeration: the stored value, its label and a sentence.
#[derive(Debug, Clone, Copy)]
pub struct Choice {
    pub value: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

/// An operator document, named by its title (the interface never shows the path).
#[derive(Debug, Clone, Copy)]
pub struct DocRef {
    pub title: &'static str,
    pub path: &'static str,
}

/// The words of one settings field (C-972).
#[derive(Debug, Clone, Copy)]
pub struct FieldMeta {
    pub key: &'static str,
    pub label: &'static str,
    /// One sentence: what the setting does and when to change it.
    pub help: &'static str,
    pub kind: FieldKind,
    pub choices: &'static [Choice],
    /// The default, in words ("15 minutes", "Off", "Daneel").
    pub default_display: &'static str,
    /// The unit the control edits in ("minutes", "days", "MiB", "requests a minute").
    pub unit: Option<&'static str>,
    pub doc: Option<DocRef>,
}

const NO: &[Choice] = &[];

const fn doc(title: &'static str, path: &'static str) -> Option<DocRef> {
    Some(DocRef { title, path })
}

const SIGN_IN: Option<DocRef> = doc("Sign-in providers", "docs/operator/sign-in.md");
const GATEWAY: Option<DocRef> = doc(
    "Gateway posture and BYO agents",
    "docs/operator/gateway-posture.md",
);
const LLM_DOC: Option<DocRef> = doc("AI endpoints and rules", "docs/operator/llm-endpoints.md");
const LIMITS_DOC: Option<DocRef> = doc(
    "Rate limits, approvals and e-mail",
    "docs/operator/limits-and-approvals.md",
);
const RETENTION: Option<DocRef> = doc(
    "Retention and attachment quota",
    "docs/operator/retention-and-quota.md",
);
const SKILLS: Option<DocRef> = doc("Git skill libraries", "docs/operator/skill-libraries.md");
const SCIM: Option<DocRef> = doc("Directory sync", "docs/operator/scim-provisioning.md");
const PREVIEWS: Option<DocRef> = doc("Port previews", "docs/operator/previews.md");
const TERMINAL: Option<DocRef> = doc("Web terminal", "docs/operator/terminal.md");
const HARNESSES: Option<DocRef> = doc("Harnesses", "docs/operator/harnesses.md");
const DEFAULTS_DOC: Option<DocRef> = doc(
    "Deployment defaults",
    "docs/operator/deployment-defaults.md",
);
const VAULT: Option<DocRef> = doc("Secret stores", "docs/operator/vault.md");
const RUNNERS: Option<DocRef> = doc("Runners", "docs/operator/runners.md");

#[allow(clippy::too_many_arguments)]
const fn f(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    kind: FieldKind,
    default_display: &'static str,
    unit: Option<&'static str>,
    doc: Option<DocRef>,
) -> FieldMeta {
    FieldMeta {
        key,
        label,
        help,
        kind,
        choices: NO,
        default_display,
        unit,
        doc,
    }
}

const fn with_choices(mut m: FieldMeta, choices: &'static [Choice]) -> FieldMeta {
    m.choices = choices;
    m
}

use FieldKind as K;

const SCOPES: &[Choice] = &[
    Choice {
        value: "organization",
        label: "Organization",
        description: "An organization owner adds the key for everyone in the organization.",
    },
    Choice {
        value: "workspace",
        label: "Workspace",
        description: "A workspace owner adds the key for that workspace only.",
    },
];
const LLM_POSTURE: &[Choice] = &[
    Choice {
        value: "open",
        label: "Any AI endpoint",
        description: "Organizations decide which AI endpoints their agents may use.",
    },
    Choice {
        value: "sanctioned_only",
        label: "Approved endpoints only",
        description: "Agents may only use the AI endpoints you set up and the known providers.",
    },
];
const EGRESS_POSTURE: &[Choice] = &[
    Choice {
        value: "open",
        label: "Any website",
        description: "Organizations decide where their agents may connect.",
    },
    Choice {
        value: "credentialed_only",
        label: "Only websites with an account",
        description: "Agents may only connect to websites an account was added for, plus the always-allowed list.",
    },
];
const OPEN_LANE: &[Choice] = &[
    Choice {
        value: "enabled",
        label: "Organizations choose",
        description: "An organization may leave open access to the internet on.",
    },
    Choice {
        value: "allowlist_only",
        label: "Allowed websites only",
        description: "Open access works only for websites on the always-allowed list.",
    },
    Choice {
        value: "disabled",
        label: "Never open",
        description: "No agent has open access to the internet.",
    },
];
const BYO_POSTURE: &[Choice] = &[
    Choice {
        value: "enabled",
        label: "Anyone in a workspace",
        description: "Any workspace member may connect their own agent.",
    },
    Choice {
        value: "owners_only",
        label: "Workspace owners only",
        description: "Only workspace owners may connect their own agent.",
    },
    Choice {
        value: "disabled",
        label: "Nobody",
        description: "Nobody may connect their own agent; the ones already connected stop working.",
    },
];
const SIGNUP: &[Choice] = &[
    Choice {
        value: "invite_only",
        label: "Invited people only",
        description: "Organization owners and admins invite each person.",
    },
    Choice {
        value: "sso_jit",
        label: "Anyone with a verified e-mail",
        description: "They join at their first sign-in through your identity provider.",
    },
    Choice {
        value: "open",
        label: "Anyone",
        description: "Anyone who can reach this address may create an account.",
    },
];
const PLATFORM_TOOLS: &[Choice] = &[
    Choice {
        value: "on_demand",
        label: "On demand",
        description: "Only the few core tools are listed; the agent searches for the others.",
    },
    Choice {
        value: "all",
        label: "All listed",
        description: "Every tool is listed to the agent in every session.",
    },
];
const ORG_CREATION: &[Choice] = &[
    Choice {
        value: "instance_admins",
        label: "Aurora administrators only",
        description: "Only Aurora administrators create organizations.",
    },
    Choice {
        value: "anyone",
        label: "Anyone who is signed in",
        description: "Every person may create an organization of their own.",
    },
];

/// Every movable key's field, in section order.
pub static FIELDS: &[FieldMeta] = &[
    // ---------------------------------------------------------------- governance
    with_choices(
        f(
            "governance.llm.allowed_key_scopes",
            "Where AI keys may be added",
            "Pick who may add an AI key; removing a level stops new keys there.",
            K::List,
            "Organizations and workspaces",
            None,
            LLM_DOC,
        ),
        SCOPES,
    ),
    with_choices(
        f(
            "governance.llm.minimum_posture",
            "Which AI endpoints agents may use",
            "The strictest rule an organization may not go below.",
            K::Enum,
            "Any AI endpoint",
            None,
            LLM_DOC,
        ),
        LLM_POSTURE,
    ),
    f(
        "governance.llm.require_key_origin",
        "Check where AI keys come from",
        "When on, a key only works for the provider it was added for, so a key cannot be sent somewhere else.",
        K::Bool,
        "Off",
        None,
        LLM_DOC,
    ),
    f(
        "governance.llm.known_llm_hosts_extra",
        "More AI provider addresses",
        "Host names to treat as AI providers on top of the built-in list, one per line.",
        K::List,
        "None",
        None,
        LLM_DOC,
    ),
    with_choices(
        f(
            "governance.egress.minimum_posture",
            "Where agents may connect",
            "The strictest Internet access an organization may not go below.",
            K::Enum,
            "Any website",
            None,
            GATEWAY,
        ),
        EGRESS_POSTURE,
    ),
    f(
        "governance.egress.baseline_allowlist",
        "Websites always allowed",
        "Websites every agent may reach whatever its organization chose; one host name per line, optionally followed by a path.",
        K::List,
        "None",
        None,
        GATEWAY,
    ),
    with_choices(
        f(
            "governance.egress.open_lane_minimum",
            "Open access to the internet",
            "How far an organization may open its agents' access to the whole internet.",
            K::Enum,
            "Organizations choose",
            None,
            GATEWAY,
        ),
        OPEN_LANE,
    ),
    f(
        "governance.agent_tokens.max_age_days",
        "Longest life of an agent token",
        "Lowering it shortens tokens that already exist, keeping at least seven days; leave it empty for no limit.",
        K::Int,
        "No limit",
        Some("days"),
        GATEWAY,
    ),
    f(
        "byo.enabled",
        "Allow agents people run themselves",
        "Turn it off to stop new BYO agents and refuse the ones already connected, at once.",
        K::Bool,
        "On",
        None,
        GATEWAY,
    ),
    with_choices(
        f(
            "byo.minimum_posture",
            "Who may connect their own agent",
            "The widest permission an organization may give.",
            K::Enum,
            "Workspace owners only",
            None,
            GATEWAY,
        ),
        BYO_POSTURE,
    ),
    f(
        "byo.default_source_cidrs",
        "Addresses their agents may connect from",
        "Network ranges a new BYO agent accepts, one per line; empty means any address.",
        K::List,
        "None",
        None,
        GATEWAY,
    ),
    f(
        "byo.require_context_credentials",
        "Ask BYO agents to prove where they run",
        "When on, a BYO agent must present the context key issued to it, not only its token.",
        K::Bool,
        "Off",
        None,
        GATEWAY,
    ),
    // ---------------------------------------------------------------- llm
    f(
        "llm.enabled_providers",
        "AI providers you offer",
        "The providers people may add AI keys for, one per line.",
        K::List,
        "LiteLLM and Anthropic",
        None,
        LLM_DOC,
    ),
    f(
        "llm.provider_preference",
        "AI provider order",
        "When more than one provider can answer, the first in this list is used; one provider per line.",
        K::List,
        "LiteLLM, then Anthropic",
        None,
        LLM_DOC,
    ),
    f(
        "llm.endpoints",
        "AI model endpoints",
        "Servers your agents may send AI requests to, with the models each one offers.",
        K::KeyedList,
        "None",
        None,
        LLM_DOC,
    ),
    // ---------------------------------------------------------------- sign_in
    with_choices(
        f(
            "auth.signup_mode",
            "Who can sign up",
            "How a new person gets an account.",
            K::Enum,
            "Invited people only",
            None,
            SIGN_IN,
        ),
        SIGNUP,
    ),
    with_choices(
        f(
            "auth.org_creation",
            "Who can create an organization",
            "Who may start a new organization.",
            K::Enum,
            "Aurora administrators only",
            None,
            SIGN_IN,
        ),
        ORG_CREATION,
    ),
    f(
        "auth.session_idle_secs",
        "Sign out after being away",
        "How long someone may be away before they have to sign in again.",
        K::Duration,
        "7 days",
        Some("days"),
        SIGN_IN,
    ),
    f(
        "auth.session_absolute_secs",
        "Longest a sign-in lasts",
        "Everyone signs in again after this long, however active they are.",
        K::Duration,
        "30 days",
        Some("days"),
        SIGN_IN,
    ),
    f(
        "auth.cli_key_ttl_days",
        "How long a command-line key lasts",
        "Keys made by signing in on the command line expire after this long; 0 means they never expire.",
        K::Int,
        "90 days",
        Some("days"),
        SIGN_IN,
    ),
    f(
        "auth.oidc_providers",
        "How people sign in",
        "The identity providers people sign in with.",
        K::KeyedList,
        "None",
        None,
        SIGN_IN,
    ),
    // ---------------------------------------------------------------- email
    f(
        "email.smtp_url",
        "Mail server",
        "The address of the server that sends Aurora's e-mails; Aurora checks it can reach it before saving.",
        K::Text,
        "Not set",
        None,
        LIMITS_DOC,
    ),
    f(
        "email.from",
        "Send as",
        "The address approval and invitation e-mails come from.",
        K::Text,
        "Not set",
        None,
        LIMITS_DOC,
    ),
    f(
        "email.smtp_username",
        "User name",
        "The account Aurora signs in with on the mail server, if it needs one.",
        K::Text,
        "Not set",
        None,
        LIMITS_DOC,
    ),
    f(
        "email.smtp_password",
        "Password",
        "Sent only to the mail server and never shown again once set.",
        K::Secret,
        "Not set",
        None,
        LIMITS_DOC,
    ),
    // ---------------------------------------------------------------- secrets
    f(
        "secrets.default_store",
        "Where new secrets are kept",
        "The secret store used when someone adds a secret without choosing one.",
        K::Text,
        "Aurora's built-in store",
        None,
        VAULT,
    ),
    f(
        "secrets.stores",
        "Secret stores",
        "Outside stores such as Vault that can hold secret values for Aurora.",
        K::KeyedList,
        "None",
        None,
        VAULT,
    ),
    // ---------------------------------------------------------------- agents
    f(
        "agents.default_name",
        "Name of a new person's agent",
        "The name an agent gets unless the person who creates it picks another.",
        K::Text,
        "Daneel",
        None,
        DEFAULTS_DOC,
    ),
    f(
        "agents.default_brief",
        "Starting instructions for new agents",
        "The text a new agent starts with; existing agents keep theirs.",
        K::Text,
        "Aurora's standard brief",
        None,
        DEFAULTS_DOC,
    ),
    f(
        "hosted.enabled",
        "Agents Aurora runs for people",
        "Turn it off to stop new hosted agents and sandbox starts; agents people run themselves are unaffected.",
        K::Bool,
        "On",
        None,
        RUNNERS,
    ),
    f(
        "hosted.default_harness",
        "Agent program new agents use",
        "Must be one of the programs listed below.",
        K::Text,
        "claude-code",
        None,
        HARNESSES,
    ),
    f(
        "hosted.enabled_harnesses",
        "Agent programs agents may run",
        "One per line; each must be offered by at least one runner.",
        K::List,
        "claude-code",
        None,
        HARNESSES,
    ),
    f(
        "hosted.default_timezone",
        "Time zone for new agents",
        "A name such as Europe/Zurich; a person's own time zone wins.",
        K::Text,
        "UTC",
        None,
        DEFAULTS_DOC,
    ),
    f(
        "hosted.idle_stop_secs",
        "Stop an idle computer after",
        "How long a computer may sit without activity before it is stopped.",
        K::Duration,
        "30 minutes",
        Some("minutes"),
        RUNNERS,
    ),
    f(
        "hosted.background_cap_secs",
        "Longest a computer stays awake for background work",
        "A computer running background work is stopped after this long.",
        K::Duration,
        "12 hours",
        Some("hours"),
        RUNNERS,
    ),
    f(
        "hosted.turn_ceiling_secs",
        "Longest one turn may run",
        "Must be at least the longest a question may wait plus 15 minutes.",
        K::Duration,
        "6 hours",
        Some("hours"),
        RUNNERS,
    ),
    f(
        "hosted.turn_stall_secs",
        "Treat a silent turn as stuck after",
        "A turn that produces nothing for this long is stopped.",
        K::Duration,
        "10 minutes",
        Some("minutes"),
        RUNNERS,
    ),
    f(
        "hosted.sandbox_ready_timeout_secs",
        "How long to wait for a computer to start",
        "If its computer is not ready by then, the turn fails and says why.",
        K::Duration,
        "5 minutes",
        Some("minutes"),
        RUNNERS,
    ),
    with_choices(
        f(
            "hosted.platform_tools",
            "How agents find Aurora's own tools",
            "On demand keeps the list the agent starts with short; it finds the rest with a command in its computer.",
            K::Enum,
            "On demand",
            None,
            RUNNERS,
        ),
        PLATFORM_TOOLS,
    ),
    // ---------------------------------------------------------------- approvals
    f(
        "approvals.timeout_secs",
        "How long a question waits for an answer",
        "After this, the request is refused and the agent is told no one answered.",
        K::Duration,
        "15 minutes",
        Some("minutes"),
        LIMITS_DOC,
    ),
    f(
        "approvals.max_timeout_secs",
        "Longest a rule may make a question wait",
        "A rule asking for more is cut to this; it must stay below the longest turn minus 15 minutes.",
        K::Duration,
        "1 hour",
        Some("minutes"),
        LIMITS_DOC,
    ),
    f(
        "approvals.email_notifications",
        "E-mail people about questions",
        "Send an e-mail when a question is waiting for them; this also covers the attachment storage notices.",
        K::Bool,
        "On",
        None,
        LIMITS_DOC,
    ),
    f(
        "approvals.park.enabled",
        "Let agents stop waiting for long approvals",
        "A request that needs a long approval is refused at once and the agent resumes when a person approves; off, it is held open as before.",
        K::Bool,
        "Off",
        None,
        LIMITS_DOC,
    ),
    f(
        "approvals.park.threshold_secs",
        "Park questions that wait longer than",
        "A question that may wait longer than this is parked instead of held open; it must not exceed the longest a rule may make a question wait while parking is on.",
        K::Duration,
        "15 minutes",
        Some("minutes"),
        LIMITS_DOC,
    ),
    f(
        "approvals.park.max_timeout_secs",
        "Longest a parked question may wait",
        "While parking is on, a rule may ask for this long; it must be at least the longest a rule may make a question wait.",
        K::Duration,
        "1 day",
        Some("hours"),
        LIMITS_DOC,
    ),
    f(
        "approvals.park.resume_window_secs",
        "How long an approval stays usable",
        "After a person approves a parked question, the agent has this long to repeat the request once.",
        K::Duration,
        "30 minutes",
        Some("minutes"),
        LIMITS_DOC,
    ),
    f(
        "approvals.park.deny_cooldown_secs",
        "Pause after a denial",
        "After a denial the same request is refused without a new question for this long.",
        K::Duration,
        "15 minutes",
        Some("minutes"),
        LIMITS_DOC,
    ),
    f(
        "approvals.park.max_body_bytes",
        "Largest request that can be parked",
        "A request body above this is held open instead; parked requests keep only a fingerprint of the body.",
        K::Int,
        "256 KiB",
        Some("bytes"),
        LIMITS_DOC,
    ),
    f(
        "approvals.park.max_pending_per_agent",
        "Parked questions per agent",
        "An agent with this many parked questions waiting gets a limit answer instead of a new one.",
        K::Int,
        "20",
        None,
        LIMITS_DOC,
    ),
    // ---------------------------------------------------------------- retention
    f(
        "retention.request_log_days",
        "Keep the activity of requests for",
        "How long the record of what agents asked for is kept.",
        K::Int,
        "30 days",
        Some("days"),
        RETENTION,
    ),
    f(
        "retention.conversation_days_max",
        "Longest an organization may keep conversations",
        "Organizations choose freely when empty; otherwise none may keep conversations longer.",
        K::Int,
        "No limit",
        Some("days"),
        RETENTION,
    ),
    f(
        "attachments.default_org_quota_mib",
        "File storage for an organization",
        "What an organization gets until it sets its own; 0 means no limit.",
        K::Int,
        "10 GiB",
        Some("MiB"),
        RETENTION,
    ),
    f(
        "attachments.org_quota_mib_max",
        "Most file storage an organization may set",
        "The ceiling for an organization's own limit; 0 means no ceiling.",
        K::Int,
        "50 GiB",
        Some("MiB"),
        RETENTION,
    ),
    f(
        "attachments.days_max",
        "Longest an organization may keep files",
        "Organizations choose freely when empty; otherwise none may keep files longer.",
        K::Int,
        "No limit",
        Some("days"),
        RETENTION,
    ),
    // ---------------------------------------------------------------- skills
    f(
        "skills.library.enabled",
        "Skill libraries from git",
        "Turn it off to stop refreshing libraries; agents keep the skills they already have.",
        K::Bool,
        "On",
        None,
        SKILLS,
    ),
    f(
        "skills.library.refresh_interval_secs",
        "Check libraries for changes every",
        "How often each library is fetched again; at least five minutes.",
        K::Duration,
        "15 minutes",
        Some("minutes"),
        SKILLS,
    ),
    f(
        "skills.library.bytes_per_agent",
        "Most library content one agent receives",
        "Larger libraries are cut to this size per agent.",
        K::Int,
        "1 MiB",
        Some("bytes"),
        SKILLS,
    ),
    // ---------------------------------------------------------------- scim
    f(
        "scim.enabled",
        "Directory sync",
        "Lets your identity system add and remove people and groups automatically.",
        K::Bool,
        "Follows what your deployment provides",
        None,
        SCIM,
    ),
    f(
        "scim.allowed_email_domains",
        "E-mail domains directory sync may add",
        "One domain per line; empty means any domain an organization owner lists.",
        K::List,
        "Any",
        None,
        SCIM,
    ),
    f(
        "scim.rate_per_minute",
        "Requests a minute per sync token",
        "Between 30 and 6000; sync clients slow down beyond it.",
        K::Int,
        "600",
        Some("requests a minute"),
        SCIM,
    ),
    f(
        "scim.token_max_days",
        "Longest life of a sync token",
        "Between 1 and 730 days.",
        K::Int,
        "730 days",
        Some("days"),
        SCIM,
    ),
    // ---------------------------------------------------------------- previews
    f(
        "previews.session_ttl_secs",
        "Longest a visitor's preview lasts",
        "A visitor signs in again after this long.",
        K::Duration,
        "8 hours",
        Some("hours"),
        PREVIEWS,
    ),
    f(
        "previews.session_idle_secs",
        "End a quiet preview after",
        "A preview with no traffic for this long ends.",
        K::Duration,
        "1 hour",
        Some("minutes"),
        PREVIEWS,
    ),
    f(
        "previews.bytes_per_share_day",
        "Data one shared preview may serve a day",
        "Counted in bytes; a share stops serving when it is used up.",
        K::Int,
        "5 GiB",
        Some("bytes"),
        PREVIEWS,
    ),
    f(
        "previews.shares_per_agent",
        "Shared previews per agent",
        "How many shares one agent may have at the same time.",
        K::Int,
        "10",
        Some("shares"),
        PREVIEWS,
    ),
    f(
        "previews.shares_per_org",
        "Shared previews per organization",
        "How many shares one organization may have at the same time.",
        K::Int,
        "200",
        Some("shares"),
        PREVIEWS,
    ),
    f(
        "previews.awake_cap_secs",
        "Longest a preview keeps a computer awake",
        "A computer serving a preview is allowed to sleep after this long.",
        K::Duration,
        "4 hours",
        Some("hours"),
        PREVIEWS,
    ),
    // ---------------------------------------------------------------- terminal
    f(
        "terminal.enabled",
        "Web terminal",
        "An interactive shell in an agent's computer; turning it off closes new sessions at once.",
        K::Bool,
        "Follows what your deployment provides",
        None,
        TERMINAL,
    ),
    f(
        "terminal.awake_cap_secs",
        "Longest a terminal keeps a computer awake",
        "At most the background limit set under Agents.",
        K::Duration,
        "4 hours",
        Some("hours"),
        TERMINAL,
    ),
    f(
        "terminal.wake_window_secs",
        "How long a wake-up request stays valid",
        "A request to wake a sleeping computer expires after this long.",
        K::Duration,
        "1 minute",
        Some("minutes"),
        TERMINAL,
    ),
    f(
        "terminal.wake_wait_secs",
        "How long opening a terminal waits",
        "Opening a terminal waits this long for a sleeping computer to wake.",
        K::Duration,
        "45 seconds",
        Some("seconds"),
        TERMINAL,
    ),
    // ---------------------------------------------------------------- limits
    f(
        "ratelimit.api.multiplier",
        "Scale every request limit",
        "Between 0.1 and 100; 2 doubles every limit and 0.5 halves it.",
        K::Float,
        "1 (no change)",
        Some("times"),
        LIMITS_DOC,
    ),
    f(
        "ratelimit.api.org_per_minute",
        "Requests an organization may make a minute",
        "The burst allowed is a fifth of it.",
        K::Int,
        "3000",
        Some("requests a minute"),
        LIMITS_DOC,
    ),
    f(
        "ratelimit.api.classes",
        "Limits for one kind of request",
        "Overrides the limit of a single kind of request; the scale above applies on top.",
        K::KeyedList,
        "None",
        None,
        LIMITS_DOC,
    ),
    f(
        "gateway.allowed_private",
        "Private addresses agents may reach",
        "Network ranges, addresses or host names on your own network, one per line; every entry opens that range to agents and apps.",
        K::List,
        "None",
        None,
        GATEWAY,
    ),
    f(
        "gateway.condition_body_bytes",
        "How much of a request a rule reads",
        "The part of a request body the gateway reads to check a rule's conditions: 4096 to 8388608 bytes.",
        K::Int,
        "256 KiB",
        Some("bytes"),
        GATEWAY,
    ),
    f(
        "gateway.upstream_header_timeout_secs",
        "How long to wait for a website to start answering",
        "A website that has not begun to answer by then is treated as down.",
        K::Duration,
        "5 minutes",
        Some("minutes"),
        GATEWAY,
    ),
];

/// The field of a key.
pub fn field(key: &str) -> Option<&'static FieldMeta> {
    FIELDS.iter().find(|m| m.key == key)
}

/// The fields of one section, in registry order.
pub fn section_fields(section: &str) -> Vec<&'static FieldMeta> {
    registry()
        .section(section)
        .filter_map(|d| field(d.key))
        .collect()
}

fn plural(n: i64, unit: &str) -> String {
    let singular = unit.strip_suffix('s').unwrap_or(unit);
    if n == 1 {
        format!("1 {singular}")
    } else {
        format!("{n} {unit}")
    }
}

/// A number of seconds in the largest whole unit ("15 minutes", "1 hour", "45 seconds").
pub fn duration_words(secs: i64) -> String {
    for (size, unit) in [(86_400, "days"), (3600, "hours"), (60, "minutes")] {
        if secs >= size && secs % size == 0 {
            return plural(secs / size, unit);
        }
    }
    plural(secs, "seconds")
}

fn scalar_words(m: &FieldMeta, v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Bool(b) => if *b { "On" } else { "Off" }.into(),
        Value::Number(n) => {
            if m.kind == FieldKind::Duration
                && let Some(secs) = n.as_i64()
            {
                return duration_words(secs);
            }
            match (m.unit, n.as_i64()) {
                (Some(u), Some(i)) => plural(i, u),
                _ => n.to_string(),
            }
        }
        Value::String(s) => m
            .choices
            .iter()
            .find(|c| c.value == s)
            .map_or_else(|| s.clone(), |c| c.label.to_string()),
        Value::Object(o) => o
            .get("host")
            .or_else(|| o.get("id"))
            .or_else(|| o.get("class"))
            .and_then(Value::as_str)
            .map_or_else(|| "An entry".into(), str::to_string),
        _ => String::new(),
    }
}

/// A key's value in words, for the read-only view (C-973): "On", "15 minutes", "Approved endpoints
/// only", a list's entries comma-separated, "None" for an empty list and "Not set" for nothing. A
/// secret key reads only "Set" or "Not set", whatever `value` holds.
pub fn value_words(m: &FieldMeta, value: &serde_json::Value, secret_set: bool) -> String {
    use serde_json::Value;
    if m.kind == FieldKind::Secret {
        return if secret_set { "Set" } else { "Not set" }.into();
    }
    match value {
        Value::Null => "Not set".into(),
        Value::Array(a) if a.is_empty() => "None".into(),
        Value::Array(a) if m.kind == FieldKind::KeyedList => {
            format!("{} configured", a.len())
        }
        Value::Array(a) => a
            .iter()
            .map(|x| scalar_words(m, x))
            .collect::<Vec<_>>()
            .join(", "),
        v => scalar_words(m, v),
    }
}

/// The kind a registry definition implies (`FIELDS` must agree).
#[cfg(test)]
fn implied_kind(d: &SettingDef) -> &'static [FieldKind] {
    use FieldKind as K;
    match (&d.ty, d.list.is_some()) {
        (SettingType::Bool, _) => &[K::Bool],
        (SettingType::Int { .. }, _) => &[K::Int],
        (SettingType::Float { .. }, _) => &[K::Float],
        (SettingType::DurationSecs { .. }, _) => &[K::Duration],
        (SettingType::Str { .. }, _) if d.secret => &[K::Secret],
        (SettingType::Str { .. }, _) => &[K::Text],
        (SettingType::Enum(_), _) => &[K::Enum],
        (SettingType::List { .. }, _) => &[K::List, K::KeyedList],
        (SettingType::Object, _) => &[K::KeyedList],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `hosted.*_secs` and the other deadline keys are plain `Int` in the registry but read as durations.
    fn kind_ok(d: &SettingDef, m: &FieldMeta) -> bool {
        implied_kind(d).contains(&m.kind)
            || (m.kind == K::Duration && matches!(d.ty, SettingType::Int { .. }))
    }

    #[test]
    fn every_movable_key_has_a_field_and_every_field_a_key() {
        for d in registry().defs().iter().filter(|d| !d.test_only) {
            assert!(field(d.key).is_some(), "{} has no FieldMeta", d.key);
        }
        for m in FIELDS {
            assert!(
                registry().get(m.key).is_some_and(|d| !d.test_only),
                "{} has a FieldMeta but is not a movable key",
                m.key
            );
        }
        let mut keys: Vec<_> = FIELDS.iter().map(|m| m.key).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(n, keys.len(), "a key has two FieldMeta entries");
    }

    #[test]
    fn labels_help_and_choices_are_written_in_words() {
        for m in FIELDS {
            assert!(!m.label.trim().is_empty(), "{}: empty label", m.key);
            assert!(!m.help.trim().is_empty(), "{}: empty help", m.key);
            assert!(
                !m.default_display.trim().is_empty(),
                "{}: empty default",
                m.key
            );
            for text in [m.label, m.help, m.default_display] {
                assert!(
                    !text.contains('§') && !text.contains("docs/"),
                    "{}: `{text}` cites a spec section or a repository path",
                    m.key
                );
            }
            for c in m.choices {
                assert!(
                    !c.label.trim().is_empty(),
                    "{}: choice {} has no label",
                    m.key,
                    c.value
                );
                assert!(
                    !c.description.trim().is_empty(),
                    "{}: choice {} has no description",
                    m.key,
                    c.value
                );
                assert!(
                    c.label != c.value && !c.label.contains('_'),
                    "{}: choice {} shows its raw id",
                    m.key,
                    c.value
                );
            }
            if let Some(d) = m.doc {
                assert!(
                    d.path.starts_with("docs/operator/") && !d.title.contains("docs/"),
                    "{}",
                    m.key
                );
            }
            assert!(
                !m.label.contains('_') && !m.label.contains('.'),
                "{}: label looks like a key",
                m.key
            );
        }
    }

    #[test]
    fn kinds_and_choices_agree_with_the_registry() {
        for m in FIELDS {
            let d = registry().get(m.key).unwrap();
            assert!(
                kind_ok(d, m),
                "{}: kind {:?} does not fit {:?}",
                m.key,
                m.kind,
                d.ty
            );
            if let SettingType::Enum(options) = &d.ty {
                let mut have: Vec<_> = m.choices.iter().map(|c| c.value).collect();
                let mut want = options.clone();
                have.sort_unstable();
                want.sort_unstable();
                assert_eq!(
                    have, want,
                    "{}: choices must be exactly the enumeration",
                    m.key
                );
            } else if m.kind != K::List {
                assert!(
                    m.choices.is_empty(),
                    "{}: choices on a non-enumeration",
                    m.key
                );
            }
            if m.kind == K::Duration {
                assert!(
                    matches!(m.unit, Some("seconds" | "minutes" | "hours" | "days")),
                    "{}: a duration is edited in seconds, minutes, hours or days",
                    m.key
                );
            }
        }
    }

    #[test]
    fn values_read_in_words_and_a_secret_is_never_echoed() {
        use serde_json::json;
        let m = |k| field(k).unwrap();
        assert_eq!(
            value_words(m("approvals.timeout_secs"), &json!(900), false),
            "15 minutes"
        );
        assert_eq!(
            value_words(m("hosted.turn_ceiling_secs"), &json!(21_600), false),
            "6 hours"
        );
        assert_eq!(
            value_words(m("terminal.wake_wait_secs"), &json!(45), false),
            "45 seconds"
        );
        assert_eq!(value_words(m("byo.enabled"), &json!(true), false), "On");
        assert_eq!(
            value_words(
                m("governance.llm.minimum_posture"),
                &json!("sanctioned_only"),
                false
            ),
            "Approved endpoints only"
        );
        assert_eq!(
            value_words(m("byo.default_source_cidrs"), &json!([]), false),
            "None"
        );
        assert_eq!(
            value_words(
                m("governance.llm.allowed_key_scopes"),
                &json!(["organization", "workspace"]),
                false
            ),
            "Organization, Workspace"
        );
        assert_eq!(
            value_words(m("auth.cli_key_ttl_days"), &json!(1), false),
            "1 day"
        );
        assert_eq!(
            value_words(m("retention.conversation_days_max"), &json!(null), false),
            "Not set"
        );
        // A secret reads "Set" or "Not set", never the value it is handed.
        assert_eq!(
            value_words(m("email.smtp_password"), &json!("hunter2"), true),
            "Set"
        );
        assert_eq!(
            value_words(m("email.smtp_password"), &json!("hunter2"), false),
            "Not set"
        );
    }

    #[test]
    fn section_fields_follow_the_registry_order() {
        let keys: Vec<_> = section_fields("agents").iter().map(|m| m.key).collect();
        assert_eq!(keys.first(), Some(&"agents.default_name"));
        assert_eq!(keys.len(), registry().section("agents").count());
    }
}
