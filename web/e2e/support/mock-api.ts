import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import type { Page } from "@playwright/test";

import { withWords } from "./settings-words";

// Shared API mock for the stage-4 enforcement specs. No backend is needed. The data uses the REAL
// built-in packages (names from plugins/builtin/*/plugin.yaml, icons from their assets/): never
// generic stand-ins.

const BUILTIN = join(__dirname, "..", "..", "..", "plugins", "builtin");

export const now = new Date().toISOString();
export const later = new Date(Date.now() + 3_600_000).toISOString();
export const id = (n: number) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;

export const org = { id: id(1), slug: "acme", name: "Acme Labs", my_role: "owner", app_posture: "open", external_id: "x", settings: { previews_enabled: true }, created_at: now };
export const ws = { id: id(2), org_id: id(1), slug: "main", name: "Main workspace", my_role: "owner", memory_language: "en", timezone: "UTC", created_at: now, updated_at: now };
export const agent = {
  id: id(11), org_id: id(1), workspace_id: id(2), handle: "daneel", name: "Daneel", kind: "hosted", is_default: true,
  avatar_url: null, brief: "Triage the team's pull requests and issues.", harness: null, model_provider: null, model_endpoint_id: null, model_id: null,
  effort: null, proactive_messaging: "ask", frozen: null, effective_frozen: false, created_at: now, updated_at: now,
  model_roles: null, subagent_model: "inherit",
};

/** Real built-in packages: id, display name (plugin.yaml `name:`). Every one ships icon.png and icon-dark.png. */
export const PACKAGES: { id: string; name: string; description: string }[] = [
  { id: "atlassian", name: "Atlassian", description: "Jira and Confluence Cloud" },
  { id: "aws-keys", name: "AWS", description: "AWS APIs through an assumed IAM role or with access keys" },
  { id: "github", name: "GitHub", description: "Repositories, issues, pull requests, files and Actions runs (GitHub App, OAuth or token)" },
  { id: "gitlab", name: "GitLab", description: "Projects, merge requests and pipelines" },
  { id: "google-workspace", name: "Google Workspace", description: "Gmail, Calendar and Drive" },
  { id: "indico", name: "Indico", description: "Events and contributions" },
  { id: "linear", name: "Linear", description: "Issues and projects" },
  { id: "microsoft-365", name: "Microsoft 365", description: "Outlook, Teams and OneDrive" },
  { id: "notion", name: "Notion", description: "Pages and databases" },
  { id: "postgresql", name: "PostgreSQL", description: "Databases over the wire protocol" },
  { id: "sentry", name: "Sentry", description: "Issues and releases" },
];

const apps = PACKAGES.map((p) => ({
  app_id: p.id, available: true, category: "developer", connection_count: p.id === "github" ? 1 : 0, description: p.description,
  has_dark_icon: true, name: p.name, package_id: p.id, package_version: "1.0.0",
  // Mirrors plugins/builtin/*/plugin.yaml `grants:` (P4-GRANTS): Indico is custom access only; the AWS packages block untooled requests.
  app_restrictions: {
    custom_only: p.id === "indico",
    block_untooled: p.id === "indico" || p.id === "aws-keys",
  },
  // (P4-GSL) the built-in GitHub and GitLab packages declare a `library` block.
  serves_library: p.id === "github" || p.id === "gitlab",
  // (P5-POLISH-3/5, C-1029) The catalogue says which apps wait for an admin (no sign-in app registered), what they need and
  // who can fix it, and the accounts of the app the workspace has; the journey groups from this alone.
  summary: p.description,
  accounts: [] as unknown[],
  readiness: ["google-workspace", "microsoft-365", "notion", "linear", "sentry"].includes(p.id)
    ? {
        state: "needs_admin", needs: ["oauth_client"],
        who_can: { action: "register_oauth_client", role: "org_admin", people: [{ name: "Ada Lovelace" }], you_can: false },
      }
    : { state: "ready" },
}));

const connection = (n: number, appId: string, label: string) => ({
  app_id: appId, app_version: "1.0.0", created_at: now, id: id(n), injecting: [], label, metadata: {}, missing_scopes: [], org_id: id(1),
  package_id: appId, scope: "workspace", scopes: [], status: "active", updated_at: now, workspace_id: id(2),
});

const approval = {
  id: id(60), org_id: id(1), workspace_id: id(2), status: "pending", created_at: now, deadline: later, host: "api.github.com", method: "PUT",
  path: "/repos/acme/web/pulls/42/merge", app_id: "github", agent: { id: id(11), handle: "daneel", name: "Daneel" },
  summary: { source: "app", title: "merge a pull request", fields: [{ label: "Repository", value: "acme/web" }] },
  // (P5-UX-16, C-982) The sentence, what asked (the agent's access) and whether the viewer may choose Always allow.
  sentence: {
    agent: { id: id(11), name: "Daneel" }, action: "merge a pull request", app: { app_id: "github", name: "GitHub" },
    account: { ref: { type: "connection", id: id(30) }, name: "Acme GitHub" },
  },
  asked_by: { kind: "access", agent_id: id(11), connection_id: id(30), tool_id: "merge_pull" },
  can_always_allow: true,
};

/** P4-011B-4: an approval the gateway parked (the agent stopped waiting): the Parked chip and sentence render. */
const parkedApproval = {
  ...approval, id: id(61), parked: true, deadline: new Date(Date.now() + 7_200_000).toISOString(),
  summary: { source: "app", title: "delete a repository", fields: [] },
  // A rule asked this one: no Always allow, "Asked by the rule …".
  sentence: { agent: { id: id(11), name: "Daneel" }, action: "delete a repository", app: { app_id: "github", name: "GitHub" } },
  asked_by: { kind: "rule", rule_id: id(81), rule_name: "Ask before deleting", scope: "workspace" },
  can_always_allow: false,
  method: "DELETE", path: "/repos/acme/old",
};

const generation = (status: string) => ({
  id: id(80), number: 1, status, created_at: now, default_action: "none", open_lane_posture: "enabled", rules: [], uncredentialed_allowlist: [],
  uncredentialed_default: "none", draft_revision: 1,
});
const policyDoc = { scope: "workspace", draft: generation("draft"), published: generation("published") };

const sourced = <T>(value: T) => ({ source: "default", value });
const effective = {
  byo: { posture: sourced("enabled"), require_context_credentials: sourced(false), source_cidrs: sourced(null) },
  egress: { baseline_allowlist: [], org_allowlist: [], open_lane: sourced("enabled"), posture: sourced("enabled") },
  llm: { endpoints: [], posture: sourced("enabled") },
  agent_tokens: { max_age_days: sourced(null) },
  non_compliant_secrets: [],
};

// The real built-in connector (connectors/mattermost, name "Mattermost"). It ships no icon file yet
// (`icon_png_b64: None`), so the page shows the monogram tile; the shape of the data is C-450/C-452.
const mattermostSetup = [
  // docs/13 §3.1: the server URL and admin token stay per integration (`integration_setup`).
  {
    name: "public_url", label: "Callback URL", type: "url", required: false, placeholder: "https://aurora.example.org/connectors/mattermost",
    help: "Where Mattermost reaches Aurora. Leave empty to use <external URL>/connectors/mattermost.",
  },
  { name: "callback_hmac_key", label: "Callback signing key", type: "generated_secret", required: false },
  { name: "catchup_window_secs", label: "Catch-up window (seconds)", type: "number", required: false, default: 600, min: 1, max: 86400 },
  { name: "token_check_secs", label: "Token check interval (seconds)", type: "number", required: false, default: 30, min: 1, max: 3600 },
];
const mattermostManifest = {
  package: { id: "mattermost" },
  capabilities: { threads: true, files: true, reactions: true, edits: false },
  public_callbacks: ["actions", "commands"],
  integration_setup: { fields: [{ name: "server_url", label: "Server URL", type: "text", store: "metadata" }, { name: "bot_token", label: "Bot token", type: "secret", store: "credentials" }] },
  instance_setup: { fields: mattermostSetup },
};
export const mattermostType = {
  type_id: "mattermost", name: "Mattermost", version: "1.2.0", publisher: "Aurora", icon_png_b64: null,
  setup_guide_md: "# Mattermost\n\nConnect a Mattermost team with a bot account so agents can chat there.",
  manifest: mattermostManifest, manifest_digest: "sha256:aaaa", instance_setup: { fields: mattermostSetup }, hosts: 2, added: false,
};
export const mattermostConnector = {
  id: "mattermost", display_name: "Mattermost", status: "active", version: "1.2.0", contract: 1, managed: true, type_id: "mattermost",
  manifest: mattermostManifest, manifest_digest: "sha256:aaaa", approved_manifest_digest: "sha256:aaaa", approved_manifest: mattermostManifest, widening: [],
  replicas: [], integration_count: 2, approved_at: now, created_at: now,
  settings: { public_url: null, catchup_window_secs: 600, token_check_secs: 30 },
  secrets: { callback_hmac_key: { set: true, generated: true, updated_at: now } },
  host_status: [{ host_instance: id(501), state: "running", message: null, at: now }],
};

export const groupFixture = (n: number, source: "manual" | "idp") => ({
  id: id(n), org_id: id(1), name: source === "idp" ? "CERN platform e-group" : "Platform team", description: "Runs the shared infrastructure",
  source, idp_provider: source === "idp" ? "corp" : null, idp_value: source === "idp" ? "aurora-platform" : null,
  org_role: source === "idp" ? "member" : null, member_count: 1, workspace_count: 1, revision: 1, created_at: now,
});

/** P4-ONB-2: Daneel holds the organization's LiteLLM key (AIGW) and a default model resolves. */
export const readinessReady = {
  harness: "claude-code",
  keys: [{ secret_id: id(300), name: "AIGW", scope: "organization", provider: "litellm", endpoint_id: "litellm", usable: true, reason: null }],
  override_dropped: false,
  default: {
    endpoint_id: "litellm",
    model_id: "claude-sonnet-4",
    provider: "litellm",
    source: "endpoint_default",
    roles: {
      large: { model_id: "claude-opus-4", source: "endpoint", warning: null },
      medium: { model_id: "claude-sonnet-4", source: "endpoint", warning: null },
      small: { model_id: null, source: "none", warning: "role_unset" },
    },
    roles_dropped: [],
    subagent_model: "inherit",
    subagent_warning: null,
  },
};
/** Nothing granted yet: the guide opens the key step. */
export const readinessEmpty = { harness: "claude-code", keys: [], override_dropped: false, default: null };

const setupCheck = (cid: string, status: string, reason: string, message: string, extra: Record<string, unknown> = {}) => ({ id: cid, status, reason, message, value: null, helm: null, ...extra });
/** `GET /v1/admin/setup` (C-533) for an instance with real names: CERN SSO, LiteLLM (AIGW), Mattermost. */
export const setupStatus = {
  placement_saved: false,
  you: {
    provider: "cern", provider_name: "CERN SSO", role_claim: "cern_roles", values: ["aurora-admins", "aurora-users"],
    granted: [
      { value: "aurora-admins", instance_admin: true, org: null, workspaces: [] },
      { value: "aurora-users", instance_admin: false, org: { slug: "users", role: "member" }, workspaces: [] },
    ],
  },
  steps: [
    { id: "roles", status: "ok", reason: "roles_ok", checks: [setupCheck("role_mappings", "ok", "role_mappings_set", "Roles in the sign-in token decide who is an admin.", { helm: "auth.oidc_providers[].role_mappings" })] },
    { id: "placement", status: "todo", reason: "placement_unsaved", checks: [setupCheck("placement", "todo", "placement_unsaved", "Choose where new users land.")] },
    {
      id: "llm", status: "ok", reason: "llm_ok",
      checks: [
        setupCheck("endpoint:litellm", "ok", "endpoint_reachable", "AIGW answers.", { value: "https://aigw.example.org" }),
        setupCheck("default_model:litellm", "ok", "default_model_automatic", "AIGW: agents use the first model each key serves unless you set one.", { helm: "llm.endpoints[].default_model" }),
      ],
    },
    { id: "runners", status: "ok", reason: "runners_ok", checks: [setupCheck("runners", "ok", "runner_online", "1 runner(s) online.", { value: "1" }), setupCheck("gateway", "ok", "gateway_online", "The gateway is up: agents reach the internet through it.", { value: "1" })] },
    { id: "optional", status: "ok", reason: "optional", checks: [setupCheck("connectors", "todo", "no_connectors", "Connectors bring chat tools such as Mattermost to your agents.", { value: "0" })] },
  ],
};

/** P4-PRV-1: the shares of the mock agent, at `/v1/agents/{agent}/previews`. */
export const previewShares = [
  {
    id: id(120), agent_id: agent.id, port: 3000, label: "pqvgh2sjkbmxfwqondslrazi",
    hostname: "pqvgh2sjkbmxfwqondslrazi.previews.example.org",
    url: "https://pqvgh2sjkbmxfwqondslrazi.previews.example.org/",
    visibility: "private", state: "active", status: "listening",
    start_command: "npm run dev", cwd: null, last_start: { at: now, exit_code: 0, tail: "ready on 3000" },
    last_accessed_at: now, created_by: { id: id(99), label: "ada@example.test" }, created_at: now,
  },
];

export const user ={ id: id(99), display_name: "Ada Lovelace", email: "ada@example.test", is_instance_admin: true };

/** (P5-UX-3) Accounts a spec's saves made, listed first by the workspace's accounts list; emptied by every `mockApi` call. */
export const extraAccounts: Record<string, unknown>[] = [];

/** (P5-UX-14) Ids of the connect methods that have a probe (`has_probe`); none by default. Emptied by every `mockApi` call. */
export const probed = new Set<string>();

/** (P5-UX-9) OAuth clients a spec registered in the workspace; the setup status counts them and the list shows them. Emptied by every `mockApi` call. */
export const extraClients: Record<string, unknown>[] = [];

/** (P5-REFINE-13) An app's detail carries no `first_prompt` while `on` (the Done step then offers Open chat). The spec that sets it resets it. */
export const noFirstPrompt = { on: false };

export async function mockApi(
  page: Page,
  /** (FIX-029) Extra canned responses: a regex or exact path → JSON body, or `{status, body}`. */
  overrides: Record<string, unknown> = {},
) {
  const exact = new Map<string, unknown>();
  const patterns: [RegExp, unknown][] = [];
  extraAccounts.length = 0;
  extraClients.length = 0;
  probed.clear();
  let availability: { posture: string; allow: { app_id: string; workspace_id?: string }[] } = { posture: "open", allow: [] };
  // (P5-IA-4, C-969) The organization's retention: Administration's maximums above, saved values kept across requests.
  let retention: Record<string, unknown> = {
    conversations_days: 180, conversations_days_max: 365, attachments_days: null, attachments_days_max: 90,
    attachment_quota_mib: null, attachment_quota_mib_max: 102_400, attachment_quota_mib_effective: 51_200,
  };
  for (const [k, v] of Object.entries(overrides)) {
    if (/[\^*+()[\]{}|\\]/.test(k)) patterns.push([new RegExp(k.startsWith("/") ? `^${k}$` : k), v]);
    else exact.set(k, v);
  }
  await page.route("**/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const p = url.pathname;
    // (P5-IA-5) The settings sections come with their words (`fields`), as the API's field registry writes them.
    const json = (b: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(/^\/v1\/admin\/settings\/[a-z_]+$/.test(p) ? withWords(b) : b) });
    if (p.includes("/stream")) return route.abort();
    const hit = exact.get(p) ?? patterns.find(([re]) => re.test(p))?.[1];
    if (hit !== undefined) {
      const { status = 200, body = hit, ...rest } = hit && typeof hit === "object" && "body" in (hit as Record<string, unknown>) ? (hit as Record<string, unknown>) : { body: hit };
      void rest;
      return json(body, status as number);
    }
    const icon = p.match(/^\/v1\/apps\/([^/]+)\/icon$/);
    if (icon) {
      const file = join(BUILTIN, icon[1], "assets", url.searchParams.get("variant") === "dark" ? "icon-dark.png" : "icon.png");
      // An uploaded app (no built-in folder) has no icon file here: the API's 404, which the page draws as initials.
      if (!existsSync(file)) return route.fulfill({ status: 404, contentType: "application/json", body: "{}" });
      return route.fulfill({ status: 200, contentType: "image/png", body: readFileSync(file) });
    }
    if (p === "/v1/me") {
      return json({ user, memberships: [{ org, status: "active", workspaces: [{ ...ws, personal: false }] }], default_org_id: id(1), client_address: "203.0.113.7" });
    }
    // P4-ONB-2: onboarding progress (seen, so `/` stays on the workspace), the guide's readiness and the setup wizard.
    if (p === "/v1/me/user-tokens") return json({ tokens: [] });
    // (P5-REFINE-3, C-1056) "Show more controls" is on in the mock so every spec sees the whole control surface; the
    // altitude spec overrides this route to test it off.
    if (p === "/v1/me/onboarding") return json({ welcome_seen: true, guide_dismissed: [], tour_completed_at: null, notices_seen: ["renames-2026-11"], more_controls: true });
    if (/^\/v1\/agents\/[^/]+\/model-readiness$/.test(p)) return json(readinessReady);
    // FIX-029 (C-611) + P4-PRV-1: ONE sandbox view — running with port 3000 listening (the
    // Previews tab's chips, the Computer tab's "Listening ports") and the FIX-029 start-log
    // fields. Tests that need a different state override this route (`exact`/`patterns` hit
    // first), e.g. stage4-routes' failed-start notice.
    if (/^\/v1\/agents\/[^/]+\/sandbox$/.test(p)) {
      return json({
        agent_id: agent.id, state: "running", boot_id: id(130), runner_id: id(131), image: "aurora-sandbox:latest",
        started_at: now, keep_awake_reasons: ["preview"], failure_code: null, failure_message: null, start_attempt: null,
        retryable: false, start_log: [], processes: [],
        ports: [{ port: 3000, protocol: "tcp", shared: previewShares[0].hostname }],
        restart_to_update: false, preview_in_use: true,
      });
    }
    if (/^\/v1\/agents\/[^/]+\/start-failures$/.test(p)) return json({ items: [] });
    if (/^\/v1\/agents\/[^/]+\/sandbox\/retry$/.test(p)) return json({ error: { code: "sandbox_not_failed", message: "not failed" } }, 409);
    if (/^\/v1\/admin\/agents\/[^/]+\/start-failures$/.test(p)) return json({ items: [] });
    if (p === "/v1/admin/setup") return json(setupStatus);
    if (p.endsWith("/policy") && (p.startsWith("/v1/orgs/") || p.startsWith("/v1/workspaces/"))) return json(policyDoc);
    // (FIX-164, C-1020) The published rules, which every member of the workspace reads.
    if (/^\/v1\/workspaces\/[^/]+\/internet-access$/.test(p)) {
      const { number, default_action, uncredentialed_default, uncredentialed_allowlist, open_lane_posture, rules } = policyDoc.published;
      return json({ number, default_action, uncredentialed_default, uncredentialed_allowlist, open_lane_posture, rules });
    }
    if (p === "/v1/llm/providers") return json({ providers: [] });
    if (p.endsWith("/governance/effective")) return json(effective);
    if (/^\/v1\/orgs\/[^/]+\/retention$/.test(p)) {
      if (route.request().method() === "PUT") retention = { ...retention, ...(route.request().postDataJSON() as Record<string, unknown>) };
      return json(retention);
    }
    // (P5-USAGE-2, C-1000, C-1003, C-1006) Usage: a week with turns, one deleted agent, one unknown model.
    if (/\/usage$/.test(p) && (p === "/v1/admin/usage" || /^\/v1\/(workspaces|orgs)\/[^/]+\/usage$/.test(p))) {
      const from = url.searchParams.get("from") ?? "2026-10-19";
      const to = url.searchParams.get("to") ?? "2026-11-17";
      const zero = { turns: 0, turn_seconds: 0, awake_seconds: 0, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0 };
      const days: Record<string, unknown>[] = [];
      for (let d = new Date(`${from}T00:00:00Z`); d <= new Date(`${to}T00:00:00Z`) && days.length < 400; d = new Date(d.getTime() + 86_400_000)) {
        const n = d.getUTCDate() % 5;
        days.push({ day: d.toISOString().slice(0, 10), ...zero, turns: n, turn_seconds: n * 90, awake_seconds: n * 600, input_tokens: n * 12_000, output_tokens: n * 1_500 });
      }
      const sum = (k: string) => days.reduce((t, x) => t + (x[k] as number), 0);
      const totals = Object.fromEntries(Object.keys(zero).map((k) => [k, sum(k)]));
      const row = { ...zero, turns: 7, turn_seconds: 630, awake_seconds: 4200, input_tokens: 84_000, output_tokens: 10_500, cache_read_tokens: 2_000, cache_write_tokens: 300 };
      const by_model = [
        { provider: "litellm", endpoint_id: "gateway", model: "claude-sonnet", turns: 7, turn_seconds: 630, input_tokens: 80_000, output_tokens: 10_000, cache_read_tokens: 2_000, cache_write_tokens: 300 },
        { provider: null, endpoint_id: null, model: null, turns: 0, turn_seconds: 0, input_tokens: 4_000, output_tokens: 500, cache_read_tokens: 0, cache_write_tokens: 0 },
      ];
      const common = { from, to, totals, by_day: days, by_model, runtime_as_of: now };
      if (p === "/v1/admin/usage") {
        return json({ ...common, by_org: [{ org_id: id(1), org_name: "Acme Labs", ...row }, { org_id: id(77), org_name: null, ...zero, input_tokens: 5 }],
          by_workspace: [{ workspace_id: id(2), workspace_name: "Main workspace", org_id: id(1), org_name: "Acme Labs", ...row }], other_workspaces: { ...zero, input_tokens: 9 } });
      }
      if (p.startsWith("/v1/orgs/")) return json({ ...common, by_workspace: [{ workspace_id: id(2), workspace_name: "Main workspace", ...row }], other_workspaces: null });
      return json({ ...common, by_agent: [{ agent_id: agent.id, agent_name: agent.name, ...row }, { agent_id: id(555), agent_name: null, ...zero, input_tokens: 7 }] });
    }
    if (p === "/v1/admin/storage") {
      return json({
        attachment_bytes: 850_000_000, attachment_files: 12, blob_bytes: 1_200_000, blob_count: 4, purged_files: 3,
        orgs: [{ org_id: id(1), slug: "acme", name: "Acme Labs", files: 12, attachment_bytes: 850_000_000, quota_bytes: 1_000_000_000 }],
        tables: [{ table: "events", bytes: 4_800_000 }],
      });
    }
    // P4-GRP-1: org Groups (a manual group and a sign-in-linked one).
    if (p === `/v1/orgs/${id(1)}/groups`) return json({ items: [groupFixture(401, "manual"), groupFixture(402, "idp")], next_cursor: null });
    // P4-GRP-2: SCIM directory sync (org settings page shows the members' badges too).
    if (p === `/v1/orgs/${id(1)}/scim`) {
      return json({
        available: true, enabled: true, exclusive: false, base_url: "https://aurora.example.org/scim/v2",
        email_domains: ["example.test"],
      });
    }
    if (p === `/v1/orgs/${id(1)}/scim/tokens`) {
      return json({
        tokens: [
          { id: id(420), name: "Entra ID", display_prefix: "aud_Ej4KUq2mTz9", created_at: now, expires_at: new Date(Date.now() + 300 * 86_400_000).toISOString(), last_used_at: now, revoked: false, expiring: false },
          { id: id(421), name: "Old keycloak", display_prefix: "aud_Yb3Rn8wQx1c", created_at: now, expires_at: new Date(Date.now() + 10 * 86_400_000).toISOString(), last_used_at: null, revoked: false, expiring: true },
        ],
      });
    }
    if (p === `/v1/orgs/${id(1)}/scim/status`) {
      return json({
        enabled: true, last_request_at: now,
        users: { active: 2, suspended: 1, removed: 3 },
        groups: 1,
        errors: [{ at: now, method: "POST", resource: "/Users", status: 409, scim_type: "uniqueness" }],
      });
    }
    const grp = p.match(/^\/v1\/groups\/([^/]+)(\/[a-z]+)?$/);
    if (grp) {
      const g = grp[1] === id(402) ? groupFixture(402, "idp") : groupFixture(401, "manual");
      if (!grp[2]) return json(g);
      if (grp[2] === "/members") {
        return json({
          items: [{ user: { id: id(99), email: "ada@example.test", display_name: "Ada Lovelace" }, status: "active", source: g.source, synced_at: g.source === "idp" ? now : null, added_at: now }],
          next_cursor: null,
        });
      }
      if (grp[2] === "/workspaces") return json({ items: [{ workspace: { id: id(2), name: "Main workspace", slug: "main" }, role: "member", created_at: now }], next_cursor: null });
      if (grp[2] === "/impact") return json({ workspaces: [] });
    }
    // P4-ONB-1: Admin › Sign-up & placement (a saved shared-org placement with a starter grant).
    if (p === "/v1/admin/placement") {
      return json({
        saved: true, signup_mode: "open", mode: "shared_org", default_org: { id: id(1), slug: "users", name: "Users" }, default_org_role: "member",
        shared_workspaces: [{ workspace_id: id(2), slug: "main", name: "Main workspace", role: "member" }],
        personal_workspace: { enabled: true, role: "owner", name_template: "{sub}|{email_local}" },
        starter_grants: [{ kind: "secret", id: id(300), name: "org-llm-key", detail: "llm" }],
        personal_orgs: { count: 1, items: [{ org_id: id(301), slug: "ada-s-org", name: "Ada's org", owner_email: "ada@example.test", created_at: now }] },
        warnings: ["per_user_orgs_active"], updated_at: now, updated_by: id(99),
      });
    }
    // (phase 5, SET-1) Admin › Settings: the index with a Helm-locked key, and one generic section.
    if (p === "/v1/admin/settings") {
      const titles: [string, string, string][] = [
        ["governance", "Instance rules", "What organizations may not go below: AI keys, Internet access and agents people run themselves."],
        ["llm", "LLM endpoints", "Providers and endpoints agents may reach."],
        ["sign_in", "Sign-in", "Sign-up, sessions and identity providers."],
      ];
      return json({
        sections: titles.map(([section, title, description], i) => ({
          section, title, description, generation: i, updated_at: i === 0 ? null : now, updated_by_label: i === 0 ? null : "root@example.test",
          keys_total: 4, keys_locked: i === 0 ? 1 : 0, keys_dashboard: i === 0 ? 0 : 2,
        })),
        locked: [{ section: "governance", key: "governance.egress.minimum_posture", entry_id: null, helm_key: "config.governance.egress.minimum_posture" }],
      });
    }
    // (phase 5, FIX-124) Admin › Secret stores: the built-in store, a Helm store and a dashboard store.
    if (p === "/v1/admin/secret-stores") {
      const health = { ok: true, checked_at: now, error_code: null };
      const base = { references: false, default: false, health, error: null, managed_values: 0, reference_values: 0, reference_scopes: [] as unknown[], helm_key: null as string | null };
      return json({
        default_store: { value: "inline", source: "default" }, effective_default: "inline", generation: 3,
        migration: { state: "none", target: null, started_at: null, finished_at: null, started_by: null, per_purpose: [], error: null, running_elsewhere: false },
        stores: [
          { ...base, id: "inline", kind: "inline", source: "builtin", settings: {}, default: true, managed_values: 12 },
          { ...base, id: "vault", kind: "hashicorp-vault", source: "helm", helm_key: "vault.*", references: true, settings: { address: "https://vault.example.test:8200", kv_mount: "secret", managed_prefix: "aurora", auth: { method: "kubernetes", role: "aurora-api" } } },
          { ...base, id: "team-vault", kind: "hashicorp-vault", source: "dashboard", references: true, health: { ok: false, checked_at: now, error_code: "unavailable" }, settings: { address: "https://team-vault.example.test:8200", auth: { method: "approle", role_id: "r1", secret_id: { $secret: { set: true } } } }, reference_scopes: [{ org_id: id(1), org_slug: "acme", paths: ["teams/acme/*"] }] },
        ],
      });
    }
    if (p === "/v1/admin/settings/governance") {
      const sch = (kind: string, extra: Record<string, unknown> = {}) => ({ kind, min: null, max: null, max_len: null, options: null, item_kind: null, ...extra });
      return json({
        section: "governance", title: "Instance rules", description: "What organizations may not go below: AI keys, Internet access and agents people run themselves.", generation: 1,
        updated_at: now, updated_by_label: "root@example.test", extra: null,
        settings: [
          { key: "governance.egress.minimum_posture", value: "restricted", source: "helm", locked: true, helm_key: "config.governance.egress.minimum_posture", default: "open", schema: sch("enum", { options: ["open", "restricted", "strict"] }), list: null, id_field: null, entries: null, secret: null },
          { key: "governance.llm.require_key_origin", value: true, source: "dashboard", locked: false, helm_key: "config.governance.llm.require_key_origin", default: false, schema: sch("bool"), list: null, id_field: null, entries: null, secret: null },
          { key: "governance.agent_tokens.max_age_days", value: 90, source: "default", locked: false, helm_key: "config.governance.agent_tokens.max_age_days", default: 90, schema: sch("int", { min: 1, max: 3650 }), list: null, id_field: null, entries: null, secret: null },
          { key: "byo.minimum_posture", value: "owners_only", source: "default", locked: false, helm_key: "config.byo.minimum_posture", default: "owners_only", schema: sch("enum", { options: ["enabled", "owners_only", "disabled"] }), list: null, id_field: null, entries: null, secret: null },
          { key: "byo.default_source_cidrs", value: ["10.0.0.0/8"], source: "dashboard", locked: false, helm_key: "config.byo.default_source_cidrs", default: [], schema: sch("list", { item_kind: "string", max: 50 }), list: "whole", id_field: null, entries: null, secret: null },
          { key: "governance.example.token", value: null, source: "dashboard", locked: false, helm_key: "config.governance.example.token", default: null, schema: sch("string"), list: null, id_field: null, entries: null, secret: { set: true, updated_at: now, from_env: null } },
          { key: "governance.example.items", value: [{ id: "managed" }], source: "default", locked: false, helm_key: "config.governance.example.items", default: [], schema: sch("list", { item_kind: "object" }), list: "keyed", id_field: "id", entries: [{ id: "managed", value: { id: "managed" }, source: "helm", locked: true, warnings: null }], secret: null },
        ],
      });
    }
    // (phase 5, SET-3) Admin › Settings › LLM endpoints: a Helm endpoint and a dashboard endpoint, the providers
    // of this release, the preference order locked by Helm.
    if (p === "/v1/admin/settings/llm") {
      const sch = (kind: string, extra: Record<string, unknown> = {}) => ({ kind, min: null, max: null, max_len: null, options: null, item_kind: null, ...extra });
      const central = { id: "central", provider: "litellm", base_url: "https://llm.example.test", default_model: "claude-sonnet" };
      const team = { id: "team", provider: "openai_compatible", base_url: "https://team-llm.example.test/v1", auth_header: "api-key" };
      return json({
        section: "llm", title: "LLM endpoints", description: "Providers and endpoints agents may reach.", generation: 2,
        updated_at: now, updated_by_label: "root@example.test", extra: { providers: ["litellm", "anthropic", "openai", "openai_compatible", "vertex"], endpoint_harnesses: { central: ["claude_code", "codex"], team: [] } },
        settings: [
          { key: "llm.enabled_providers", value: ["litellm", "anthropic", "openai_compatible"], source: "dashboard", locked: false, helm_key: "config.llm.enabled_providers", default: ["litellm", "anthropic"], schema: sch("list", { item_kind: "string" }), list: "whole", id_field: null, entries: null, secret: null },
          { key: "llm.provider_preference", value: ["litellm", "anthropic"], source: "helm", locked: true, helm_key: "config.llm.provider_preference", default: ["litellm", "anthropic"], schema: sch("list", { item_kind: "string" }), list: "whole", id_field: null, entries: null, secret: null },
          {
            key: "llm.endpoints", value: [central, team], source: "helm", locked: false, helm_key: "config.llm.endpoints", default: [], schema: sch("list", { item_kind: "object" }), list: "keyed", id_field: "id", secret: null,
            entries: [
              { id: "central", value: central, source: "helm", locked: true, warnings: null },
              { id: "team", value: team, source: "dashboard", locked: false, warnings: ["needs_allowed_private"] },
            ],
          },
        ],
      });
    }
    // (phase 5, SET-4) Admin › Settings › Sign-in and Email.
    if (p === "/v1/admin/settings/sign_in") {
      const sch = (kind: string, extra: Record<string, unknown> = {}) => ({ kind, min: null, max: null, max_len: null, options: null, item_kind: null, ...extra });
      const k = { locked: false, helm_key: "config.auth.x", list: null, id_field: null, entries: null, secret: null, source: "default" };
      const helm = { id: "helm", display_name: "Helm SSO", issuer: "https://sso.example.test", client_id: "aurora", client_secret_env: "AURORA_OIDC_SECRET" };
      const corp = { id: "corp", display_name: "Corp directory", issuer: "https://corp.example.test/realms/main", client_id: "aurora-corp", client_secret: { $secret: { set: true } }, role_claim: "roles", role_mappings: [{ value: "ops", instance_admin: false, org: { slug: "acme", role: "member" } }] };
      return json({
        section: "sign_in", title: "Sign-in", description: "Sign-up, sessions and identity providers.", generation: 3,
        updated_at: now, updated_by_label: "root@example.test", extra: { providers_from_helm: 1, signed_in_with: "helm" },
        settings: [
          { ...k, key: "auth.signup_mode", value: "invite_only", default: "invite_only", schema: sch("enum", { options: ["invite_only", "sso_jit", "open"] }) },
          { ...k, key: "auth.org_creation", value: "instance_admins", default: "instance_admins", schema: sch("enum", { options: ["instance_admins", "anyone"] }) },
          { ...k, key: "auth.session_idle_secs", value: 3600, default: 3600, schema: sch("duration_secs", { min: 60 }) },
          { ...k, key: "auth.session_absolute_secs", value: 86400, default: 86400, schema: sch("duration_secs", { min: 60 }) },
          { ...k, key: "auth.cli_key_ttl_days", value: 90, default: 90, schema: sch("int", { min: 1 }) },
          {
            ...k, key: "auth.oidc_providers", value: [helm, corp], source: "helm", helm_key: "config.auth.oidc_providers", default: [], list: "keyed", id_field: "id", schema: sch("list", { item_kind: "object" }),
            entries: [
              { id: "helm", value: helm, source: "helm", locked: true, warnings: null },
              { id: "corp", value: corp, source: "dashboard", locked: false, warnings: null },
            ],
          },
        ],
      });
    }
    if (p === "/v1/admin/settings/email") {
      const sch = (kind: string) => ({ kind, min: null, max: null, max_len: null, options: null, item_kind: null });
      const k = { locked: false, helm_key: "config.email.x", list: null, id_field: null, entries: null, source: "dashboard", default: null };
      return json({
        section: "email", title: "Email", description: "The mail server for sign-in, invitations and notifications.", generation: 1,
        updated_at: now, updated_by_label: "root@example.test", extra: null,
        settings: [
          { ...k, key: "email.smtp_url", value: "smtp://mail.example.test:25", schema: sch("string"), secret: null },
          { ...k, key: "email.from", value: "aurora@example.test", schema: sch("string"), secret: null },
          { ...k, key: "email.smtp_username", value: "aurora", schema: sch("string"), secret: null },
          { ...k, key: "email.smtp_password", value: null, schema: sch("string"), secret: { set: true, updated_at: now, from_env: null } },
        ],
      });
    }
    // (phase 5, SET-5) The operational pages: agents with the runners' sizes, a terminal the deployment does not
    // provide, and limits with a Helm rate class and the allowed-private list.
    if (p === "/v1/admin/settings/agents" || p === "/v1/admin/settings/terminal" || p === "/v1/admin/settings/limits") {
      const sch = (kind: string, o: Record<string, unknown> = {}) => ({ kind, min: null, max: null, max_len: null, options: null, item_kind: null, ...o });
      const k = { locked: false, helm_key: "config.x", list: null, id_field: null, entries: null, source: "default", default: null, secret: null };
      const sec = (section: string, title: string, settings: unknown[], extra: unknown) => json({
        section, title, description: `${title} settings.`, generation: 2, updated_at: now, updated_by_label: "root@example.test", extra, settings,
      });
      if (p.endsWith("/agents")) {
        return sec("agents", "Agents & sandboxes", [
          { ...k, key: "agents.default_name", value: "Daneel", default: "Daneel", schema: sch("string", { max_len: 100 }) },
          { ...k, key: "hosted.default_harness", value: "claude-code", default: "claude-code", schema: sch("string", { max_len: 64 }) },
          { ...k, key: "hosted.enabled_harnesses", value: ["claude-code"], default: ["claude-code"], schema: sch("list", { item_kind: "string" }) },
          { ...k, key: "hosted.idle_stop_secs", value: 1800, source: "helm", locked: true, helm_key: "config.hosted.idle_stop_secs", default: 1800, schema: sch("int") },
          { ...k, key: "hosted.turn_ceiling_secs", value: 21600, default: 21600, schema: sch("int") },
        ], {
          runners: [
            { id: id(40), name: "main", sandbox_memory_mib: 4096, sandbox_cpus: 2, sandbox_pids: 512, home_gib: 5, max_sandboxes: 7, harnesses: ["claude-code", "codex"] },
            { id: id(41), name: "second", sandbox_memory_mib: 4096, sandbox_cpus: 2, sandbox_pids: 512, home_gib: 5, max_sandboxes: 7, harnesses: ["claude-code", "goose"] },
          ],
          offered_harnesses: ["claude-code", "codex", "goose"],
        });
      }
      if (p.endsWith("/terminal")) {
        return sec("terminal", "Terminal", [
          { ...k, key: "terminal.enabled", value: false, default: false, schema: sch("bool") },
          { ...k, key: "terminal.awake_cap_secs", value: 14400, default: 14400, schema: sch("int") },
        ], { available: false });
      }
      return sec("limits", "Limits", [
        { ...k, key: "ratelimit.api.multiplier", value: 1, default: 1, schema: sch("float", { min: 0, max: 100 }) },
        {
          ...k, key: "ratelimit.api.classes", value: [{ class: "write", per_minute: 600, burst: 100 }], source: "helm", list: "keyed", id_field: "class", default: [],
          schema: sch("list", { item_kind: "object", max: 16 }),
          entries: [{ id: "write", source: "helm", locked: true, value: { class: "write", per_minute: 600, burst: 100 }, warnings: null }],
        },
        { ...k, key: "gateway.allowed_private", value: ["10.1.0.0/16"], source: "dashboard", default: [], schema: sch("list", { item_kind: "string" }) },
      ], null);
    }
    // (P5-WEBHOOK-4, C-912) The outgoing_webhooks section on Apps & accounts › Advanced: one Helm-locked ceiling, one dashboard value.
    if (p === "/v1/admin/settings/outgoing_webhooks") {
      const sch = (min: number, max: number) => ({ kind: "int", min, max, max_len: null, options: null, item_kind: null });
      const k = { list: null, id_field: null, entries: null, secret: null };
      return json({
        section: "outgoing_webhooks", title: "Outgoing webhooks", description: "How often one outgoing webhook may be called.", generation: 1, updated_at: now,
        updated_by_label: "root@example.test", extra: null,
        settings: [
          { ...k, key: "outgoing_webhooks.per_minute_max", value: 30, source: "helm", locked: true, helm_key: "config.outgoing_webhooks.per_minute_max", default: 30, schema: sch(1, 600) },
          { ...k, key: "outgoing_webhooks.per_day_max", value: 500, source: "default", locked: false, helm_key: "config.outgoing_webhooks.per_day_max", default: 500, schema: sch(1, 100000) },
        ],
      });
    }
    if (p === "/v1/admin/placement/preview") return json({ name: "bobj", slug: "bobj", used: 0, unresolved: [] });
    if (p === "/v1/admin/placement/choices") {
      return json({ workspaces: [{ id: id(2), slug: "main", name: "Main workspace" }], secrets: [{ id: id(300), name: "org-llm-key", kind: "llm" }], connections: [] });
    }
    // P5-ADMDEL (C-733): counts and ownerless state; the organization page and its workspaces.
    const adminOrg = { id: id(1), slug: "users", name: "Users", frozen: null, created_at: now, ownerless: true, member_count: 3, workspace_count: 2, agent_count: 5 };
    if (p === "/v1/admin/orgs") return json({ items: [adminOrg], next_cursor: null });
    if (p === `/v1/admin/orgs/${id(1)}`) return json(adminOrg);
    if (p === `/v1/admin/orgs/${id(1)}/workspaces`) {
      return json({
        items: [
          { id: id(2), slug: "main", name: "Main workspace", personal: false, agent_count: 4, created_at: now },
          { id: id(3), slug: "ada", name: "Ada", personal: true, agent_count: 1, created_at: now },
        ],
        next_cursor: null,
      });
    }
    // (FIX-167 Part B, C-1022) Nothing needs the admin by default: the badge stays hidden. `admin-attention.spec.ts` overrides it.
    if (p === "/v1/admin/attention") return json({ items: [] });
    if (p === "/v1/admin/connectors") return json({ items: [mattermostConnector] });
    if (p === "/v1/admin/connector-types") return json({ host_running: true, items: [mattermostType] });
    // FIX-025 (C-586) + FIX-029 (C-614): the runners admin view and its per-runner failures.
    if (p === "/v1/admin/runners") return json({ items: [] });
    if (/^\/v1\/connectors\/[^/]+\/icon$/.test(p)) return route.fulfill({ status: 404, contentType: "application/json", body: "{}" });
    if (/^\/v1\/admin\/plugins\/[^/]+$/.test(p)) {
      const pkg = PACKAGES.find((x) => x.id === p.split("/")[4]) ?? PACKAGES[3];
      return json({
        id: id(70), package_id: pkg.id, version: "1.0.0", contract: 1, digest: "sha256:0000", publisher: "aurora", source: "builtin", status: "approved",
        origin: "aurora", summary: null, reaches: ["api.github.com"], reaches_more: 0, created_at: now, signer: { kind: "builtin", name: "Aurora" },
        apps: [{ app_id: pkg.id, name: pkg.name, ...(pkg.id === "github" ? { library_host: "api.github.com" } : {}) }], host_claims: [], violations: [], widenings: [], impact: null,
        permissions: { hooks: [], context: [], egress: { "api.github.com": ["GET"] } },
      });
    }
    // P4-011B-4: Web Push is on (the Account card and its device list render) and parking is on (the limits route).
    if (p === "/v1/push/config") return json({ available: true, vapid_public_key: "B".repeat(87), allowed_hosts: ["fcm.googleapis.com"] });
    if (p === "/v1/me/push-subscriptions") {
      return json({
        subscriptions: [
          { id: id(700), label: "Chrome on Linux", show_details: false, paused: false, state: "active", this_session: false, created_at: now, last_success_at: now, last_failure_at: null },
          { id: id(701), label: "Firefox on Windows", show_details: true, paused: false, state: "disabled", this_session: false, created_at: now, last_success_at: null, last_failure_at: now },
        ],
      });
    }
    if (p === "/v1/policy/approval-limits") {
      return json({ timeout_secs: 300, max_timeout_secs: 3600, park: { enabled: true, threshold_secs: 900, max_timeout_secs: 86400, resume_window_secs: 1800 } });
    }
    if (p === "/v1/instance") {
      return json({
        version: "0.1.0", signup_mode: "closed", setup_errors: [],
        features: { hosted: true, previews: true, terminal: true, push: true },
        previews: { enabled: true, domain: "previews.example.org" },
        oidc_providers: [{ id: "corp", display_name: "Corporate SSO", groups: true }],
      });
    }
    if (p === "/v1/orgs") return json({ items: [org] });
    if (p === "/v1/resolve") return json({ org, workspace: ws });
    if (p === "/v1/apps") return json({ items: apps });
    if (/^\/v1\/apps\/[^/]+$/.test(p)) {
      const a = PACKAGES.find((x) => x.id === p.split("/")[3]) ?? PACKAGES[3];
      // Connect methods as in plugins/builtin/github/plugin.yaml (FIX-123: GitHub App, token form and OAuth),
      // with the chooser's label, description and Recommended flag.
      const connect = [
        {
          id: "github_app", kind: "hook", access_levels: [], default_scopes: [], scopes: [], recommended: true, label: "GitHub App",
          description: "An app installed on the repositories you choose. Permissions are fine-grained and per repository, tokens last one hour and access does not depend on one person's account. Recommended for teams.",
          fields: [
            { name: "app_id", label: "GitHub App ID or client ID", type: "text", store: "metadata", required: true, placeholder: "123456" },
            { name: "installation_id", label: "Installation ID", type: "text", store: "metadata", required: true, placeholder: "12345678" },
            { name: "private_key", label: "Private key (.pem file)", type: "file", store: "credentials", required: true },
          ],
        },
        {
          id: "token", kind: "fields", access_levels: [], default_scopes: [], recommended: false, label: "Personal access token",
          scopes: [{ scope: "repo", name: "Repositories", access: "write" }, { scope: "read:org", name: "Read your organizations", access: "read" }],
          scope_presets: { read: ["read:org"], write: ["repo", "read:org"] },
          // (P5-POLISH-3, C-1031) What to tick on GitHub's fine-grained token form, per access.
          permission_hints: [
            { level: "read", label: "Read only", hints: ["Contents · Read", "Issues · Read", "Pull requests · Read", "Metadata · Read"] },
            { level: "write", label: "Read and write", hints: ["Contents · Read and write", "Issues · Read and write", "Pull requests · Read and write", "Metadata · Read"] },
          ],
          description: "Acts as you, on everything your account can reach. The quickest way to start for personal use; a fine-grained token can be limited to chosen repositories.",
          fields: [{ name: "access_token", label: "Personal access token", type: "secret", store: "credentials", required: true, placeholder: "Fine-grained or classic personal access token", help_url: "https://github.com/settings/personal-access-tokens" }],
        },
        {
          id: "oauth", kind: "oauth2", access_levels: [], default_scopes: ["read:user"], recommended: false, label: "OAuth", client_key: "github", callback: "query",
          description: "Sign in with GitHub and act as you, with the scopes you tick. Needs an OAuth app registered by an administrator.",
          scopes: [{ scope: "read:user", name: "Read your profile", access: "read" }], fields: [],
        },
      ];
      // (P5-UX-2/3) Every method also carries the audience, the scope presets per access and the values to show.
      const full = connect.map((m) => ({ probe_scopes: [], audience: "person", scope_presets: { read: [], write: [] }, show_context: [], permission_hints: [], has_probe: probed.has(m.id), ...m }));
      return json({
        ...apps.find((x) => x.app_id === a.id), connect: full, credentials: [], hosts: [], metadata: [],
        first_prompt: noFirstPrompt.on ? null : "Which pull requests are waiting for my review?",
        // (P5-UX-9, C-946) The OAuth client definitions say which connect methods use them.
        oauth_clients: ["github", "linear", "gitlab"].includes(a.id)
          ? [{
              client_key: a.id, used_by_methods: [{ id: "oauth", label: `Sign in with ${a.name}` }], setup_note: null,
              redirect_uri: "https://aurora.example.org/v1/oauth/callback",
              fields: [{ name: "client_id", label: "Client ID", secret: false }, { name: "client_secret", label: "Client secret", secret: true }],
            }]
          : [],
        tools: [
          { id: "get_repo", name: "Get a repository", group: "read", description: "Read one repository's details." },
          { id: "create_issue", name: "Create an issue", group: "write", description: "Open an issue with a title and a body." },
        ],
      });
    }
    // (P5-UX-2, C-955) What people here can do to connect an app. Sentry's sign-in is not set up: only an organization
    // administrator can fix it (the journey's "Needs setup by an admin" group); every other app is ready with a token.
    if (/^\/v1\/apps\/[^/]+\/connect-plan$/.test(p)) {
      const app = p.split("/")[3]!;
      const method = (mid: string, kind: string, over: Record<string, unknown> = {}) => ({
        id: mid, kind, label: mid === "token" ? "Personal access token" : "OAuth", description: null, recommended: mid === "token", audience: "person", needs: [],
        ready: true, scope_presets: mid === "token" ? { read: ["read:org"], write: ["repo", "read:org"] } : { read: [], write: [] }, ...over,
      });
      if (app === "sentry") {
        const who = { action: "set_up_oauth", role: "org_admin", people: [{ id: id(99), name: "Ada Lovelace" }], you_can: false };
        return json({ app_id: app, available: false, available_who_can: who, suggested_method: null, methods: [method("oauth", "oauth2", { ready: false, needs: ["oauth_client"], who_can: who })] });
      }
      return json({ app_id: app, available: true, suggested_method: "token", methods: [method("token", "fields"), method("oauth", "oauth2")] });
    }
    // (P5-UX-9, C-931) The setup status of an app. Linear's sign-in has no OAuth client yet (the workspace owner can register
    // one; for everyone only Ada can, by name), GitHub's App needs an administrator's material, GitLab needs a self-hosted
    // server (the organization view can register one), AWS carries the organization's external id.
    if (/^\/v1\/apps\/[^/]+\/setup$/.test(p)) {
      const app = p.split("/")[3]!;
      const orgView = !url.searchParams.get("workspace_id");
      const registered = extraClients.filter((c) => c.package_id === app);
      const m = (mid: string, kind: string, label: string, over: Record<string, unknown> = {}) => ({
        id: mid, kind, label, description: null, recommended: false, audience: "person", needs: [], ready: true,
        scope_presets: { read: [], write: [] }, ...over,
      });
      const adminWho = { action: "register_oauth_client", role: "org_admin", people: [{ id: id(99), name: "Ada Lovelace" }, { id: id(100), name: "Alan Turing" }], you_can: orgView };
      const methods: Record<string, unknown>[] =
        app === "linear"
          ? [
              m("key", "fields", "API key"),
              registered.length > 0
                ? m("oauth", "oauth2", "Sign in with Linear", { client_key: "linear" })
                : m("oauth", "oauth2", "Sign in with Linear", { client_key: "linear", ready: false, needs: ["oauth_client"], who_can: adminWho }),
            ]
          : app === "github"
            ? [
                m("token", "fields", "Personal access token"),
                m("oauth", "oauth2", "Sign in with GitHub", { client_key: "github" }),
                m("github_app", "hook", "GitHub App installation", { audience: "admin", ready: false, needs: ["admin_material"] }),
              ]
            : app === "gitlab"
              ? [m("token", "fields", "Personal access token", { ready: false, needs: ["instance"] }), m("oauth", "oauth2", "Sign in with GitLab", { client_key: "gitlab" })]
              : [m("token", "fields", app === "aws-keys" ? "Access keys" : "API key")];
      return json({
        app_id: app, name: PACKAGES.find((x) => x.id === app)?.name ?? app, available: true, suggested_method: "token", instances: [],
        values: { callback_url: "https://aurora.example.org/v1/oauth/callback", ...(app === "aws-keys" ? { external_id: "aurora-ext-7f3a9c" } : {}) },
        can: orgView
          ? { register_workspace_client: false, register_org_client: true, register_instance: app === "gitlab", set_availability: true, connect_workspace: false, connect_org: true }
          : { register_workspace_client: true, register_org_client: false, register_instance: false, set_availability: false, connect_workspace: true, connect_org: false },
        clients: {
          workspace: orgView ? null : registered.map((c) => ({ id: c.id, client_key: c.client_key, enabled: true, used_by_methods: [{ id: "oauth", label: `Sign in with ${PACKAGES.find((x) => x.id === app)?.name}` }] })),
          organization: [], instance: { set_up: false },
        },
        methods,
      });
    }
    if (/^\/v1\/workspaces\/[^/]+\/oauth-clients$/.test(p)) {
      if (route.request().method() === "POST") {
        const body = route.request().postDataJSON() as { package_id: string; client_key: string; fields: Record<string, string> };
        extraClients.push({
          id: id(300 + extraClients.length), scope: "workspace", package_id: body.package_id, client_key: body.client_key,
          fields: { ...body.fields, client_secret: "" }, enabled: true, connection_count: 0, created_at: now, updated_at: now,
        });
        return json(extraClients[extraClients.length - 1], 201);
      }
      return json({ items: extraClients });
    }
    // (P5-UX-13, C-940) The setup guide, with the viewer's values filled in.
    if (/^\/v1\/apps\/[^/]+\/guide$/.test(p)) {
      const app = p.split("/")[3]!;
      return route.fulfill({
        status: 200, contentType: "text/markdown",
        body: `1. In ${app}, open the developer settings and create an OAuth application.\n2. Paste \`https://aurora.example.org/v1/oauth/callback\` as the callback URL.`,
      });
    }
    // (P5-UX-9, C-933) Which apps the organization allows: saved values kept across requests.
    if (/^\/v1\/orgs\/[^/]+\/app-availability$/.test(p)) {
      if (route.request().method() === "PUT") availability = route.request().postDataJSON() as typeof availability;
      return json(availability);
    }
    // (P5-UX-4, C-917) The one list of accounts: two apps, a website nobody can use yet and an AI key; the organization's
    // list has an app and an AI key. With `agent_id` the agent's own come first (`given`).
    if (/^\/v1\/(workspaces|orgs)\/[^/]+\/accounts$/.test(p)) {
      const who = { agent: { id: agent.id, name: agent.name, handle: agent.handle }, workspace: { id: ws.id, name: ws.name } };
      const can = { give_access: true, change_access: true, manage: true };
      const forAgent = url.searchParams.get("agent_id") === agent.id;
      const mark = (given: boolean) => (forAgent ? { given } : {});
      if (p.startsWith("/v1/orgs/")) {
        return json({
          items: [
            { ref: { type: "connection", id: id(43) }, kind: "app", name: "platform-prod", scope: "organization", levels: ["read", "ask", "act"], limit: "ask", app: { app_id: "aws-keys", name: "AWS", custom_only: false }, health: { state: "not_used", last_used_at: now }, agents: [{ ...who, level: "ask" }], can },
            { ref: { type: "secret", id: id(62) }, kind: "ai_key", name: "Users AI gateway", scope: "organization", levels: [], health: { state: "not_used", last_used_at: now }, agents: [{ ...who, level: "use" }], can },
          ],
          next_cursor: null,
        });
      }
      return json({
        items: [
          ...extraAccounts,
          { ref: { type: "connection", id: id(40) }, kind: "app", name: "acme-bot", scope: "workspace", levels: ["read", "ask", "act"], app: { app_id: "github", name: "GitHub", custom_only: false }, health: { state: "not_used", last_used_at: now }, agents: [{ ...who, level: "read" }], can, ...mark(true) },
          { ref: { type: "connection", id: id(41) }, kind: "app", name: "Product team", scope: "workspace", levels: ["read", "ask", "act"], app: { app_id: "linear", name: "Linear", custom_only: false }, health: { state: "sign_in_expired" }, agents: [{ ...who, level: "ask", always_allowed: [{ tool_id: "create_issue", name: "Create an issue" }] }], can, ...mark(true) },
          { ref: { type: "secret", id: id(60) }, kind: "ai_key", name: "Team key", scope: "workspace", levels: [], health: { state: "not_used", last_used_at: now }, agents: [{ ...who, level: "use", ...(forAgent ? { model: "claude-sonnet" } : {}) }], can, ...mark(true) },
          { ref: { type: "secret", id: id(61) }, kind: "website", name: "Weather API", scope: "workspace", levels: [], health: { state: "not_used" }, agents: [], can, ...mark(false) },
        ],
        next_cursor: null,
      });
    }
    // The two lists are different scopes: these are workspace connections, the organization's list has none.
    // (Serving them for both made the Access tab, which merges the lists, render ids 40/41 twice.)
    if (p.endsWith("/connections")) {
      return json({ items: p.startsWith("/v1/orgs/") ? [] : [connection(40, "github", "acme-bot"), connection(41, "linear", "Product team")] });
    }
    // (P5-IA-1) The topic pages render several sections at once (approvals, previews, scim, retention, skills …):
    // any section without its own mock above is an empty form. Before the workspace routes: `…/settings/approvals`
    // would otherwise be taken for a workspace's approvals list.
    const section = p.match(/^\/v1\/admin\/settings\/([a-z_]+)$/)?.[1];
    if (section && section !== "export" && section !== "import") {
      // (P5-IA-5, C-973) The retention section stands for one that is not editable yet: its current values, read-only.
      if (section === "retention") {
        return json({
          section, title: "Retention & storage", description: "How long data is kept and storage quotas.", generation: 1, updated_at: now, updated_by_label: null, extra: null,
          settings: [], editable: false,
          fields: [{ key: "retention.request_log_days", label: "Keep the activity of requests for", help: "How long the record of what agents asked for is kept.", kind: "int", choices: null, default_display: "30 days", unit: "days", doc: { title: "Retention and attachment quota", path: "docs/operator/retention-and-quota.md" } }],
          current: [{ key: "retention.request_log_days", label: "Keep the activity of requests for", value_display: "30 days", source: "helm" }],
        });
      }
      return json({ section, title: section, description: "", generation: 1, updated_at: now, updated_by_label: null, extra: null, settings: [] });
    }
    if (p.endsWith("/approvals")) return json({ items: url.searchParams.get("status") === "pending" ? [approval, parkedApproval] : [], next_cursor: null });
    if (p.endsWith("/workspaces")) return json({ items: [ws] });
    if (p.endsWith("/agents")) return json({ items: [agent] });
    // P4-PRV-1: the agent's preview shares (the Previews tab; the sandbox view marks port 3000 shared).
    if (/^\/v1\/agents\/[^/]+\/previews$/.test(p)) return json({ shares: previewShares });
    if (/^\/v1\/agents\/[^/]+\/sandbox$/.test(p)) {
      return json({
        agent_id: agent.id, state: "running", boot_id: id(130), runner_id: id(131), image: "aurora-sandbox:latest",
        started_at: now, keep_awake_reasons: ["preview"], failure_code: null, failure_message: null, start_attempt: null,
        processes: [], ports: [{ port: 3000, protocol: "tcp", shared: previewShares[0].hostname }], restart_to_update: false, preview_in_use: true,
      });
    }
    if (p === `/v1/workspaces/${ws.id}`) return json(ws);
    if (p.endsWith("/storage") && p.startsWith("/v1/orgs/")) {
      return json({ org_id: id(1), used_bytes: 850_000_000, quota_bytes: 1_000_000_000, files: 12, purged_files: 3, notice: "warning", by_workspace: [{ workspace_id: id(2), workspace_name: "Main workspace", direction: "in", files: 12, bytes: 850_000_000 }] });
    }
    // P4-GRP-2: the Members page shows a "Directory" badge for SCIM-provisioned members.
    if (p === `/v1/orgs/${id(1)}/members`) {
      return json({
        items: [
          { user_id: id(99), email: "ada@example.test", display_name: "Ada Lovelace", role: "owner", status: "active", created_at: now, idp_value: null },
          { user_id: id(102), email: "bob@example.test", display_name: "Bob Builder", role: "member", status: "active", created_at: now, idp_value: "aurora-users", directory: { state: "active", external_id: "entra-bob-1" } },
        ],
        next_cursor: null,
      });
    }
    // P5-IA-2 (C-964): the workspaces that tightened Internet access below the organization.
    if (p === `/v1/orgs/${id(1)}/workspace-overrides`) {
      return json({ items: [{ workspace: { id: ws.id, name: ws.name }, uncredentialed: "block", open_lane: "disabled", default_rule: "none", rule_count: 2 }], next_cursor: null });
    }
    if (/^\/v1\/orgs\/[^/]+$/.test(p)) return json(org);
    return json({ items: [] });
  });
}

type RouteEntry = { name: string; path: string; admin?: boolean };

/** The dashboard's routes before P5-IA-1. The ones that moved are `LEGACY_ROUTES` (each must redirect); see `ROUTES`. */
const PRE_IA_ROUTES: RouteEntry[] = [
  { name: "root", path: "/" },
  { name: "sign-in", path: "/auth/login" },
  { name: "cli-login", path: "/auth/cli" },
  { name: "join", path: "/join" },
  { name: "link-channel", path: "/link/channel" },
  { name: "setup-error", path: "/setup-error" },
  { name: "account", path: "/account" },
  { name: "account-security", path: "/account/security" },
  { name: "account-api-keys", path: "/account/api-keys" },
  { name: "account-channels", path: "/account/channels" },
  { name: "account-delete", path: "/account/delete" },
  { name: "admin-setup", path: "/admin/setup" },
  { name: "admin-users", path: "/admin/users" },
  { name: "admin-orgs", path: "/admin/orgs" },
  { name: "admin-org-detail", path: `/admin/orgs/${id(1)}` },
  { name: "admin-placement", path: "/admin/placement" },
  { name: "admin-settings", path: "/admin/settings" },
  { name: "admin-settings-section", path: "/admin/settings/governance" },
  { name: "admin-settings-llm", path: "/admin/settings/llm" },
  { name: "admin-settings-sign-in", path: "/admin/settings/sign-in" },
  { name: "admin-settings-email", path: "/admin/settings/email" },
  { name: "admin-settings-agents", path: "/admin/settings/agents" },
  { name: "admin-settings-terminal", path: "/admin/settings/terminal" },
  { name: "admin-settings-limits", path: "/admin/settings/limits" },
  { name: "admin-secret-stores", path: "/admin/secret-stores" },
  { name: "admin-plugins", path: "/admin/plugins" },
  { name: "admin-plugin-detail", path: "/admin/plugins/github" },
  { name: "admin-addon-sources", path: "/admin/addon-sources" },
  { name: "admin-connectors", path: "/admin/connectors" },
  { name: "admin-storage", path: "/admin/storage" },
  { name: "admin-host-claims", path: "/admin/host-claims" },
  { name: "admin-instances", path: "/admin/instances" },
  { name: "admin-oauth-clients", path: "/admin/oauth-clients" },
  { name: "admin-trusted-keys", path: "/admin/trusted-keys" },
  { name: "admin-audit", path: "/admin/audit" },
  { name: "org-workspaces", path: "/o/acme" },
  { name: "org-api-keys", path: "/o/acme/api-keys" },
  { name: "org-apps", path: "/o/acme/apps" },
  { name: "org-audit", path: "/o/acme/audit" },
  { name: "org-channels", path: "/o/acme/channels" },
  { name: "org-credentials", path: "/o/acme/credentials" },
  { name: "org-governance", path: "/o/acme/governance" },
  { name: "org-instances", path: "/o/acme/instances" },
  { name: "org-members", path: "/o/acme/members" },
  { name: "org-groups", path: "/o/acme/groups" },
  { name: "org-group-manual", path: `/o/acme/groups/${id(401)}` },
  { name: "org-group-idp", path: `/o/acme/groups/${id(402)}` },
  { name: "org-policy", path: "/o/acme/policy" },
  { name: "org-settings", path: "/o/acme/settings" },
  { name: "org-directory", path: "/o/acme/settings/directory" },
  { name: "org-skills", path: "/o/acme/skills" },
  { name: "org-skill-libraries", path: "/o/acme/skills/libraries" },
  { name: "org-storage", path: "/o/acme/storage" },
  { name: "ws-overview", path: "/o/acme/w/main" },
  { name: "ws-welcome", path: "/o/acme/w/main/welcome" },
  { name: "ws-activity", path: "/o/acme/w/main/activity" },
  { name: "ws-agents", path: "/o/acme/w/main/agents" },
  { name: "ws-agent", path: "/o/acme/w/main/agents/daneel" },
  { name: "ws-approvals", path: "/o/acme/w/main/approvals" },
  { name: "ws-app-connect", path: "/o/acme/w/main/credentials?tab=apps&connect=github" },
  { name: "ws-credentials", path: "/o/acme/w/main/credentials" },
  { name: "ws-policy", path: "/o/acme/w/main/policy" },
  { name: "ws-skills", path: "/o/acme/w/main/skills" },
  { name: "ws-settings", path: "/o/acme/w/main/settings" },
];

/** Routes that kept their address, and the shared pages, from the list above. */
const KEPT = new Set([
  "root", "sign-in", "cli-login", "join", "link-channel", "setup-error", "account", "account-security", "account-api-keys",
  "account-channels", "account-delete", "admin-settings", "org-workspaces", "org-settings", "ws-overview", "ws-welcome",
  "ws-activity", "ws-agents", "ws-agent", "ws-approvals", "ws-skills", "ws-settings",
]);

/** Every old address of a page that moved (C-960): it must still open, on its new page. */
export const LEGACY_ROUTES: RouteEntry[] = PRE_IA_ROUTES.filter((r) => !KEPT.has(r.name));

/** The topic pages of P5-IA-1, one entry per tab (`14 §Topics per level`). */
const tab = (base: string, tabs: string[], prefix: string): RouteEntry[] => tabs.map((t) => ({ name: `${prefix}-${t}`, path: `${base}?tab=${t}` }));
const TOPIC_ROUTES: RouteEntry[] = [
  { name: "admin-overview", path: "/admin" },
  ...tab("/admin/agents", ["defaults", "byo", "computers", "skills"], "admin-agents"),
  ...tab("/admin/apps", ["apps", "oauth-clients", "self-hosted", "advanced"], "admin-apps"),
  { name: "admin-app-package", path: "/admin/apps/packages/github" },
  ...tab("/admin/ai-models", ["endpoints", "rules"], "admin-ai-models"),
  { name: "admin-internet-access", path: "/admin/internet-access" },
  ...tab("/admin/people", ["users", "organizations", "sign-in", "new-users"], "admin-people"),
  { name: "admin-organization", path: `/admin/people/organizations/${id(1)}` },
  { name: "admin-chat-platforms", path: "/admin/chat-platforms" },
  ...tab("/admin/data", ["retention", "storage", "audit", "usage"], "admin-data"),
  ...tab("/admin/platform", ["runners", "secret-stores", "email", "limits"], "admin-platform"),
  ...tab("/o/acme/agents", ["agents", "byo", "skills"], "org-agents"),
  { name: "org-skill-libraries", path: "/o/acme/agents/skills/libraries" },
  ...tab("/o/acme/apps", ["accounts", "apps", "oauth-clients", "self-hosted"], "org-apps"),
  { name: "org-ai-models", path: "/o/acme/ai-models" },
  { name: "org-internet-access", path: "/o/acme/internet-access" },
  ...tab("/o/acme/people", ["members", "groups"], "org-people"),
  { name: "org-group-manual", path: `/o/acme/people/groups/${id(401)}` },
  { name: "org-group-idp", path: `/o/acme/people/groups/${id(402)}` },
  { name: "org-chat-platforms", path: "/o/acme/chat-platforms" },
  ...tab("/o/acme/data", ["retention", "storage", "audit", "usage"], "org-data"),
  ...tab("/o/acme/w/main/activity", ["requests", "usage"], "ws-activity"),
  ...tab("/o/acme/settings", ["general", "api-keys", "danger"], "org-settings"),
  ...tab("/o/acme/w/main/apps", ["accounts", "apps"], "ws-apps"),
  // (P5-UX-3) The connect journey: a given app (the Connect button, with the one agent preselected), the app step (empty
  // `connect`, from Accounts › Add) and the agent's Access tab.
  { name: "ws-connect-journey", path: "/o/acme/w/main/apps?tab=apps&connect=github" },
  { name: "ws-connect-journey-apps", path: "/o/acme/w/main/apps?tab=accounts&connect=" },
  { name: "ws-connect-journey-agent", path: "/o/acme/w/main/agents/daneel?tab=access&connect=github" },
  // (P5-UX-9) The per-app setup pages: a workspace owner's (Linear: sign-in needs a client) and an organization's (AWS: external id).
  { name: "ws-app-setup", path: "/o/acme/w/main/apps/linear" },
  { name: "org-app-setup", path: "/o/acme/apps/aws-keys" },
  { name: "ws-internet-access", path: "/o/acme/w/main/internet-access" },
  { name: "ws-people", path: "/o/acme/w/main/people" },
  ...tab("/o/acme/w/main/settings", ["general", "api-keys", "audit", "danger"], "ws-settings"),
];

/** Every dashboard route, with a heading to wait for. Parametrised segments use the mock org, workspace and agent. */
export const ROUTES: RouteEntry[] = [...PRE_IA_ROUTES.filter((r) => KEPT.has(r.name) && !/^ws-settings$|^org-settings$/.test(r.name)), ...TOPIC_ROUTES];
