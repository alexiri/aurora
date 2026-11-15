import { expect, type Page } from "@playwright/test";

import { agent, mattermostConnector, mattermostType, mockApi, org } from "./mock-api";
import type { Scheme } from "./open";

// FIX-127: a stateful mock of the Mattermost path — instance (add the connector), org (probe,
// create the integration) and the done screen (attach an agent) — layered over the shared mock.
// Routes registered later win in Playwright, `fallback()` hands everything else to `mockApi`.

const now = new Date().toISOString();

export interface MattermostMock {
  /** Every mutating request, as `METHOD path` with its parsed JSON body. */
  calls: { key: string; body: unknown }[];
}

const integration = {
  id: "00000000-0000-4000-8000-000000000701", org_id: org.id, connector_id: "mattermost", tenant_ref: "chat.example.com/alpha",
  tenant_label: "@daneel on chat.example.com", config: { server_url: "https://chat.example.com", team: "alpha" }, shared_bot: { handle: "daneel", display_name: "Daneel" },
  status: "active", status_reason: null, created_at: now, updated_at: now,
};

const presence = {
  id: "00000000-0000-4000-8000-000000000702", agent_id: agent.id, integration_id: integration.id, connector_id: "mattermost",
  tenant_label: integration.tenant_label, mode: "shared", handle: "@daneel", status: "active", status_reason: null, created_at: now,
};

/** Integration-setup fields of the real Mattermost connector (docs/20) as `GET /v1/connectors` serves them. */
const integrationSetup = {
  fields: [
    { name: "server_url", label: "Your Mattermost server", type: "text", required: true, store: "config", placeholder: "https://chat.example.com" },
    { name: "team", label: "Team", type: "text", required: true, store: "config", choices: "from_validate" },
    { name: "bot_token", label: "Bot access token", type: "secret", required: true, store: "credentials" },
    { name: "admin_token", label: "System-admin personal access token (optional)", type: "secret", required: false, store: "credentials",
      help: "Lets Aurora reply privately and register /aurora." },
  ],
};

export interface FlowOptions {
  /** Start with the connector already added (the org half of the journey). */
  added?: boolean;
  /** The availability state while it is not added. */
  state?: "not_added" | "waiting_for_host" | "disabled" | "needs_approval";
  /** The agent already has a shared presence (the agent-tab screens). */
  attached?: boolean;
  /** The org has no integration at all (the agent-tab empty state). */
  noIntegration?: boolean;
  /** Make `GET /integrations/{id}/spaces` refuse. */
  spaces?: "link_required";
}

export async function mockMattermostFlow(page: Page, opts: FlowOptions = {}): Promise<MattermostMock> {
  const mock: MattermostMock = { calls: [] };
  let added = opts.added === true;
  let created = opts.attached === true;
  let attached = false;
  const bindings: Record<string, unknown>[] = [];
  await page.route("**/v1/**", async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const m = req.method();
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (m !== "GET") {
      let body: unknown = null;
      try {
        body = req.postDataJSON();
      } catch {
        body = null;
      }
      mock.calls.push({ key: `${m} ${p}`, body });
    }
    if (p === "/v1/admin/connector-types") return json({ host_running: true, items: [{ ...mattermostType, added }] });
    if (p === "/v1/admin/connectors" && m === "POST") {
      added = true;
      return json({ connector: mattermostConnector }, 201);
    }
    if (p === "/v1/admin/connectors") return json({ items: added ? [mattermostConnector] : [] });
    if (p === "/v1/connectors") {
      return json({
        items: added
          ? [{ id: "mattermost", display_name: "Mattermost", icon_url: "/v1/connectors/mattermost/icon", setup_guide_md: "Create a bot account in Mattermost and copy its access token.",
              capabilities: { presence_shared: true }, integration_setup: integrationSetup, presence_setup: {} }]
          : [],
      });
    }
    if (p === "/v1/connector-availability") {
      return json({
        items: [{ type_id: "mattermost", name: "Mattermost", icon_url: null, setup_state: added ? "active" : (opts.state ?? "not_added"), connector_id: added ? "mattermost" : null }],
        instance_admins: [{ name: "Ada Lovelace" }],
      });
    }
    if (p === `/v1/orgs/${org.id}/channel-integrations:probe`) {
      return json({
        choices: { team: [{ value: "alpha", label: "Alpha Team" }, { value: "beta", label: "Beta Team" }] },
        tenant_label: "@daneel on chat.example.com", warnings: ["No admin token: replies to individuals arrive as DMs."],
      });
    }
    if (p === `/v1/orgs/${org.id}/channel-integrations` && m === "POST") {
      created = true;
      return json(integration, 201);
    }
    if (p === `/v1/orgs/${org.id}/channel-integrations`) return json({ items: created ? [integration] : [], next_cursor: null });
    // The agent's Channels tab (Part C): presences, presence options, my spaces, bindings.
    if (p === `/v1/agents/${agent.id}/presences` && m === "POST") {
      attached = true;
      return json(presence, 201);
    }
    if (p === `/v1/agents/${agent.id}/presences`) return json({ items: attached || opts.attached ? [presence] : [] });
    if (p === `/v1/agents/${agent.id}/presence-options`) {
      return json({
        items: opts.noIntegration
          ? []
          : [{ integration_id: integration.id, connector_id: "mattermost", connector_name: "Mattermost", tenant_label: integration.tenant_label,
              modes: ["shared"], presence_setup: {}, attached: attached || opts.attached === true }],
      });
    }
    if (p === `/v1/integrations/${integration.id}/spaces`) {
      if (opts.spaces === "link_required") return json({ error: { code: "link_required", message: "Link your chat account first." } }, 409);
      return json({
        items: [
          { space_ref: "chan-town", label: "town-square", kind: "channel", bot_present: true },
          { space_ref: "chan-secret", label: "secret-plans", kind: "private", bot_present: true },
        ],
      });
    }
    if (p === `/v1/agents/${agent.id}/space-bindings` && m === "POST") {
      const body = req.postDataJSON() as { space_ref: string; listen?: string };
      const label = body.space_ref === "chan-secret" ? "secret-plans" : "town-square";
      bindings.push({
        id: `00000000-0000-4000-8000-00000000080${bindings.length + 1}`, integration_id: integration.id, space_ref: body.space_ref, space_label: label,
        agent_id: agent.id, listen: body.listen ?? "mentions", followups_need_mention: false, active: true, bound_by: null, created_at: now,
      });
      return json(bindings[bindings.length - 1], 201);
    }
    if (p === `/v1/agents/${agent.id}/space-bindings`) return json({ items: bindings });
    return route.fallback();
  });
  return mock;
}

/** Opens a route with the Mattermost path mocked on top of the shared mock. */
export async function openMattermost(page: Page, path: string, scheme: Scheme, opts: FlowOptions = {}) {
  // Playwright runs the latest-registered handler first: the shared mock goes in first, ours on top.
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  const mock = await mockMattermostFlow(page, opts);
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, `${path} threw`).toEqual([]);
  return mock;
}
