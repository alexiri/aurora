import type { Page } from "@playwright/test";

import { agent, extraAccounts, id, ws } from "./mock-api";
import { openRoute, type Scheme } from "./open";

// P5-UX-3: the connect journey ("Give <Agent> <App>", C-914) against the mocked API. `mockApi` serves the apps, the connect
// plans and the accounts list; this adds what a person's saves change: the connect request, a website's one request
// (C-988) and the agent's access are recorded, and the accounts list then shows the new account with its level.

export type Saved = { path: string; method: string; body: Record<string, unknown> };
type Level = "read" | "ask" | "act";

export async function openJourney(page: Page, path: string, scheme: Scheme) {
  const saved: Saved[] = [];
  await openRoute(page, path, scheme);
  await page.route("**/v1/**", async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const m = req.method();
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    const who = { agent: { id: agent.id, name: agent.name, handle: agent.handle }, workspace: { id: ws.id, name: ws.name } };
    const can = { give_access: true, change_access: true, manage: true };
    if (m === "POST" && /^\/v1\/apps\/[^/]+\/connect$/.test(p)) {
      const body = req.postDataJSON() as Record<string, unknown>;
      // A token the app refuses (B4): the value must stay in the field and the error is said under it.
      if ((body.fields as { access_token?: string } | undefined)?.access_token === "refused-token") {
        return json({ error: { code: "connect_probe_failed", message: "refused", details: { cause: "rejected" } } }, 422);
      }
      saved.push({ path: p, method: m, body });
      const appId = p.split("/")[3]!;
      const level = ((body.grant as { level?: Level } | undefined)?.level ?? "read") as Level;
      const name = appId === "github" ? "GitHub" : appId;
      extraAccounts.push({
        ref: { type: "connection", id: id(500) }, kind: "app", name: "acme-prod", scope: "workspace", levels: ["read", "ask", "act"],
        app: { app_id: appId, name, custom_only: false }, health: { state: "not_used" }, agents: [{ ...who, level }], can, given: true,
      });
      return json({
        kind: "connected", connection: { id: id(500), scope: "workspace", app_id: appId, scopes: [], missing_scopes: [], label: "acme", status: "active" },
        grants: [{ agent_id: agent.id }], missing_scopes: [],
      });
    }
    if (m === "POST" && /^\/v1\/workspaces\/[^/]+\/secrets$/.test(p)) {
      const body = req.postDataJSON() as Record<string, unknown>;
      saved.push({ path: p, method: m, body });
      const name = String(body.name ?? "Website");
      extraAccounts.push({
        ref: { type: "secret", id: id(501) }, kind: "website", name, scope: "workspace", levels: [], health: { state: "not_used" },
        agents: [{ ...who, level: "use" }], can, given: true,
      });
      return json({ id: id(501), name, kind: "generic", scope: "workspace", created_at: new Date().toISOString() });
    }
    // (P5-POLISH-4, C-1030) Ask an admin to set an app up, and Undo.
    if (/^\/v1\/workspaces\/[^/]+\/apps\/[^/]+\/setup-request$/.test(p)) {
      saved.push({ path: p, method: m, body: {} });
      if (m === "DELETE") return route.fulfill({ status: 204 });
      return json({ id: id(900), org_id: id(1), app_id: p.split("/")[5], app_name: p.split("/")[5], status: "open", askers: [], needs: ["oauth_client"], unavailable: false, created_at: new Date().toISOString() });
    }
    if (m === "PUT"&& /^\/v1\/agents\/[^/]+\/grants\//.test(p)) {
      saved.push({ path: p, method: m, body: req.postDataJSON() as Record<string, unknown> });
      return json({});
    }
    return route.fallback();
  });
  return saved;
}
