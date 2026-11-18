import { expect, type Page } from "@playwright/test";

import { agent, extraAccounts, id, mockApi, now, ws } from "./mock-api";
import { titled, type Scheme } from "./open";

// P5-WEBHOOK-4: the Outgoing webhook app against the mocked API. P5-WEBHOOK-2 (the API) is not merged: the routes of C-909 and
// C-912 are answered here, and every request is recorded.

export type Sent = { method: string; path: string; body: Record<string, unknown> | null };

export const URL_CANARY = "https://hooks.slack.com/services/T0CANARY/B0CANARY/xyzCANARYxyz";
export const SECRET_CANARY = "secret-canary-value-7731";
export const SIGNING = "whsec_signing_canary_5521";
export const CONN = id(600);

const slackConfig = {
  version: 1, method: "POST", content_type: "application/json", template: '{\n  "text": {{text}}\n}',
  slots: [{ name: "text", type: "string", required: true, description: "The message", example: "Deploy 42 failed" }],
  headers: [], description: "", signing: false, rate: { per_minute: null, per_day: null }, preset: "slack",
};
export const presets = [
  { id: "slack", name: "Slack", suggest_for_hosts: ["hooks.slack.com"], secret_value_names: [], config: slackConfig },
  { id: "generic-json", name: "Generic JSON", secret_value_names: [], config: { ...slackConfig, template: "{{payload}}", slots: [{ name: "payload", type: "json", required: true, description: "", example: { a: 1 } }], preset: "generic-json" } },
];

const webhookApp = {
  app_id: "webhook", available: true, category: "developer", connection_count: 1, description: "Agents call a webhook you set up", has_dark_icon: true,
  name: "Outgoing webhook", package_id: "webhook", package_version: "1.0.0", app_restrictions: { custom_only: false, block_untooled: false }, serves_library: false,
};
const method = {
  id: "url", kind: "fields", access_levels: [], default_scopes: [], scopes: [], recommended: true, label: "Webhook address", description: null,
  fields: [{ name: "name", label: "Name", type: "text", store: "metadata", required: true }, { name: "url", label: "URL", type: "secret", store: "credentials", required: true }],
  probe_scopes: [], audience: "person", scope_presets: { read: [], write: [] }, show_context: [], has_probe: false,
};

/** Opens a route with the API mocked, the webhook routes added and the colour scheme emulated. */
export async function openWebhook(page: Page, path: string, scheme: Scheme, opts: { withAccount?: boolean } = {}) {
  const sent: Sent[] = [];
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  // A render error is caught by the app's error boundary and only reaches the console.
  page.on("console", (m) => {
    if (m.type() === "error" && !/Failed to load resource/.test(m.text())) errors.push(m.text().slice(0, 400));
  });
  extraAccounts.length = 0;
  await mockApi(page);
  if (opts.withAccount) {
    const who = { agent: { id: agent.id, name: agent.name, handle: agent.handle }, workspace: { id: ws.id, name: ws.name } };
    extraAccounts.push({
      ref: { type: "connection", id: CONN }, kind: "app", name: "ops-alerts", scope: "workspace", levels: ["read", "ask", "act"],
      app: { app_id: "webhook", name: "Outgoing webhook", custom_only: false }, health: { state: "unknown" }, agents: [{ ...who, level: "read" }],
      can: { give_access: true, change_access: true, manage: true },
    });
  }
  let view: Record<string, unknown> = { ...slackConfig, host: "hooks.slack.com", secret_value_names: [], effective_rate: { per_minute: 30, per_day: 500, source: "org" } };
  await page.route("**/v1/**", async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const m = req.method();
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    const body = m === "GET" ? null : (req.postDataJSON() as Record<string, unknown> | null);
    // The package's icon is P5-WEBHOOK-1's; a 1x1 PNG stands in.
    if (p === "/v1/apps/webhook/icon") return route.fulfill({ status: 200, contentType: "image/png", body: Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==", "base64") });
    if (p === "/v1/apps" && m === "GET") return json({ items: [webhookApp] });
    if (p === "/v1/apps/webhook" && m === "GET") return json({ ...webhookApp, connect: [method], credentials: [], hosts: [], metadata: [], oauth_clients: [], tools: [{ id: "send", name: "Send", group: "write", description: "Send one message." }] });
    if (p === "/v1/apps/webhook/connect-plan") return json({ app_id: "webhook", available: true, suggested_method: "url", methods: [{ id: "url", kind: "fields", label: "Webhook address", description: null, recommended: true, audience: "person", needs: [], ready: true, scope_presets: { read: [], write: [] } }] });
    if (p === "/v1/apps/webhook/webhook-presets") return json(presets);
    if (p === "/v1/apps/webhook/webhook-preview" && m === "POST") {
      sent.push({ method: m, path: p, body });
      const cfg = (body as { config: { template: string } }).config;
      if (cfg.template.includes("{{nope}}")) return json({ error: { code: "webhook_config_invalid", message: "x", details: { fields: [{ path: "/template", code: "placeholder_unknown" }] } } }, 422);
      return json({ method: "POST", content_type: "application/json", headers: [], body: '{\n  "text": "Deploy 42 failed"\n}' });
    }
    if (p === "/v1/apps/webhook/connect" && m === "POST") {
      sent.push({ method: m, path: p, body });
      return json({ kind: "connected", connection: { id: CONN, scope: "workspace", app_id: "webhook", scopes: [], missing_scopes: [], label: "ops-alerts", status: "active" }, grants: [{ agent_id: agent.id }], missing_scopes: [], ...(JSON.stringify(body).includes('"generate"') ? { signing_secret: SIGNING } : {}) });
    }
    if (p === `/v1/connections/${CONN}/webhook`) {
      if (m === "PUT") {
        sent.push({ method: m, path: p, body });
        const { secret_values: _s, signing_secret: _g, ...config } = body as Record<string, unknown>;
        void _s;
        void _g;
        view = { ...view, ...config, host: "hooks.slack.com", secret_value_names: [], effective_rate: { per_minute: 30, per_day: 500, source: "org" } };
        return json(view);
      }
      return json(view);
    }
    if (p === `/v1/connections/${CONN}/webhook/test` && m === "POST") {
      sent.push({ method: m, path: p, body });
      return body?.dry_run
        ? json({ method: "POST", content_type: "application/json", headers: [], body: '{"text":"Deploy 42 failed"}' })
        : json({ status: 200, content_type: "text/plain", body: "ok", truncated: false, delivery_id: "7d2c1d9e-0000-4000-8000-000000000001" });
    }
    if (p === `/v1/orgs/${id(1)}` && m === "PATCH") {
      sent.push({ method: m, path: p, body });
      return json({});
    }
    return route.fallback();
  });
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await titled(page);
  void now;
  return { sent, errors };
}
