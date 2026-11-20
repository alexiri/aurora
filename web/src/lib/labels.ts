/**
 * (P5-REFINE-2, priority 4, rec 13: "copy system with teeth") The one place that maps an enum the API returns to words.
 * One table per enum the UI shows: `Record<Key, { label; sentence? }>`. `label(table, key)` gives the label; an unknown
 * key renders "?" (never the raw key), and `labels.test.ts` walks every value of the schema enums listed in `COVERED`
 * and fails when one has no label, so a new enum value in the API cannot reach the screen as snake_case. A label is never
 * derived from its key (C-974 forbids `humanizeKey`).
 *
 * `sentence(ctx)` is the longer form for a place that has room ("Blocked because no rule allows it.").
 */
export interface LabelEntry<Ctx = void> {
  label: string;
  sentence?: (ctx: Ctx) => string;
}
export type LabelTable<Key extends string = string, Ctx = void> = Record<Key, LabelEntry<Ctx>>;

/** The label of `key` in `table`; "?" when the key is unknown (and a warning in development). */
export function label<Key extends string>(table: Record<Key, LabelEntry<never>>, key: string | null | undefined): string {
  const entry = key ? (table as Record<string, LabelEntry<never> | undefined>)[key] : undefined;
  if (!entry) {
    if (key && process.env.NODE_ENV === "development") console.warn(`labels: no label for "${key}"`);
    return "?";
  }
  return entry.label;
}

/** Gateway decisions of the request log (`request_log.decision`, docs/03). */
export const GATEWAY_DECISION = {
  allow: { label: "Allowed" },
  approved: { label: "Approved" },
  blocked: { label: "Blocked" },
  blocked_by_policy: { label: "Blocked by policy" },
  default_denied: { label: "Not allowed", sentence: () => "Blocked because no rule allows it." },
  egress_denied: { label: "Outside internet access" },
  governance_refused: { label: "Refused by the organization" },
  rate_limited: { label: "Slowed down", sentence: () => "Too many requests in a short time." },
  approval_denied: { label: "Denied by an approver" },
  approval_expired: { label: "Approval expired" },
  guard_refused: { label: "Refused by a safety check" },
  error: { label: "Error" },
} satisfies LabelTable;

/** The computer's phase (`SandboxView.state`); the raw phase stays visible for Expert (brief 15). */
export const SANDBOX_PHASE = {
  unprovisioned: { label: "Off" },
  stopped: { label: "Off" },
  starting: { label: "Starting" },
  running: { label: "On" },
  stopping: { label: "Stopping" },
  failed: { label: "Failed" },
} satisfies LabelTable;

/** `ReachState` of a request to reach a person or agent. */
export const REACH_STATE = {
  pending: { label: "Waiting" },
  approved: { label: "Approved" },
  blocked: { label: "Blocked" },
  left: { label: "Left" },
  expired: { label: "Expired" },
} satisfies LabelTable;

/** An approval's decision. */
export const APPROVAL_STATE = {
  approve: { label: "Approved" },
  deny: { label: "Denied" },
  pending: { label: "Waiting" },
  expired: { label: "Expired" },
} satisfies LabelTable;

/** `HealthState` of a connected account. */
export const ACCOUNT_HEALTH = {
  sign_in_expired: { label: "Sign in again" },
  disabled: { label: "Turned off" },
  failing: { label: "Failing" },
  working: { label: "Working" },
  not_used: { label: "Not used yet" },
} satisfies LabelTable;

/** `AccountTestResult`: the outcome of testing an account. */
export const ACCOUNT_TEST = {
  ok: { label: "Works" },
  rejected: { label: "Rejected" },
  unreachable: { label: "Can't be reached" },
  unexpected: { label: "Unexpected answer" },
  not_testable: { label: "Can't be tested" },
} satisfies LabelTable;

/** Organization roles (`OrgRole`). */
export const ORG_ROLE = {
  owner: { label: "Owner" },
  admin: { label: "Admin" },
  member: { label: "Member" },
} satisfies LabelTable;

/** Workspace roles (`PersonalRole`). */
export const WORKSPACE_ROLE = {
  owner: { label: "Owner" },
  approver: { label: "Approver" },
  member: { label: "Member" },
} satisfies LabelTable;

/** `MemberStatus` and `AccessStatus`. */
export const MEMBER_STATUS = {
  active: { label: "Active" },
  suspended: { label: "Suspended" },
  pending: { label: "Waiting for first sign-in" },
} satisfies LabelTable;

/** `InvitationStatus`. */
export const INVITATION_STATUS = {
  pending: { label: "Waiting" },
  accepted: { label: "Accepted" },
  cancelled: { label: "Cancelled" },
  expired: { label: "Expired" },
} satisfies LabelTable;

/** `ConnectorStatus`. */
export const CONNECTOR_STATUS = {
  awaiting_registration: { label: "Waiting for the connector" },
  pending_approval: { label: "Needs approval" },
  active: { label: "Working" },
  disabled: { label: "Turned off" },
} satisfies LabelTable;

/** `IntegrationStatus` of a chat platform. */
export const INTEGRATION_STATUS = {
  validating: { label: "Checking" },
  active: { label: "Working" },
  needs_attention: { label: "Needs attention" },
  disabled: { label: "Turned off" },
} satisfies LabelTable;

/** `PushState` of a browser or phone that gets notifications. */
export const PUSH_STATE = {
  active: { label: "On" },
  paused: { label: "Paused" },
  disabled: { label: "Turned off" },
  expired: { label: "Expired" },
  stale_key: { label: "Needs setting up again" },
} satisfies LabelTable;

/** `AgentAccessLevel`: what an agent may do with an account. */
export const ACCESS_LEVEL = {
  read: { label: "Look but don't touch" },
  ask: { label: "Ask me first" },
  act: { label: "Go ahead" },
  custom: { label: "Custom" },
  use: { label: "Use" },
} satisfies LabelTable;

/** `LinkMethod`: how a person's chat account was linked. */
export const LINK_METHOD = {
  verified_email: { label: "Verified e-mail" },
  sign_in_link: { label: "Sign-in link" },
  admin: { label: "Set by an admin" },
} satisfies LabelTable;

/** Harness display names; `fake` is the test harness and is hidden outside development builds. */
export const HARNESS = {
  claude_code: { label: "Claude Code" },
  "claude-code": { label: "Claude Code" },
  codex: { label: "Codex" },
  pi: { label: "Pi" },
  goose: { label: "goose" },
  opencode: { label: "OpenCode" },
  fake: { label: "Test harness" },
} satisfies LabelTable;

/** Whether a harness is shown at all: `fake` only in development. */
export const harnessVisible = (id: string): boolean => id !== "fake" || process.env.NODE_ENV === "development";

/** Webhook and schedule outcomes (brief 1's words). */
export const RUN_OUTCOME = {
  started: { label: "Started a conversation" },
  skipped: { label: "Skipped" },
  failed: { label: "Failed" },
  rejected: { label: "Refused" },
  duplicate: { label: "Already handled" },
} satisfies LabelTable;

/** Why a webhook or schedule is turned off (`disabled_reason`), after "Disabled". */
export const DISABLED_REASON = {
  user: { label: "turned off by you" },
  authorization: { label: "its owner lost access" },
  failures: { label: "too many failures" },
} satisfies LabelTable;

/** "Enabled", or "Disabled (<why>)" in words. */
export function enabledText(enabled: boolean, reason?: string | null): string {
  if (enabled) return "Enabled";
  return reason ? `Disabled (${label(DISABLED_REASON, reason)})` : "Disabled";
}

/** Webhook verification kinds (how the sender proves who it is). */
export const VERIFICATION_KIND = {
  hmac: { label: "Signed with a secret" },
  bearer: { label: "Bearer token" },
  none: { label: "None" },
} satisfies LabelTable;

/** Audit actions (`api_key.revoke` → "Revoked an API key"): the ones the audit pages show by name. */
export const AUDIT_ACTION = {
  "api_key.create": { label: "Created an API key" },
  "api_key.revoke": { label: "Revoked an API key" },
  "agent.create": { label: "Created an agent" },
  "agent.delete": { label: "Deleted an agent" },
  "agent.update": { label: "Changed an agent" },
  "workspace.create": { label: "Created a workspace" },
  "workspace.delete": { label: "Deleted a workspace" },
  "member.invite": { label: "Invited a person" },
  "member.remove": { label: "Removed a person" },
  "member.role_change": { label: "Changed a role" },
  "webhook.create": { label: "Created a webhook" },
  "webhook.delete": { label: "Deleted a webhook" },
  "webhook.rotate": { label: "Rotated a webhook URL" },
  "connect.exchange": { label: "Connected an app" },
} satisfies LabelTable;

/**
 * The schema enums these tables cover, for `labels.test.ts`: every value of each type in `lib/api/schema.d.ts` must be a
 * key of its table.
 */
export const COVERED: Record<string, Record<string, LabelEntry<never>>> = {
  ReachState: REACH_STATE,
  HealthState: ACCOUNT_HEALTH,
  AccountTestResult: ACCOUNT_TEST,
  OrgRole: ORG_ROLE,
  PersonalRole: WORKSPACE_ROLE,
  MemberStatus: MEMBER_STATUS,
  AccessStatus: MEMBER_STATUS,
  InvitationStatus: INVITATION_STATUS,
  ConnectorStatus: CONNECTOR_STATUS,
  IntegrationStatus: INTEGRATION_STATUS,
  PushState: PUSH_STATE,
  AgentAccessLevel: ACCESS_LEVEL,
  LinkMethod: LINK_METHOD,
  Decision: APPROVAL_STATE,
};

/** (P5-REFINE-11, chat-7) The one plain sentence for a chat error with no copy of its own; the technical line goes under Details. */
export const SOMETHING_WENT_WRONG = "Something went wrong. Try again.";

/** `SimVerdict` of "Check what would happen": the bold lead of the answer (workspace-1). */
export const SIM_VERDICT = {
  allow: { label: "Allowed" },
  block: { label: "Blocked" },
  needs_approval: { label: "Asks first" },
  rate_limited: { label: "Slowed down" },
} satisfies LabelTable;
COVERED.SimVerdict = SIM_VERDICT;

/** `SimReason`: why the verdict came out as it did, after the lead, in a sentence (`rule` names the rule). */
export const SIM_REASON = {
  rule: { label: "A rule decided" },
  default_denied: { label: "No rule allows it" },
  egress_denied: { label: "Outside internet access" },
  governance: { label: "Your organization decides" },
  open_lane: { label: "Browser connection" },
} satisfies LabelTable;
COVERED.SimReason = SIM_REASON;

export interface SimWords {
  /** The bold lead ("Blocked." / "Dani will ask you first."). */
  lead: string;
  /** The rest of the first line; `ruleName` is shown in bold between `before` and `after`. */
  before: string;
  ruleName: string | null;
  after: string;
  /** The small second line, or null. */
  second: string | null;
}

/** The sentences of one check (workspace-1), from the verdict, the reason and the rule that matched. */
export function simWords(
  sim: { verdict: keyof typeof SIM_VERDICT; reason?: keyof typeof SIM_REASON | null },
  rule: string | null,
  agent: string,
): SimWords {
  const lead = sim.verdict === "needs_approval" ? `${agent} will ask you first` : label(SIM_VERDICT, sim.verdict);
  const none = (before: string, second: string | null = null): SimWords => ({ lead: `${lead}.`, before, ruleName: null, after: "", second });
  const byRule = (before: string, after: string, second: string | null = null): SimWords =>
    rule ? { lead: `${lead}.`, before, ruleName: rule, after, second } : none(" No rule applies to it.", second);
  switch (sim.verdict) {
    case "block":
      if (sim.reason === "default_denied") return none(" No rule allows it, and the default for sites without an account is to block.");
      if (sim.reason === "egress_denied") return none(" Internet access for this agent doesn't include this website.");
      if (sim.reason === "governance") return none(" Your organization doesn't allow it.");
      return byRule(" Your rule ", " stops it.", `${agent} can't reach this website, and nothing will ask you first.`);
    case "allow":
      if (sim.reason === "open_lane") return none(" Browser connections are open to any website.");
      if (sim.reason === "rule") return byRule(" Your rule ", " allows it.");
      return none(" No rule stops it, so it goes through.");
    case "needs_approval":
      return byRule(" Your rule ", " asks first.", `Nothing happens until you answer in Approvals.`);
    case "rate_limited":
      return byRule(" Your rule ", " limits how often this can happen.", `${agent} is slowed down, not stopped.`);
  }
}

/** What one request-log row is, for the person's view of Activity (workspace-2). */
export type ActivityKind = "answer" | "app" | "approval" | "web";
export type ActivityOutcome = "answered" | "blocked" | "waited" | "failed";

/** The filter words of Activity: "Everything · Answers · Calls to apps · Approvals". */
export const ACTIVITY_KIND = {
  "": { label: "Everything" },
  answer: { label: "Answers" },
  app: { label: "Calls to apps" },
  approval: { label: "Approvals" },
} satisfies LabelTable;

/** "Any outcome · Answered · Blocked · Waited for you · Failed". */
export const ACTIVITY_OUTCOME = {
  "": { label: "Any outcome" },
  answered: { label: "Answered" },
  blocked: { label: "Blocked" },
  waited: { label: "Waited for you" },
  failed: { label: "Failed" },
} satisfies LabelTable;

interface ActivityRow {
  method: string;
  host: string;
  path: string;
  decision: string;
  status?: number | null;
  rule_name?: string | null;
  app_id?: string | null;
  llm_model?: string | null;
  approval_id?: string | null;
}

const BLOCKED = new Set(["blocked", "blocked_by_policy", "default_denied", "egress_denied", "governance_refused", "rate_limited", "guard_refused"]);

/** A model call goes to the model endpoint (`…/v1/messages`, `…/chat/completions`) or names the model it asked for. */
export function activityKind(e: ActivityRow): ActivityKind {
  if (e.approval_id || e.decision.startsWith("approval_")) return "approval";
  if (e.app_id) return "app";
  if (e.llm_model || /\/v1\/(messages|chat\/completions|responses)\b/.test(e.path)) return "answer";
  return "web";
}

export function activityOutcome(e: ActivityRow): ActivityOutcome {
  if (e.decision === "error" || e.decision === "approval_expired") return "failed";
  if (e.approval_id || e.decision === "approval_denied") return "waited";
  if (BLOCKED.has(e.decision)) return "blocked";
  return "answered";
}

/** The tone of the dot at the start of a row. */
export function activityTone(e: ActivityRow): "accent" | "danger" | "pending" | "neutral" {
  const o = activityOutcome(e);
  return o === "answered" ? "accent" : o === "blocked" ? "danger" : o === "waited" ? "pending" : "neutral";
}

/** The two lines of a row: the sentence and the meta line ("Blocked by your rule 'X'"). `appName` names the app called. */
export function activitySentence(e: ActivityRow, agent: string, appName?: string | null): { title: string; meta: string } {
  const kind = activityKind(e);
  const outcome = activityOutcome(e);
  const entry = (GATEWAY_DECISION as Record<string, { sentence?: () => string } | undefined>)[e.decision];
  const why = e.rule_name ? `Blocked by your rule ‘${e.rule_name}’` : (entry?.sentence?.() ?? "Blocked");
  if (kind === "answer") {
    return outcome === "answered"
      ? { title: `${agent} answered you`, meta: `Used ${e.llm_model ?? "the workspace's model"}` }
      : { title: `${agent} couldn't get an answer`, meta: outcome === "blocked" ? why : "The model didn't answer" };
  }
  if (kind === "approval") {
    const meta =
      e.decision === "approved"
        ? "You approved it"
        : e.decision === "approval_denied"
          ? "You said no"
          : e.decision === "approval_expired"
            ? "Nobody answered in time"
            : "Waiting for you in Approvals";
    return { title: `${agent} asked before changing ${e.host}`, meta };
  }
  if (kind === "app") {
    const app = appName ?? "an app";
    return outcome === "blocked"
      ? { title: `${agent} called ${app}`, meta: why }
      : outcome === "failed"
        ? { title: `${agent} called ${app}`, meta: "It didn't work" }
        : { title: `${agent} called ${app}`, meta: "Allowed" };
  }
  return outcome === "blocked"
    ? { title: `${agent} tried to reach ${e.host}`, meta: why }
    : outcome === "failed"
      ? { title: `${agent} tried to reach ${e.host}`, meta: "It didn't work" }
      : { title: `${agent} reached ${e.host}`, meta: "Allowed" };
}

/** How much an audit entry matters to a workspace owner; `critical` rows are Critical whatever the table says. */
export type AuditWeight = "important" | "routine";

interface AuditWords {
  /** "revoked the API key <name>" / "revoked an API key"; the person's name leads. */
  sentence: (actor: string, target: string | null, meta: Record<string, unknown>) => string;
  weight: AuditWeight;
}

/** `verb` + the thing named, or `verb` + an unnamed thing when the target is unknown or gone. */
const named = (verb: string, noun: string, gone: string, weight: AuditWeight = "routine"): AuditWords => ({
  sentence: (actor, t) => (t ? `${actor} ${verb} ${noun} ${t}` : `${actor} ${verb} ${gone}`),
  weight,
});
const role = (m: Record<string, unknown>) => {
  const r = typeof m.role === "string" ? m.role : null;
  return r && r in WORKSPACE_ROLE ? WORKSPACE_ROLE[r as keyof typeof WORKSPACE_ROLE].label : null;
};

/**
 * One sentence template per audit action the workspace Audit page shows (workspace-3): "Bob revoked the API key ci-script".
 * An action with no template renders "<Person> did something (“<action>”)" and fails `labels.test.ts`, so it gets noticed.
 */
export const AUDIT_SENTENCE: Record<string, AuditWords> = {
  "api_key.create": named("created", "the API key", "an API key"),
  "api_key.revoke": named("revoked", "the API key", "a deleted API key", "important"),
  "agent.create": named("created", "the agent", "an agent"),
  "agent.delete": named("deleted", "the agent", "an agent", "important"),
  "agent.update": named("changed", "the agent", "an agent"),
  "agent.freeze": named("paused", "the agent", "an agent", "important"),
  "agent.unfreeze": named("resumed", "the agent", "an agent"),
  "agent.token_regenerate": named("made a new token for", "the agent", "an agent", "important"),
  "workspace.create": named("created", "the workspace", "a workspace"),
  "workspace.delete": named("deleted", "the workspace", "a workspace", "important"),
  "workspace.freeze": named("paused", "the workspace", "the workspace", "important"),
  "workspace.unfreeze": named("resumed", "the workspace", "the workspace"),
  "workspace_access.set": {
    sentence: (actor, t, m) => (role(m) ? `${actor} changed ${t ?? "a person"} to ${role(m)}` : `${actor} changed the role of ${t ?? "a person"}`),
    weight: "important",
  },
  "workspace_access.remove": { sentence: (actor, t) => `${actor} removed ${t ?? "a person"} from the workspace`, weight: "important" },
  "workspace_access.suspend": { sentence: (actor, t) => `${actor} suspended ${t ?? "a person"}`, weight: "important" },
  "workspace_access.restore": { sentence: (actor, t) => `${actor} restored ${t ?? "a person"}`, weight: "routine" },
  "workspace.access.grant": {
    sentence: (actor, t, m) => `${actor} added ${t ?? "a person"}${role(m) ? ` as ${role(m)}` : ""}`,
    weight: "routine",
  },
  "workspace.access.pending_create": { sentence: (actor, t) => `${actor} added ${t ?? "a person"}, who has not signed in yet`, weight: "routine" },
  "workspace.access.pending_apply": { sentence: (_a, t) => `${t ?? "A person"} signed in for the first time`, weight: "routine" },
  "workspace.access.pending_delete": { sentence: (actor, t) => `${actor} withdrew the invitation of ${t ?? "a person"}`, weight: "routine" },
  "member.add": { sentence: (actor, t, m) => `${actor} added ${t ?? "a person"}${role(m) ? ` as ${role(m)}` : ""}`, weight: "routine" },
  "member.remove": { sentence: (actor, t) => `${actor} removed ${t ?? "a person"}`, weight: "important" },
  "member.update": { sentence: (actor, t) => `${actor} changed ${t ?? "a person"}`, weight: "important" },
  "member.invite": { sentence: (actor, t) => `${actor} invited ${t ?? "a person"}`, weight: "routine" },
  "member.role_change": { sentence: (actor, t, m) => `${actor} changed ${t ?? "a person"}${role(m) ? ` to ${role(m)}` : "'s role"}`, weight: "important" },
  "invitation.create": { sentence: (actor, t) => `${actor} invited ${t ?? "a person"}`, weight: "routine" },
  "invitation.accept": { sentence: (_a, t) => `${t ?? "A person"} accepted an invitation`, weight: "routine" },
  "invitation.cancel": { sentence: (actor, t) => `${actor} cancelled the invitation of ${t ?? "a person"}`, weight: "routine" },
  "webhook.create": named("created", "the webhook", "a webhook"),
  "webhook.delete": named("deleted", "the webhook", "a webhook", "important"),
  "webhook.rotate": named("made a new address for", "the webhook", "a webhook", "important"),
  "connect.exchange": named("connected", "the app", "an app"),
  "connection.create": named("connected", "the account", "an account"),
  "connection.delete": named("removed", "the account", "an account", "important"),
  "connection.update": named("changed", "the account", "an account"),
  "connection.disable": named("turned off", "the account", "an account", "important"),
  "connection.enable": named("turned on", "the account", "an account"),
  "connection.reconnect": named("signed in again to", "the account", "an account"),
  "secret.create": named("added", "the secret", "a secret"),
  "secret.update": named("changed", "the secret", "a secret"),
  "secret.value_update": named("changed the value of", "the secret", "a secret", "important"),
  "secret.delete": named("deleted", "the secret", "a secret", "important"),
  "grant.create": named("gave access to", "the account", "an account", "important"),
  "grant.delete": named("took back access to", "the account", "an account"),
  "policy.publish": { sentence: (actor) => `${actor} published a new set of internet rules`, weight: "important" },
  "policy.rollback": { sentence: (actor) => `${actor} went back to earlier internet rules`, weight: "important" },
  "ssh_key.create": { sentence: (actor, t) => `${actor} added ${t ? `the SSH key ${t}` : "an SSH key"}`, weight: "routine" },
  "ssh_key.delete": { sentence: (actor, t) => `${actor} removed ${t ? `the SSH key ${t}` : "an SSH key"}`, weight: "important" },
  "approval.decide": { sentence: (actor) => `${actor} answered an approval`, weight: "routine" },
  "action_approval.decide": { sentence: (actor) => `${actor} answered an approval`, weight: "routine" },
  "skill.library.add": { sentence: (actor, t) => `${actor} added ${t ? `the skill library ${t}` : "a skill library"}`, weight: "routine" },
};

/** The words of one audit entry: the sentence and how much it matters. An action with no template says so, plainly. */
export function auditWords(
  e: { action: string; actor_label: string; critical?: boolean; metadata?: unknown },
  target: string | null,
): { sentence: string; weight: "critical" | AuditWeight; known: boolean } {
  const meta = e.metadata && typeof e.metadata === "object" && !Array.isArray(e.metadata) ? (e.metadata as Record<string, unknown>) : {};
  const t = target ?? (typeof meta.name === "string" ? meta.name : typeof meta.email === "string" ? meta.email : null);
  const tpl = AUDIT_SENTENCE[e.action];
  if (!tpl) {
    const readable = (AUDIT_ACTION as Record<string, { label: string } | undefined>)[e.action]?.label ?? e.action.replace(/[._]/g, " ");
    return { sentence: `${e.actor_label} did something (“${readable}”)`, weight: e.critical ? "critical" : "routine", known: false };
  }
  return { sentence: tpl.sentence(e.actor_label, t, meta), weight: e.critical ? "critical" : tpl.weight, known: true };
}
