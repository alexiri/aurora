/**
 * (P5-REFINE-2, priority 4, rec 13: "copy system with teeth") The one place that maps an enum the API returns to words.
 * One table per enum the UI shows: `Record<Key, { label; sentence? }>`. `label(table, key)` gives the label; an unknown
 * key renders "?" (never the raw key), and `labels.test.ts` walks every value of the schema enums listed in `COVERED`
 * and fails when one has no label, so a new enum value in the API cannot reach the screen as snake_case. A label is never
 * derived from its key (C-974 forbids `humanizeKey`).
 *
 * `sentence(ctx)` is the longer form for a place that has room ("Blocked because no rule allows it.").
 */
import type { TimelineItem, TimelineKind } from "@/lib/api/timeline";
import { personName } from "@/lib/workspace-name";

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

/** (P5-REFINE-15, C-1059, D-412) `SandboxView.computer.word`: the five words a person reads for the computer. */
export const COMPUTER_WORD = {
  off: { label: "Off" },
  starting: { label: "Starting" },
  on: { label: "On" },
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

/** A workspace role's choices for a select, least access first, in words ("Member", never "member"). */
export const WORKSPACE_ROLE_OPTIONS = [
  { value: "member", label: WORKSPACE_ROLE.member.label },
  { value: "approver", label: WORKSPACE_ROLE.approver.label },
  { value: "owner", label: WORKSPACE_ROLE.owner.label },
];

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

/** (P5-REFINE-14, C-1077) A schedule run's `result`, in words. "Running" is the in-flight state; the others are outcomes. */
export const RUN_RESULT = {
  running: { label: "Running" },
  answered: { label: "Answered" },
  skipped_by_gate: { label: "Skipped by the gate" },
  failed: { label: "Failed" },
  skipped: { label: "Skipped" },
} satisfies LabelTable;

/** (C-1077) A run's `reason`, in words, by the result it belongs to. Unknown reasons (a turn's error code) say nothing technical. */
export function runReason(result: string, reason?: string | null): string | null {
  if (result === "skipped_by_gate") return "nothing to report";
  if (!reason) return null;
  const words: Record<string, string> = {
    gate_failed: "the check could not run",
    unauthorized: "its owner can no longer run it",
    frozen: "the agent was paused",
    missed: "it was already running",
    paused: "the schedule was paused",
  };
  if (words[reason]) return words[reason];
  return result === "failed" ? "something went wrong" : null;
}

/** (P5-REFINE-14, C-1079) A webhook delivery's `result`, in words. Refusals say why; a repeat is "Duplicate, ignored". */
export const DELIVERY_RESULT = {
  answered: { label: "Accepted" },
  running: { label: "Accepted" },
  failed: { label: "Accepted, the run failed" },
  refused_signature: { label: "Refused: wrong signature" },
  refused_too_large: { label: "Refused: too large" },
  refused_paused: { label: "Refused: paused" },
  refused_frozen: { label: "Refused: the agent is paused" },
  refused_unauthorized: { label: "Refused: not allowed" },
  duplicate: { label: "Duplicate, ignored" },
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

/** `LinkPolicy`: one side's policy for tasks the other agent hands it (P5-REFINE-12, channels-1). */
export const LINK_POLICY = {
  ask: { label: "Ask me first" },
  allow: { label: "Allowed" },
  blocked: { label: "Blocked" },
} satisfies LabelTable;
COVERED.LinkPolicy = LINK_POLICY;

/** `peer_tasks.status` (docs/03): where a task between two agents stands, in a sentence's last words. */
export const PEER_TASK_STATUS = {
  queued: { label: "waiting its turn" },
  open: { label: "in progress" },
  completed: { label: "completed" },
  cancelled: { label: "cancelled" },
  expired: { label: "ran out of time" },
} satisfies LabelTable;

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

/** What a person filters Activity by (workspace-2), and the timeline kinds each choice asks the API for. */
export type ActivityKind = "answer" | "app" | "approval" | "automation";
export type ActivityOutcome = "answered" | "blocked" | "waited" | "failed";

/** The filter words of Activity: "Everything · Answers · Calls to apps · Approvals · Automations". */
export const ACTIVITY_KIND = {
  "": { label: "Everything" },
  answer: { label: "Answers" },
  app: { label: "Calls to apps" },
  approval: { label: "Approvals" },
  automation: { label: "Automations" },
} satisfies LabelTable;

/** The timeline kinds (`kind=` of the feed) behind each choice of the first filter. */
export const ACTIVITY_KIND_FILTER: Record<ActivityKind, readonly TimelineKind[]> = {
  answer: ["answered", "computer"],
  app: ["called"],
  approval: ["asked_before"],
  automation: ["ran_schedule", "received_webhook"],
};

/** "Any outcome · Answered · Blocked · Waited for you · Failed". */
export const ACTIVITY_OUTCOME = {
  "": { label: "Any outcome" },
  answered: { label: "Answered" },
  blocked: { label: "Blocked" },
  waited: { label: "Waited for you" },
  failed: { label: "Failed" },
} satisfies LabelTable;

/**
 * The "Any outcome" group a timeline row belongs to. The API's outcome words differ per kind, so the filter is the web's:
 * an approval (or a row waiting for a person) is "Waited for you", then Blocked (a rule, the default, a refusal, a limit),
 * then Failed, and the rest Answered.
 */
export function timelineOutcomeGroup(item: Pick<TimelineItem, "kind" | "state" | "outcome">): ActivityOutcome {
  const out = item.outcome ?? "";
  if (item.kind === "asked_before" || item.state === "waiting" || out === "not_approved") return "waited";
  if (item.state === "blocked" || out.startsWith("blocked") || out.startsWith("refused") || out === "rate_limited") return "blocked";
  if (item.state === "failed" || out === "failed") return "failed";
  return "answered";
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
/** A sentence with no named thing: "<actor> <text>". */
const plain = (text: string, weight: AuditWeight = "routine"): AuditWords => ({ sentence: (actor) => `${actor} ${text}`, weight });
/** A sentence about a person: "<actor> <verb> <person>" ("a person" when unnamed), with an optional tail. */
const aboutPerson = (verb: string, tail = "", weight: AuditWeight = "routine"): AuditWords => ({
  sentence: (actor, t) => `${actor} ${verb} ${t ?? "a person"}${tail}`,
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
    // (batch 6) A person with no earlier binding (`previous_role: null`) was added, not changed; no row is written when nothing changed.
    sentence: (actor, t, m) =>
      role(m) && m.previous_role === null
        ? `${actor} added ${t ?? "a person"} as ${role(m)}`
        : role(m)
          ? `${actor} changed ${t ?? "a person"} to ${role(m)}`
          : `${actor} changed the role of ${t ?? "a person"}`,
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
  // The row's target is the agent that got (or lost) the access, never the account: an agent is never called an account.
  "grant.create": { sentence: (actor, t) => `${actor} gave ${t ?? "an agent"} access to an account`, weight: "important" },
  "grant.delete": { sentence: (actor, t) => `${actor} took away ${t ? `${t}'s` : "an agent's"} access to an account`, weight: "routine" },
  "policy.publish": { sentence: (actor) => `${actor} published a new set of internet rules`, weight: "important" },
  "policy.rollback": { sentence: (actor) => `${actor} went back to earlier internet rules`, weight: "important" },
  "ssh_key.create": { sentence: (actor, t) => `${actor} added ${t ? `the SSH key ${t}` : "an SSH key"}`, weight: "routine" },
  "ssh_key.delete": { sentence: (actor, t) => `${actor} removed ${t ? `the SSH key ${t}` : "an SSH key"}`, weight: "important" },
  "approval.decide": { sentence: (actor) => `${actor} answered an approval`, weight: "routine" },
  "action_approval.decide": { sentence: (actor) => `${actor} answered an approval`, weight: "routine" },
  "skill.library.add": { sentence: (actor, t) => `${actor} added ${t ? `the skill library ${t}` : "a skill library"}`, weight: "routine" },
  // (P5-REFINE-18 #11, admin-10) What instance administrators did, for the Administration audit log.
  "instance_settings.update": {
    sentence: (actor, _t, m) => `${actor} changed the ${settingsAreaWords(m.section)} settings`,
    weight: "important",
  },
  "instance_settings.reset": {
    sentence: (actor, _t, m) => `${actor} put the ${settingsAreaWords(m.section)} settings back to their defaults`,
    weight: "important",
  },
  "instance_settings.import": { sentence: (actor) => `${actor} imported settings`, weight: "important" },
  "instance_settings.helm_sync": { sentence: (actor) => `${actor} synced the settings set in Helm`, weight: "routine" },
  "placement.update": { sentence: (actor) => `${actor} changed how new people are placed`, weight: "important" },
  "plugin.activate": named("approved", "the app", "an app", "important"),
  "plugin.auto_approved": named("approved", "the app", "an app"),
  "plugin.delete": named("removed", "the app", "an app", "important"),
  "plugin.hold": named("held back an update of", "the app", "an app"),
  "plugin.ingest": named("added the package of", "the app", "an app"),
  "plugin.supersede": named("replaced an older version of", "the app", "an app"),
  "plugin.host_claim.decide": { sentence: (actor) => `${actor} decided who may use an app's web address`, weight: "important" },
  "admin.connector_create": named("added", "the chat platform", "a chat platform"),
  "admin.connector_approve": named("approved an update of", "the chat platform", "a chat platform"),
  "admin.connector_rotate": named("made a new token for", "the chat platform", "a chat platform", "important"),
  "admin.connector_delete": named("removed", "the chat platform", "a chat platform", "important"),
  "admin.connector_host_token_issue": { sentence: (actor) => `${actor} made a token for a connector host`, weight: "important" },
  "admin.connector_host_token_revoke": { sentence: (actor) => `${actor} revoked a connector host token`, weight: "important" },
  "connector.created": { sentence: (actor) => `${actor} added a chat platform`, weight: "routine" },
  "connector.updated": { sentence: (actor) => `${actor} changed a chat platform`, weight: "routine" },
  "connector.update_pending": { sentence: (actor) => `${actor} found an update of a chat platform waiting for approval`, weight: "routine" },
  "admin.runner_create": named("added", "the runner", "a runner"),
  "admin.runner_rotate": named("made a new token for", "the runner", "a runner", "important"),
  "admin.runner_delete": named("removed", "the runner", "a runner", "important"),
  "runner.image": { sentence: (actor) => `${actor} changed a runner's image`, weight: "routine" },
  "trusted_key.create": { sentence: (actor) => `${actor} trusted a new app publisher`, weight: "important" },
  "trusted_key.revoke": { sentence: (actor) => `${actor} stopped trusting an app publisher`, weight: "important" },
  "addon_source.add": named("added", "the app source", "an app source"),
  "addon_source.update": named("changed", "the app source", "an app source"),
  "addon_source.remove": named("removed", "the app source", "an app source", "important"),
  "addon_source.check": { sentence: (actor) => `${actor} checked the app sources for updates`, weight: "routine" },
  "org.create": named("created", "the organization", "an organization"),
  "org.update": named("changed", "the organization", "an organization"),
  "org.delete": named("deleted", "the organization", "an organization", "important"),
  "admin.user_update": named("changed", "the person", "a person", "important"),
  "admin.org.delete": named("deleted", "the organization", "an organization", "important"),
  "admin.workspace.delete": named("deleted", "the workspace", "a workspace", "important"),
  // (P5-REFINE-20, workspace-3, admin-10) The rest of what the API writes (every action of the Rust audit call sites:
  // `tests/fixtures/audit-actions.json`), so no row ever says "did something".
  // People and sign-in
  "user.login": plain("signed in"),
  "user.logout": plain("signed out"),
  "user.signup": plain("joined Aurora"),
  "user.update": plain("changed their profile"),
  "user.delete": aboutPerson("deleted", "", "important"),
  "user.reauth": plain("signed in again to confirm it was them"),
  "user.admit_by_claim": { sentence: (_a, t) => `${t ?? "A person"} was let in by their sign-in provider`, weight: "routine" },
  "session.revoke_others": plain("signed out of every other browser"),
  "cli_login.approve": plain("approved a sign-in from the command line"),
  "cli_login.consume": plain("signed in from the command line"),
  "instance_admin.grant": aboutPerson("made", " an Aurora administrator", "important"),
  "instance_admin.revoke": aboutPerson("took the Aurora administrator role from", "", "important"),
  "org_role.idp_sync_blocked": { sentence: (_a, t) => `${t ?? "A person"}'s role from the sign-in provider was not applied`, weight: "important" },
  "user_link.create": plain("linked a chat account to their Aurora account"),
  "user_link.delete": plain("unlinked a chat account from their Aurora account"),
  "user_token.create": plain("connected a chat account so agents can send as them"),
  "user_token.grant": plain("let an agent send as them"),
  "user_token.ungrant": plain("stopped letting an agent send as them"),
  "user_token.revoke": plain("disconnected a chat account that agents sent as", "important"),
  "user_token.invalid": { sentence: () => "A chat account agents sent as stopped working", weight: "routine" },
  "user_token.send": plain("sent a message as a person, through an agent"),
  "invitation.link": aboutPerson("made a new invitation link for"),
  "invitation.resend": aboutPerson("sent the invitation again to"),
  "scim.update": plain("changed directory sync", "important"),
  "scim.error": { sentence: () => "A directory sync request failed", weight: "important" },
  "scim_token.create": plain("made a directory sync token", "important"),
  "scim_token.revoke": plain("revoked a directory sync token", "important"),
  "scim_token.expiry_notice": { sentence: () => "A directory sync token is about to expire", weight: "routine" },
  "oauth_client.create": named("added", "the sign-in app", "a sign-in app", "important"),
  "oauth_client.update": named("changed", "the sign-in app", "a sign-in app", "important"),
  "oauth_client.delete": named("removed", "the sign-in app", "a sign-in app", "important"),
  "oauth_client.test": named("tested", "the sign-in app", "a sign-in app"),
  // Teams
  "group.create": named("created", "the team", "a team"),
  "group.update": named("changed", "the team", "a team"),
  "group.delete": named("deleted", "the team", "a team", "important"),
  "group_member.add": aboutPerson("added", " to a team"),
  "group_member.remove": aboutPerson("removed", " from a team", "important"),
  "group_access.set": plain("gave a team access to a workspace", "important"),
  "group_access.remove": plain("took a team's access to a workspace away", "important"),
  // Agents
  "agent.avatar_set": named("changed the picture of", "the agent", "an agent"),
  "agent.avatar_remove": named("removed the picture of", "the agent", "an agent"),
  "agent.byo_settings.update": named("changed where agents run for", "the agent", "an agent"),
  "workspace.byo_posture.update": plain("changed whether agents may run outside Aurora in the workspace", "important"),
  "agent.context_credential_create": plain("gave an agent an account for one conversation"),
  "agent.files.write": plain("saved a file on an agent's computer"),
  "agent.files.delete": plain("deleted files on an agent's computer", "important"),
  "agent.files.rename": plain("renamed a file on an agent's computer"),
  "agent.files.mkdir": plain("made a folder on an agent's computer"),
  "agent.files.download": plain("downloaded a file from an agent's computer"),
  "agent.sandbox_retry": plain("started an agent's computer again"),
  "agent.sandbox_sleep": plain("put an agent's computer to sleep"),
  "agent.sandbox_wake": plain("woke an agent's computer"),
  "agent.ssh_certificate.mint": plain("made a sign-in certificate for an agent's computer"),
  "agent.ssh.open": plain("opened an SSH session to an agent's computer"),
  "agent.ssh.close": plain("closed an SSH session to an agent's computer"),
  "ssh_session.disconnect": plain("ended an SSH session", "important"),
  "agent.terminal.open": plain("opened a terminal on an agent's computer"),
  "agent.terminal.close": plain("closed a terminal on an agent's computer"),
  "agent_link.create": plain("linked two agents", "important"),
  "agent_link.delete": plain("unlinked two agents", "important"),
  "agent_link.direction": plain("changed which way two linked agents may ask each other", "important"),
  "agent_link.policy": plain("changed what two linked agents may ask each other", "important"),
  "reach_grant.decide": plain("decided who may reach an agent", "important"),
  "reach.requested": { sentence: (actor) => `${actor} asked to reach an agent`, weight: "routine" },
  "push_subscription.create": plain("turned on push notifications on a device"),
  "push_subscription.update": plain("refreshed push notifications on a device"),
  "push_subscription.move": plain("moved push notifications to another account"),
  "push_subscription.delete": plain("turned off push notifications on a device"),
  "push.vapid_rotate": plain("made new push notification keys", "important"),
  // Workspaces, organizations and their settings
  "workspace.update": named("changed", "the workspace", "a workspace"),
  "org.freeze": named("paused", "the organization", "an organization", "important"),
  "org.unfreeze": named("resumed", "the organization", "an organization"),
  "org.app_availability.update": plain("changed which apps the organization offers", "important"),
  "org.previews.update": plain("changed port previews for the organization", "important"),
  "governance.update": plain("changed the AI and Internet access rules", "important"),
  "retention.update": plain("changed how long data is kept", "important"),
  "policy.draft_update": plain("edited the internet rules that are not applied yet"),
  "preview.session.create": plain("opened a port preview"),
  "preview.share.create": plain("shared a port preview", "important"),
  "preview.share.start": plain("started sharing a port preview", "important"),
  "preview.share.update": plain("changed who can open a shared port preview", "important"),
  "preview.share.delete": plain("stopped sharing a port preview"),
  // Apps and accounts
  "account.test": named("tested", "the account", "an account"),
  "addon.ingest": plain("added an app package", "important"),
  "app_instance.create": named("set up", "the app", "an app"),
  "app_instance.update": named("changed", "the app", "an app"),
  "app_instance.promote": named("moved to a wider level", "the app", "an app", "important"),
  "app_instance.delete": named("removed", "the app", "an app", "important"),
  "app_setup_request.ask": plain("asked for an app to be set up"),
  "app_setup_request.decline": plain("declined a request to set up an app"),
  "app_setup_request.resolve": plain("set up an app that was asked for"),
  "app_setup_request.withdraw": plain("withdrew a request to set up an app"),
  "connection.webhook.update": named("changed the outgoing webhook of", "the account", "an account", "important"),
  "grant.converted_by_app": plain("moved access to an app's own account"),
  "plugin.approve": named("approved", "the app", "an app", "important"),
  "plugin.reject": named("rejected", "the app", "an app", "important"),
  "plugin.enable": named("turned on", "the app", "an app"),
  "plugin.disable": named("turned off", "the app", "an app", "important"),
  "secret.accept": named("accepted", "the secret", "a secret"),
  "secret.reject": named("rejected", "the secret", "a secret"),
  "secret.verify": named("checked", "the secret", "a secret"),
  "secret_stores.migrate_start": plain("started moving secrets to another store", "important"),
  "secret_stores.migrate_finish": plain("finished moving secrets to another store", "important"),
  // Chat platforms
  "channel_integration.create": named("connected", "the chat platform", "a chat platform"),
  "channel_integration.update": named("changed", "the chat platform", "a chat platform"),
  "channel_integration.delete": named("disconnected", "the chat platform", "a chat platform", "important"),
  "channel_integration.autolink_enable": plain("turned on linking people by verified e-mail on a chat platform", "important"),
  "channel_integration.autolink_disable": plain("turned off linking people by verified e-mail on a chat platform", "important"),
  "channel_presence.attach": plain("put an agent on a chat platform"),
  "channel_presence.detach": plain("took an agent off a chat platform"),
  "channel_presence.reattach": plain("put an agent back on a chat platform"),
  "channel.credential_dead": { sentence: () => "A chat platform stopped accepting its credential", weight: "important" },
  "presence.deprovision_failed": { sentence: () => "An agent could not be removed from a chat platform", weight: "important" },
  "space_binding.create": plain("connected a chat channel to an agent"),
  "space_binding.update": plain("changed how a chat channel is connected to an agent"),
  "space_binding.delete": plain("disconnected a chat channel from an agent"),
  "dm_default.set": plain("chose which agent answers their direct messages"),
  // Conversations, memory, skills, schedules, evals
  "conversation.delete": plain("deleted a conversation", "important"),
  "conversation.export": plain("exported a conversation", "important"),
  "conversation.rename": plain("renamed a conversation"),
  "conversation.owner_view": plain("read someone else's conversation", "important"),
  "transcript_event.redact": plain("erased part of a conversation", "important"),
  "attachment.delete": plain("deleted an attachment"),
  "attachment.purge": plain("removed attachments that were past their time"),
  "attachment.quota_notice": { sentence: () => "Attachment storage is nearly full", weight: "routine" },
  "approval.expire": { sentence: () => "An approval ran out of time without an answer", weight: "routine" },
  "automation.reassign": plain("handed a schedule or webhook to someone else", "important"),
  "memory.save": plain("saved a memory"),
  "memory.delete": plain("deleted a memory", "important"),
  "memory.restore": plain("brought back a memory"),
  "memory.redact": plain("erased what a memory said", "important"),
  "skill.create": named("added", "the skill", "a skill"),
  "skill.update": named("changed", "the skill", "a skill"),
  "skill.delete": named("deleted", "the skill", "a skill", "important"),
  "skill.file.put": plain("saved a file in a skill"),
  "skill.file.delete": plain("deleted a file from a skill"),
  "skill_library.create": named("added", "the skill library", "a skill library"),
  "skill_library.update": named("changed", "the skill library", "a skill library"),
  "skill_library.delete": named("removed", "the skill library", "a skill library", "important"),
  "skill.library.detach": named("detached", "the skill library", "a skill library"),
  "skill.library.update_proposed": plain("found an update of a skill library"),
  "skill.library.update_apply": plain("applied an update of a skill library"),
  "skill.library.update_reject": plain("rejected an update of a skill library"),
  "schedule.create": named("created", "the schedule", "a schedule"),
  "schedule.update": named("changed", "the schedule", "a schedule"),
  "schedule.delete": named("deleted", "the schedule", "a schedule", "important"),
  "schedule.run_now": named("ran now", "the schedule", "a schedule"),
  "webhook.update": named("changed", "the webhook", "a webhook"),
  "webhook.rotate_secret": named("made a new secret for", "the webhook", "a webhook", "important"),
  "webhook.test_sent": named("sent a test to", "the webhook", "a webhook"),
  // (glossary) "eval" is retired: the page says Tests, test run, question and grader.
  "eval.question.create": plain("added a test question"),
  "eval.question.update": plain("changed a test question"),
  "eval.question.delete": plain("deleted a test question"),
  "eval.run.start": plain("started a test run"),
  "eval.run.cancel": plain("stopped a test run"),
  "eval.run.delete": plain("deleted a test run"),
  "eval.schedule.create": plain("scheduled test runs"),
  "eval.schedule.update": plain("changed the schedule of test runs"),
  "eval.schedule.delete": plain("stopped scheduled test runs"),
  "eval.settings.update": plain("changed the grader settings", "important"),
};

/** A settings section id in words for a sentence: "terminal" → "web terminal", an unknown one reads as its id with spaces. */
const SETTINGS_AREA: Record<string, string> = {
  terminal: "web terminal",
  governance: "AI and Internet access",
  llm: "AI endpoints",
  sign_in: "sign-in",
  scim: "directory sync",
  email: "mail server",
  secrets: "secret stores",
  agents: "agent defaults",
  approvals: "approvals",
  previews: "previews",
  skills: "skills library",
  retention: "data retention",
  limits: "rate limits",
  outgoing_webhooks: "outgoing webhooks",
};
function settingsAreaWords(section: unknown): string {
  const s = typeof section === "string" ? section.replace(/-/g, "_") : "";
  return SETTINGS_AREA[s] ?? (s ? s.replace(/_/g, " ") : "instance");
}

/** Where in Administration an audit action belongs, for the line under a sentence ("Instance settings"). */
export function auditArea(action: string): string | null {
  const prefix: [string, string][] = [
    ["instance_settings.", "Instance settings"],
    ["placement.", "New users"],
    ["plugin.", "Apps"],
    ["admin.connector", "Chat platforms"],
    ["connector.", "Chat platforms"],
    ["admin.runner", "Runners"],
    ["runner.", "Runners"],
    ["admin.user", "People"],
    ["admin.org", "People"],
    ["admin.workspace", "People"],
  ];
  return prefix.find(([p]) => action.startsWith(p))?.[1] ?? null;
}

/** Install and sync events the system itself writes: hidden from the Administration audit log until asked for. */
export function isSystemAudit(e: { actor_kind: string; action: string }): boolean {
  return (
    e.actor_kind === "system" ||
    ["instance_settings.helm_sync", "plugin.ingest", "plugin.supersede", "plugin.auto_approved", "runner.image"].includes(e.action)
  );
}

/** Two actions whose sentences read alike get their own words in the filter (the list never repeats a line). */
const FILTER_LABEL: Record<string, string> = {
  "member.add": "Added a person to the organization",
  "workspace.access.grant": "Added a person to a workspace",
  "invitation.create": "Invited a person to the organization",
  "member.invite": "Sent an invitation",
  "approval.decide": "Answered an approval",
  "action_approval.decide": "Answered an app action approval",
  "skill.library.add": "Added a skill library",
  "skill_library.create": "Connected a skill library",
  "admin.user_update": "Changed a person's account",
  "plugin.approve": "Approved an app update",
  "plugin.auto_approved": "An app was approved automatically",
  "app_instance.delete": "Removed an app from a workspace",
  "connector.created": "A chat platform announced itself",
  "connector.updated": "A chat platform announced a change",
  "admin.workspace.delete": "Deleted a workspace as an administrator",
  "admin.org.delete": "Deleted an organization as an administrator",
};

/** An action in words for a filter, without a subject: "Revoked an API key", "Saved a memory". */
export function auditActionLabel(action: string): string {
  const known = FILTER_LABEL[action] ?? (AUDIT_ACTION as Record<string, { label: string } | undefined>)[action]?.label;
  if (known) return known;
  const tpl = AUDIT_SENTENCE[action];
  const text = tpl ? tpl.sentence("", null, {}).trim() : action.replace(/[._]/g, " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** Where an action belongs in the filter's list, by what it is about. */
const AUDIT_GROUPS: [string, string[]][] = [
  ["People and sign-in", ["user", "session", "cli_login", "instance_admin", "org_role", "user_link", "user_token", "invitation", "scim", "scim_token", "oauth_client", "member", "workspace_access", "workspace.access", "group", "group_access", "group_member", "admin.user"]],
  ["Agents", ["agent", "agent_link", "reach", "reach_grant", "ssh_key", "ssh_session", "preview", "api_key", "push", "push_subscription"]],
  ["Apps and accounts", ["plugin", "addon", "addon_source", "app_instance", "app_setup_request", "connect", "connection", "grant", "secret", "secret_stores", "trusted_key", "account", "policy", "approval", "action_approval"]],
  ["Chat platforms", ["channel", "channel_integration", "channel_presence", "connector", "admin.connector", "presence", "space_binding", "dm_default"]],
  ["Conversations, memory and skills", ["conversation", "transcript_event", "attachment", "memory", "skill", "skill_library", "schedule", "webhook", "eval", "automation"]],
  ["Workspaces and organizations", ["workspace", "org", "retention", "governance", "instance_settings", "placement", "admin.org", "admin.workspace", "admin.runner", "runner"]],
];

/** What only an instance administrator does: not offered on a workspace's or an organization's audit. */
const INSTANCE_ONLY = ["instance_settings.", "placement.", "plugin.", "addon", "trusted_key.", "admin.", "connector.", "runner.", "scim", "secret_stores.", "instance_admin.", "oauth_client.", "push.vapid", "org_role.", "channel.credential_dead", "user.admit_by_claim"];

export interface AuditFilterGroup {
  group: string;
  options: { value: string; label: string }[];
}

/** The action filter's choices, grouped, in words. `instance` offers every action; the others leave out the instance administrator's. */
export function auditFilterGroups(level: "workspace" | "organization" | "instance"): AuditFilterGroup[] {
  const groupOf = (action: string) => {
    const head = action.includes(".") ? action.slice(0, action.indexOf(".")) : action;
    // The longest key that names the action wins: "admin.connector" (a prefix) over "admin"; "workspace_access" (the first word).
    let best: { group: string; len: number } | null = null;
    for (const [group, keys] of AUDIT_GROUPS) {
      for (const k of keys) {
        const hit = k.includes(".") ? action.startsWith(k) : head === k;
        if (hit && (best === null || k.length > best.len)) best = { group, len: k.length };
      }
    }
    return best?.group ?? "Other";
  };
  const out = new Map<string, AuditFilterGroup>();
  for (const [g] of AUDIT_GROUPS) out.set(g, { group: g, options: [] });
  out.set("Other", { group: "Other", options: [] });
  for (const action of Object.keys(AUDIT_SENTENCE)) {
    if (level !== "instance" && INSTANCE_ONLY.some((p) => action.startsWith(p))) continue;
    out.get(groupOf(action))!.options.push({ value: action, label: auditActionLabel(action) });
  }
  return [...out.values()].filter((g) => g.options.length > 0).map((g) => ({ ...g, options: g.options.sort((a, b) => a.label.localeCompare(b.label)) }));
}

/** What the platform's own jobs are called in words (the audit's `actor_label` of a `system` actor is the job's slug). */
const MACHINE_ACTORS: Record<string, string> = {
  placement: "New-user placement",
  retention: "Data retention",
  "attachment-sweep": "Attachment clean-up",
  evals: "Scheduled test runs",
  "governance-reevaluation": "Rules re-check",
  "agent-token-max-age": "Token age limit",
  "registry-compiler": "App registry",
  "skill-library-refresh": "Skill library refresh",
  "ssh-terminator": "SSH gateway",
  "workspace-delete": "Clean-up after a deletion",
  "org-delete": "Clean-up after a deletion",
  "agent-delete": "Clean-up after a deletion",
  "addon-source-poll": "App source check",
  "app-setup-requests": "App requests",
  "channel-deprovision": "Chat platform clean-up",
  "user-token": "Chat accounts",
  "aurora-api push rotate-vapid": "Push notification keys",
  "plugin-builtin": "Built-in apps",
  "plugin-directory": "App directory",
};

/** The first word of a name ("Alice Admin" → "Alice"). */
const firstName = (full: string) => full.trim().split(/\s+/)[0] ?? full;

/** "alice.smith@example.test" → "Alice.smith": a person with no display name yet is never shown as an e-mail address. */
function nameFromAddress(label: string): string {
  const local = label.includes("@") ? (label.split("@")[0] ?? "") : label;
  return local ? `${local.charAt(0).toUpperCase()}${local.slice(1)}` : label;
}

/**
 * Who did it, in words (workspace-3, admin-10): `name` leads the sentence ("Alice"), `full` is the line under it
 * ("Alice Admin"). People are named by their display name (`actor_name`), never by e-mail; Aurora's own jobs are "Aurora"
 * with the job in words; a key says so; no dotted or dashed slug is ever shown.
 */
export function auditActor(e: { actor_kind?: string; actor_label: string; actor_name?: string | null }): { name: string; full: string } {
  const kind = e.actor_kind ?? "user";
  if (kind === "system") return { name: "Aurora", full: MACHINE_ACTORS[e.actor_label] ?? (e.actor_label.startsWith("connector:") ? "A chat platform" : "Aurora") };
  const person = e.actor_name?.trim() || (e.actor_label ? nameFromAddress(e.actor_label) : "Someone");
  if (kind === "service") {
    // A key acts as its owner (an address label); the directory-sync and sign-in-provider services have labels of their own.
    if (e.actor_label.startsWith("scim")) return { name: "Directory sync", full: "Directory sync" };
    if (e.actor_label.startsWith("idp:")) return { name: "The sign-in provider", full: "The sign-in provider" };
    return e.actor_name?.trim() ? { name: firstName(person), full: `${person} (through an API key)` } : { name: "An API key", full: "An API key" };
  }
  if (kind === "agent") return { name: e.actor_label || "An agent", full: `${e.actor_label || "An agent"} (agent)` };
  return { name: firstName(person), full: person };
}

/** The words of one audit entry: the sentence and how much it matters. An action with no template says so, plainly. */
export function auditWords(
  e: { action: string; actor_label: string; actor_kind?: string; actor_name?: string | null; critical?: boolean; metadata?: unknown },
  target: string | null,
): { sentence: string; weight: "critical" | AuditWeight; known: boolean } {
  const meta = e.metadata && typeof e.metadata === "object" && !Array.isArray(e.metadata) ? (e.metadata as Record<string, unknown>) : {};
  const raw = target ?? (typeof meta.name === "string" ? meta.name : typeof meta.email === "string" ? meta.email : null);
  // A person with no display name is stored by their address: never an e-mail where a name is shown (the switcher's rule).
  const t = raw !== null && /^[^\s@]+@[^\s@]+$/.test(raw) ? personName(raw) : raw;
  const tpl = AUDIT_SENTENCE[e.action];
  const actor = auditActor(e).name;
  if (!tpl) {
    const readable = (AUDIT_ACTION as Record<string, { label: string } | undefined>)[e.action]?.label ?? e.action.replace(/[._]/g, " ");
    return { sentence: `${actor} did something (“${readable}”)`, weight: e.critical ? "critical" : "routine", known: false };
  }
  return { sentence: tpl.sentence(actor, t, meta), weight: e.critical ? "critical" : tpl.weight, known: true };
}

/** (P5-REFINE-13, access-9) What an AI endpoint's models are, in one phrase, by its provider kind. */
const SERVICE_MODELS: Record<string, string> = {
  anthropic: "Claude models",
  openai: "GPT models",
  azure_openai: "GPT models on Azure",
  vertex: "Gemini and Claude models on Google Cloud",
  bedrock: "Claude models on AWS",
  litellm: "Your organization's own gateway",
  openai_compatible: "Your organization's own gateway",
};

/** "Claude models. Works with Claude Code." under a service's name in the picker: one plain line. */
export function serviceLine(provider: string, harness: string): string {
  const line = [SERVICE_MODELS[provider], harness].filter(Boolean).join(". ");
  return line ? `${line}.` : "";
}

/**
 * (P5-REFINE-13, access-5) A webhook test that did not go through, in one plain sentence: by the status the receiver
 * answered. 2xx is a success and has no sentence.
 */
export function webhookTestSentence(status: number): string {
  if (status === 405) return "The address doesn't accept POST. Check the URL and try again.";
  if (status === 404 || status === 410) return "Nothing is there. Check the address.";
  if (status === 401 || status === 403) return "The address refused the request. Check the sender's settings.";
  if (status >= 500) return "The other side had a problem. Try again in a minute.";
  return "The address did not accept the message. Check the URL and try again.";
}

/** The same for a test that never got an answer (the API's error codes). `null`: not a delivery problem. */
export function webhookTestErrorSentence(code: string | undefined): string | null {
  switch (code) {
    case "webhook_timeout":
      return "It took too long to answer.";
    case "link_unreachable":
    case "link_destination_refused":
      return "Couldn't reach the address.";
    case "link_not_found":
      return "Nothing is there. Check the address.";
    default:
      return null;
  }
}

// ---- The agent timeline (P5-REFINE-9 feed; P5-REFINE-11 rail, P5-REFINE-16 Activity; D-415) ----------------------------

/** The kinds of timeline row (`TimelineKind`) in the filter's words (workspace-2: "Everything · Answers · Calls to apps · Approvals · Automations"). */
export const TIMELINE_KIND = {
  answered: { label: "Answers" },
  computer: { label: "On its computer" },
  asked_before: { label: "Approvals" },
  called: { label: "Calls to apps" },
  ran_schedule: { label: "Automations" },
  received_webhook: { label: "Automations" },
  handed_task: { label: "Other agents" },
  got_task: { label: "Other agents" },
  asked_for_access: { label: "Access requests" },
} satisfies LabelTable;
COVERED.TimelineKind = TIMELINE_KIND;

/** Where a row is now (`TimelineState`), in words. */
export const TIMELINE_STATE = {
  running: { label: "Working" },
  waiting: { label: "Waiting for you" },
  done: { label: "Done" },
  failed: { label: "Failed" },
  blocked: { label: "Blocked" },
  skipped: { label: "Skipped" },
} satisfies LabelTable;
COVERED.TimelineState = TIMELINE_STATE;

/** The outcome words C-1083 lists per kind, as the meta line says them. `labels.test.ts` walks every one. */
export const TIMELINE_OUTCOME_LINE: Record<string, Record<string, string>> = {
  answered: { answered: "Answered", failed: "It didn't answer", stopped: "You stopped it" },
  computer: { ok: "It worked", failed: "It didn't work" },
  asked_before: {
    allowed_once: "Allowed once",
    always_allowed: "Always allowed",
    blocked: "Blocked",
    expired: "Nobody answered in time",
    abandoned: "The request went away before anyone answered",
  },
  called: {
    allowed: "Allowed",
    blocked_by_rule: "Blocked by a rule",
    blocked_default: "Blocked: no rule allows it",
    not_approved: "Not approved",
    rate_limited: "Slowed down: too many requests",
    failed: "It didn't work",
  },
  ran_schedule: { answered: "Answered", skipped_by_gate: "Skipped: its condition wasn't met", failed: "It didn't work", skipped: "Skipped" },
  received_webhook: {
    answered: "Answered",
    failed: "It didn't work",
    refused_signature: "Refused: the signature didn't match",
    refused_too_large: "Refused: too large",
    refused_paused: "Refused: the webhook is paused",
    refused_frozen: "Refused: the agent is paused",
    refused_unauthorized: "Refused: not authorized",
    duplicate: "Already received",
  },
  handed_task: { completed: "Done", cancelled: "Cancelled", expired: "Expired" },
  got_task: { completed: "Done", cancelled: "Cancelled", expired: "Expired" },
  asked_for_access: { given: "Access given", dismissed: "Dismissed", expired: "Expired" },
};

export interface TimelineWords {
  /** "Dani called GitHub" */
  title: string;
  /** "Blocked by your rule ‘Block evil.example.org’" */
  meta: string;
  /** The dot at the start of a row. */
  tone: "accent" | "danger" | "pending" | "neutral";
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/**
 * One timeline row as a sentence and a meta line (D-415: the API returns kinds and names, the web writes the words). A
 * running row reads as in progress ("Running ‘Morning summary’…") and turns into its outcome in place when the stream
 * upserts the same `id`.
 */
export function timelineWords(item: TimelineItem): TimelineWords {
  const agent = item.agent.name;
  const running = item.state === "running";
  const waiting = item.state === "waiting";
  const out = item.outcome ?? "";
  const outLine = TIMELINE_OUTCOME_LINE[item.kind]?.[out] ?? (item.state === "failed" ? "It didn't work" : "");
  const tone: TimelineWords["tone"] = running
    ? "accent"
    : waiting
      ? "pending"
      : item.state === "failed" || item.state === "blocked"
        ? "danger"
        : item.state === "skipped"
          ? "neutral"
          : "accent";
  const thing = item.app?.name ?? item.host ?? "something";
  const calls = item.count != null && item.count > 0 ? plural(item.count, "call", "calls") : null;
  switch (item.kind) {
    case "answered": {
      const parts = [item.model ? `Used ${item.model}` : null, calls].filter(Boolean);
      return {
        title: running ? `${agent} is answering` : out === "answered" || !out ? `${agent} answered you` : out === "stopped" ? `${agent} was stopped` : `${agent} couldn't answer`,
        meta: running ? parts.join(" · ") || "Working on it" : out === "answered" || !out ? parts.join(" · ") || "Answered" : outLine,
        tone: out === "stopped" ? "neutral" : tone,
      };
    }
    case "computer": {
      const n = item.files?.length ?? 0;
      const files = item.count ?? n;
      const title = item.command
        ? `${agent} ran ‘${item.command}’ on its computer`
        : n === 1
          ? `${agent} wrote ${item.files![0]}`
          : files > 0
            ? `${agent} wrote ${plural(files, "file", "files")}`
            : `${agent} ran something on its computer`;
      const meta = item.command && files > 0 ? `Wrote ${n === 1 ? item.files![0] : plural(files, "file", "files")}` : out === "failed" ? outLine : "On its computer";
      return { title, meta, tone };
    }
    case "asked_before":
      return {
        title: `${agent} asked before changing ${item.host ?? item.app?.name ?? "something"}`,
        meta: waiting ? "Waiting for you in Approvals" : item.person && (out === "allowed_once" || out === "always_allowed" || out === "blocked") ? `${outLine} by ${item.person.display_name}` : outLine,
        tone,
      };
    case "called": {
      const rule = item.rule?.name;
      const meta =
        out === "blocked_by_rule" && rule ? `Blocked by your rule ‘${rule}’` : out === "allowed" ? [outLine, calls && item.count! > 1 ? calls : null].filter(Boolean).join(" · ") : outLine;
      return { title: `${agent} called ${thing}`, meta, tone };
    }
    case "ran_schedule": {
      const name = item.schedule?.name ?? "a schedule";
      return { title: running ? `Running ‘${name}’…` : `Ran ‘${name}’`, meta: running ? `${agent} is on it` : outLine, tone };
    }
    case "received_webhook": {
      const name = item.webhook?.name ?? "a webhook";
      return { title: running ? `Handling ‘${name}’…` : `Received ‘${name}’`, meta: running ? `${agent} is on it` : outLine, tone };
    }
    case "handed_task": {
      const peer = item.peer?.name ?? "another agent";
      return { title: `${agent} asked ${peer} for help`, meta: running || waiting ? `Waiting for ${peer}` : outLine, tone };
    }
    case "got_task": {
      const peer = item.peer?.name ?? "another agent";
      return { title: `${peer} asked ${agent} for help`, meta: running || waiting ? `${agent} is on it` : outLine, tone };
    }
    case "asked_for_access":
      return { title: `${agent} asked for access to ${thing}`, meta: waiting ? "Waiting for you" : outLine, tone };
  }
}
