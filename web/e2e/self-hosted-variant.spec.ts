import { expect, test, type Page } from "@playwright/test";

import { openJourney } from "./support/journey";
import { id, now } from "./support/mock-api";
import { overflow } from "./support/open";
import { serious } from "./support/axe";

// FIX-202 (owner 2026-11-21): a family is ready when any variant is usable. CERN's Jira Data Center is registered (active)
// and Jira Cloud has no sign-in app yet: Jira sits under Ready, and its tile opens the Data Center token form with the
// server, never Cloud. The spec overrides the apps list, the Jira detail and plans and the registrations itself (the shared
// mock has no family of this kind).

const AGENT_ACCESS = "/o/acme/w/main/agents/daneel?tab=access";
const SERVER = id(700);

const family = (variant_label: string, instance_bound: boolean) => ({ id: "jira", name: "Jira", variant_label, instance_bound });
const item = (app_id: string, name: string, fam: ReturnType<typeof family>, readiness: unknown) => ({
  app_id, available: true, category: "developer", connection_count: 0, description: `${name} issues`, has_dark_icon: false, name,
  package_id: "jira", package_version: "1.0.0", app_restrictions: { custom_only: false, block_untooled: false }, serves_library: false,
  summary: `${name} issues`, accounts: [], family: fam, readiness,
});
const CLOUD = item("jira", "Jira Cloud", family("Cloud", false), {
  state: "needs_admin", needs: ["oauth_client"], who_can: { action: "register_oauth_client", role: "org_admin", people: [{ name: "Ada Lovelace" }], you_can: false },
});
const DC = item("jira-dc", "Jira Data Center", family("Data Center", true), { state: "ready" });
const method = {
  id: "token", kind: "fields", label: "Personal access token", description: null, recommended: true, audience: "person", access_levels: [], default_scopes: [], scopes: [],
  probe_scopes: [], scope_presets: { read: [], write: [] }, show_context: [], permission_hints: [], has_probe: false, client_key: null, callback: null,
  fields: [{ name: "access_token", label: "Personal access token", type: "secret", store: "credentials", required: true }],
};

async function overrides(page: Page) {
  const json = (body: unknown) => ({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
  await page.route("**/v1/**", async (route) => {
    const p = new URL(route.request().url()).pathname;
    if (route.request().method() !== "GET") return route.fallback();
    if (p === "/v1/apps") return route.fulfill(json({ items: [CLOUD, DC] }));
    if (p === "/v1/apps/jira-dc")
      return route.fulfill(json({ ...DC, connect: [method], credentials: [], hosts: [], metadata: [], oauth_clients: [], tools: [{ id: "get_issue", name: "Get an issue", group: "read", description: "Read an issue." }], instances: { label: "Jira host", default_host: null } }));
    if (p === "/v1/apps/jira-dc/connect-plan")
      return route.fulfill(json({ app_id: "jira-dc", available: true, suggested_method: "token", methods: [{ id: "token", kind: "fields", label: "Personal access token", description: null, recommended: true, audience: "person", needs: [], ready: true, scope_presets: { read: [], write: [] } }] }));
    if (/^\/v1\/orgs\/[^/]+\/app-instances$/.test(p))
      return route.fulfill(
        json({
          items: [{ id: SERVER, org_id: id(1), package_id: "jira", app_id: "jira-dc", host: "its.cern.ch", base_path: "/jira", base_url: "https://its.cern.ch/jira", label: "CERN Jira", status: "active", created_by: null, created_at: now, updated_at: now, connections: 0, builtin: false }],
          next_cursor: null,
        }),
      );
    return route.fallback();
  });
}

for (const width of [1280, 390]) {
  test.describe(`${width} px`, () => {
    test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

    test("Give an agent an app → Jira is Ready → the Data Center token form for the registered server", async ({ page }) => {
      const saved = await openJourney(page, AGENT_ACCESS, "light");
      await overrides(page);
      await page.reload();
      await page.getByRole("button", { name: "Give Daneel an app" }).first().click();
      const dialog = page.getByTestId("ws-connect-journey");
      await dialog.getByRole("searchbox", { name: "Search apps" }).fill("ji");
      // Ready · 1 and Needs an admin · 0: the registered server makes the family ready.
      await expect(dialog.getByRole("tab", { name: "Ready · 1", selected: true })).toBeVisible();
      await expect(dialog.getByRole("tab", { name: "Needs an admin · 0" })).toBeVisible();
      await dialog.getByRole("region", { name: /^Ready to connect/ }).getByRole("button", { name: "Jira", exact: true }).click();
      // One usable choice: no question, no Cloud step; the Data Center token form.
      await expect(dialog.getByRole("radiogroup", { name: "Where does your Jira run?" })).toHaveCount(0);
      await expect(dialog.getByText("Jira · Data Center")).toBeVisible();
      const field = dialog.getByLabel("Personal access token", { exact: true });
      await expect(field).toBeVisible();
      expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
      expect(await serious(page)).toEqual([]);
      await field.fill("not-a-real-token");
      await dialog.getByRole("button", { name: "Connect", exact: true }).click();
      await expect.poll(() => saved.length).toBeGreaterThan(0);
      expect(saved[0]!.path).toBe("/v1/apps/jira-dc/connect");
      expect(saved[0]!.body).toMatchObject({ instance_id: SERVER });
    });
  });
}
