import { expect, test, type Page } from "@playwright/test";

import { CONNECTOR_SCREENS, openConnectorScreen } from "./support/connectors";

// P2-UI stage 2: the designed screens never scroll horizontally at 390 px (the agent tab strip
// scrolls within itself). The API is mocked, so no backend is needed.

const now = new Date().toISOString();
const id = (n: number) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
const org = { id: id(1), slug: "acme", name: "Acme Labs", my_role: "owner", app_posture: "open", external_id: "x", settings: {}, created_at: now };
const ws = { id: id(2), org_id: id(1), slug: "main", name: "Main workspace", my_role: "owner", memory_language: "en", timezone: "UTC", created_at: now, updated_at: now };
const agent = {
  id: id(11), org_id: id(1), workspace_id: id(2), handle: "daneel", name: "Daneel", kind: "hosted", is_default: true,
  avatar_url: null, brief: "", harness: null, model_provider: null, model_endpoint_id: null, model_id: null,
  effort: null, proactive_messaging: "ask", created_at: now, updated_at: now,
};

async function mockApi(page: Page) {
  await page.route("**/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const p = url.pathname;
    if (p.includes("/stream")) return route.abort();
    const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
    if (p === "/v1/me") {
      return json({ user: { id: id(99), display_name: "Ada", email: "ada@example.test", is_instance_admin: false }, memberships: [{ org, status: "active" }], default_org_id: id(1) });
    }
    if (p === "/v1/orgs") return json({ items: [org] });
    if (p === "/v1/resolve") return json({ org, workspace: ws });
    if (p.endsWith("/workspaces")) return json({ items: [ws] });
    if (p.startsWith("/v1/orgs/")) return json(org);
    if (p === `/v1/workspaces/${ws.id}`) return json(ws);
    if (p.endsWith("/agents")) return json({ items: [agent] });
    if (p === "/v1/instance") {
      return json({ version: "0", signup_mode: "closed", features: { hosted: true }, setup_errors: [], oidc_providers: [{ id: "corp", display_name: "Corporate SSO" }] });
    }
    return json({ items: [] });
  });
}

const noPageScroll = (page: Page) => page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth);

test.describe("390 px", () => {
  test.use({ viewport: { width: 390, height: 800 } });

  for (const [name, path] of [
    ["overview", "/o/acme/w/main"],
    ["approvals", "/o/acme/w/main/approvals"],
    ["agent page", "/o/acme/w/main/agents/daneel"],
    ["sign-in", "/auth/login"],
  ] as const) {
    test(`${name} has no horizontal page scroll`, async ({ page }) => {
      await mockApi(page);
      await page.goto(path);
      await page.locator("h1").first().waitFor();
      expect(await noPageScroll(page)).toBe(true);
    });
  }

  // P4-MCON-3: the connector screens (list, add flow, detail, self-hosted) use the shared API mock
  // with the real built-in connector (Mattermost).
  for (const s of CONNECTOR_SCREENS) {
    test(`admin connectors › ${s} has no horizontal page scroll`, async ({ page }) => {
      await openConnectorScreen(page, s, "light");
      expect(await noPageScroll(page)).toBe(true);
    });
  }

  test("the agent tab strip scrolls within itself", async ({ page }) => {
    await mockApi(page);
    await page.goto("/o/acme/w/main/agents/daneel");
    const strip = page.getByRole("tablist", { name: "Agent" });
    await strip.waitFor();
    expect(await strip.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
    expect(await noPageScroll(page)).toBe(true);
  });
});
