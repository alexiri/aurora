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
