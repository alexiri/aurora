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

export const org = { id: id(1), slug: "acme", name: "Acme Labs", my_role: "owner", app_posture: "open", external_id: "x", settings: {}, created_at: now };
export const ws = { id: id(2), org_id: id(1), slug: "main", name: "Main workspace", my_role: "owner", memory_language: "en", timezone: "UTC", created_at: now, updated_at: now };
export const agent = {
  id: id(11), org_id: id(1), workspace_id: id(2), handle: "daneel", name: "Daneel", kind: "hosted", is_default: true,
  avatar_url: null, brief: "Triage the team's pull requests and issues.", harness: null, model_provider: null, model_endpoint_id: null, model_id: null,
  effort: null, proactive_messaging: "ask", created_at: now, updated_at: now,
};

/** Real built-in packages: id, display name (plugin.yaml `name:`). Every one ships icon.png and icon-dark.png. */
export const PACKAGES: { id: string; name: string; description: string }[] = [
  { id: "atlassian", name: "Atlassian", description: "Jira and Confluence Cloud" },
  { id: "aws-keys", name: "AWS (access keys)", description: "AWS APIs signed with an access key" },
  { id: "aws-role", name: "AWS (assume role)", description: "AWS APIs through an assumed IAM role" },
  { id: "github", name: "GitHub", description: "Repositories, issues, pull requests, files and Actions runs" },
  { id: "github-app", name: "GitHub App", description: "GitHub through an installed GitHub App" },
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

export const user ={ id: id(99), display_name: "Ada Lovelace", email: "ada@example.test", is_instance_admin: true };

export async function mockApi(page: Page) {
  await page.route("**/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const p = url.pathname;
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (p.includes("/stream")) return route.abort();
    const icon = p.match(/^\/v1\/apps\/([^/]+)\/icon$/);
    if (icon) {
      const file = url.searchParams.get("variant") === "dark" ? "icon-dark.png" : "icon.png";
      return route.fulfill({ status: 200, contentType: "image/png", body: readFileSync(join(BUILTIN, icon[1], "assets", file)) });
    }
    if (p === "/v1/me") {
      return json({ user, memberships: [{ org, status: "active", workspaces: [ws] }], default_org_id: id(1), client_address: "203.0.113.7" });
    }
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
    if (/^\/v1\/admin\/plugins\/[^/]+$/.test(p)) {
      const pkg = PACKAGES.find((x) => x.id === p.split("/")[4]) ?? PACKAGES[3];
      return json({
        id: id(70), package_id: pkg.id, version: "1.0.0", contract: 1, digest: "sha256:0000", publisher: "aurora", source: "builtin", status: "approved",
        created_at: now, signer: { kind: "builtin", name: "Aurora" }, apps: [{ app_id: pkg.id, name: pkg.name }], host_claims: [], violations: [], widenings: [], impact: null,
        permissions: { hooks: [], context: [], egress: { "api.github.com": ["GET"] } },
      });
    }
    if (p === "/v1/instance") {
      return json({ version: "0.1.0", signup_mode: "closed", features: { hosted: true }, setup_errors: [], oidc_providers: [{ id: "corp", display_name: "Corporate SSO" }] });
    }
    if (p === "/v1/orgs") return json({ items: [org] });
    if (p === "/v1/resolve") return json({ org, workspace: ws });
    if (p === "/v1/apps") return json({ items: apps });
    if (/^\/v1\/apps\/[^/]+$/.test(p)) {
      const a = PACKAGES.find((x) => x.id === p.split("/")[3]) ?? PACKAGES[3];
      // Connect methods as in plugins/builtin/github/plugin.yaml (token form and OAuth).
      const connect = [
        {
          id: "token", kind: "fields", access_levels: [], default_scopes: [], scopes: [],
          fields: [{ name: "access_token", label: "Personal access token", type: "secret", store: "credentials", required: true, placeholder: "Fine-grained or classic personal access token", help_url: "https://github.com/settings/personal-access-tokens" }],
        },
      ];
      return json({ ...apps.find((x) => x.app_id === a.id), connect, credentials: [], hosts: [], metadata: [], oauth_clients: [], tools: [] });
    }
    if (p.endsWith("/connections")) return json({ items: [connection(40, "github", "acme-bot"), connection(41, "linear", "Product team")] });
    if (p.endsWith("/approvals")) return json({ items: url.searchParams.get("status") === "pending" ? [approval] : [], next_cursor: null });
    if (p.endsWith("/workspaces")) return json({ items: [ws] });
    if (p.endsWith("/agents")) return json({ items: [agent] });
    if (p === `/v1/workspaces/${ws.id}`) return json(ws);
    if (p.endsWith("/storage") && p.startsWith("/v1/orgs/")) {
      return json({ org_id: id(1), used_bytes: 850_000_000, quota_bytes: 1_000_000_000, files: 12, purged_files: 3, notice: "warning", by_workspace: [{ workspace_id: id(2), workspace_name: "Main workspace", direction: "in", files: 12, bytes: 850_000_000 }] });
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
  { name: "admin-users", path: "/admin/users" },
  { name: "admin-orgs", path: "/admin/orgs" },
  { name: "admin-plugins", path: "/admin/plugins" },
  { name: "admin-plugin-detail", path: "/admin/plugins/github" },
  { name: "admin-connectors", path: "/admin/connectors" },
  { name: "admin-storage", path: "/admin/storage" },
  { name: "admin-host-claims", path: "/admin/host-claims" },
  { name: "admin-instances", path: "/admin/instances" },
  { name: "admin-oauth-clients", path: "/admin/oauth-clients" },
  { name: "admin-trusted-keys", path: "/admin/trusted-keys" },
  { name: "org-workspaces", path: "/o/acme" },
  { name: "org-api-keys", path: "/o/acme/api-keys" },
  { name: "org-apps", path: "/o/acme/apps" },
  { name: "org-audit", path: "/o/acme/audit" },
  { name: "org-channels", path: "/o/acme/channels" },
  { name: "org-credentials", path: "/o/acme/credentials" },
  { name: "org-governance", path: "/o/acme/governance" },
  { name: "org-instances", path: "/o/acme/instances" },
  { name: "org-members", path: "/o/acme/members" },
  { name: "org-policy", path: "/o/acme/policy" },
  { name: "org-settings", path: "/o/acme/settings" },
  { name: "org-skills", path: "/o/acme/skills" },
  { name: "org-storage", path: "/o/acme/storage" },
  { name: "ws-overview", path: "/o/acme/w/main" },
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
