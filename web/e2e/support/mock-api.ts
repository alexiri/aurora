import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { Page } from "@playwright/test";

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
}));

const connection = (n: number, appId: string, label: string) => ({
  app_id: appId, app_version: "1.0.0", created_at: now, id: id(n), injecting: [], label, metadata: {}, missing_scopes: [], org_id: id(1),
  package_id: appId, scope: "workspace", scopes: [], status: "active", updated_at: now, workspace_id: id(2),
});

const approval = {
  id: id(60), org_id: id(1), workspace_id: id(2), status: "pending", created_at: now, deadline: later, host: "api.github.com", method: "PUT",
  path: "/repos/acme/web/pulls/42/merge", app_id: "github", agent: { id: id(11), handle: "daneel", name: "Daneel" },
  summary: { source: "app", title: "Merge pull request #42", fields: [{ label: "Repository", value: "acme/web" }] },
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
  default: { endpoint_id: "litellm", model_id: "claude-sonnet-4", provider: "litellm", source: "endpoint_default" },
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
      id: "llm", status: "warning", reason: "no_default_model",
      checks: [
        setupCheck("endpoint:litellm", "ok", "endpoint_reachable", "AIGW answers.", { value: "https://aigw.example.org" }),
        setupCheck("default_model:litellm", "warning", "no_default_model", "AIGW has no default model. Agents would need a model chosen one by one.", { helm: "llm.endpoints[].default_model" }),
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

export async function mockApi(
  page: Page,
  /** (FIX-029) Extra canned responses: a regex or exact path → JSON body, or `{status, body}`. */
  overrides: Record<string, unknown> = {},
) {
  const exact = new Map<string, unknown>();
  const patterns: [RegExp, unknown][] = [];
  for (const [k, v] of Object.entries(overrides)) {
    if (/[\^*+()[\]{}|\\]/.test(k)) patterns.push([new RegExp(k.startsWith("/") ? `^${k}$` : k), v]);
    else exact.set(k, v);
  }
  await page.route("**/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const p = url.pathname;
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (p.includes("/stream")) return route.abort();
    const hit = exact.get(p) ?? patterns.find(([re]) => re.test(p))?.[1];
    if (hit !== undefined) {
      const { status = 200, body = hit, ...rest } = hit && typeof hit === "object" && "body" in (hit as Record<string, unknown>) ? (hit as Record<string, unknown>) : { body: hit };
      void rest;
      return json(body, status as number);
    }
    const icon = p.match(/^\/v1\/apps\/([^/]+)\/icon$/);
    if (icon) {
      const file = url.searchParams.get("variant") === "dark" ? "icon-dark.png" : "icon.png";
      return route.fulfill({ status: 200, contentType: "image/png", body: readFileSync(join(BUILTIN, icon[1], "assets", file)) });
    }
    if (p === "/v1/me") {
      return json({ user, memberships: [{ org, status: "active", workspaces: [{ ...ws, personal: false }] }], default_org_id: id(1), client_address: "203.0.113.7" });
    }
    // P4-ONB-2: onboarding progress (seen, so `/` stays on the workspace), the guide's readiness and the setup wizard.
    if (p === "/v1/me/onboarding") return json({ welcome_seen: true, guide_dismissed: [], tour_completed_at: null });
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
    if (p === "/v1/llm/providers") return json({ providers: [] });
    if (p.endsWith("/governance/effective")) return json(effective);
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
    if (p === "/v1/admin/placement/preview") return json({ name: "bobj", slug: "bobj", used: 0, unresolved: [] });
    if (p === "/v1/admin/placement/choices") {
      return json({ workspaces: [{ id: id(2), slug: "main", name: "Main workspace" }], secrets: [{ id: id(300), name: "org-llm-key", kind: "llm" }], connections: [] });
    }
    if (p === "/v1/admin/orgs") return json({ items: [{ id: id(1), slug: "users", name: "Users", frozen: null, created_at: now }], next_cursor: null });
    if (p === "/v1/admin/connectors") return json({ items: [mattermostConnector] });
    if (p === "/v1/admin/connector-types") return json({ host_running: true, items: [mattermostType] });
    // FIX-025 (C-586) + FIX-029 (C-614): the runners admin view and its per-runner failures.
    if (p === "/v1/admin/runners") return json({ items: [] });
    if (/^\/v1\/connectors\/[^/]+\/icon$/.test(p)) return route.fulfill({ status: 404, contentType: "application/json", body: "{}" });
    if (/^\/v1\/admin\/plugins\/[^/]+$/.test(p)) {
      const pkg = PACKAGES.find((x) => x.id === p.split("/")[4]) ?? PACKAGES[3];
      return json({
        id: id(70), package_id: pkg.id, version: "1.0.0", contract: 1, digest: "sha256:0000", publisher: "aurora", source: "builtin", status: "approved",
        created_at: now, signer: { kind: "builtin", name: "Aurora" },
        apps: [{ app_id: pkg.id, name: pkg.name, ...(pkg.id === "github" ? { library_host: "api.github.com" } : {}) }], host_claims: [], violations: [], widenings: [], impact: null,
        permissions: { hooks: [], context: [], egress: { "api.github.com": ["GET"] } },
      });
    }
    if (p === "/v1/instance") {
      return json({
        version: "0.1.0", signup_mode: "closed", setup_errors: [],
        features: { hosted: true, previews: true, terminal: true },
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
          id: "token", kind: "fields", access_levels: [], default_scopes: [], scopes: [], recommended: false, label: "Personal access token",
          description: "Acts as you, on everything your account can reach. The quickest way to start for personal use; a fine-grained token can be limited to chosen repositories.",
          fields: [{ name: "access_token", label: "Personal access token", type: "secret", store: "credentials", required: true, placeholder: "Fine-grained or classic personal access token", help_url: "https://github.com/settings/personal-access-tokens" }],
        },
        {
          id: "oauth", kind: "oauth2", access_levels: [], default_scopes: ["read:user"], recommended: false, label: "OAuth", client_key: "github", callback: "query",
          description: "Sign in with GitHub and act as you, with the scopes you tick. Needs an OAuth app registered by an administrator.",
          scopes: [{ scope: "read:user", name: "Read your profile", access: "read" }], fields: [],
        },
      ];
      return json({ ...apps.find((x) => x.app_id === a.id), connect, credentials: [], hosts: [], metadata: [], oauth_clients: [], tools: [] });
    }
    if (p.endsWith("/connections")) return json({ items: [connection(40, "github", "acme-bot"), connection(41, "linear", "Product team")] });
    if (p.endsWith("/approvals")) return json({ items: url.searchParams.get("status") === "pending" ? [approval] : [], next_cursor: null });
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
    if (/^\/v1\/orgs\/[^/]+$/.test(p)) return json(org);
    return json({ items: [] });
  });
}

/** Every dashboard route, with a heading to wait for. Parametrised segments use the mock org, workspace and agent. */
export const ROUTES: { name: string; path: string; admin?: boolean }[] = [
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
  { name: "admin-placement", path: "/admin/placement" },
  { name: "admin-plugins", path: "/admin/plugins" },
  { name: "admin-plugin-detail", path: "/admin/plugins/github" },
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
  { name: "ws-app-connect", path: "/o/acme/w/main/apps/github/connect" },
  { name: "ws-credentials", path: "/o/acme/w/main/credentials" },
  { name: "ws-policy", path: "/o/acme/w/main/policy" },
  { name: "ws-skills", path: "/o/acme/w/main/skills" },
  { name: "ws-settings", path: "/o/acme/w/main/settings" },
];
