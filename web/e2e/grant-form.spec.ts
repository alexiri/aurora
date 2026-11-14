import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";

import { agent, id, mockApi } from "./support/mock-api";
import { overflow } from "./support/open";

// FIX-147 (D-233; docs/14 §Visual design › Permission groups): connect Indico (a custom-only app) → "Give agents
// access" shows the Access tab's grant form → "Allow all Read" in one click → Give access sends one grant. Both
// themes at 1280 px and 390 px, no horizontal scroll, axe with no rule disabled. Screenshots stay outside the repo.

const SHOTS = "/tmp/aurora-screens/fix-147";
mkdirSync(SHOTS, { recursive: true });

const tool = (tid: string, name: string, group: "read" | "write") => ({
  id: tid, name, group, host: "indico.example.org", path: `/${tid}`, methods: group === "read" ? ["GET"] : ["POST"], connect_methods: [], wildcard: false,
});
const tools = [
  tool("get_event", "Read an event", "read"),
  tool("list_events", "List events", "read"),
  tool("book_room", "Book a room", "write"),
];
const indico = {
  app_id: "indico", name: "Indico", package_id: "indico", package_version: "1.0.0", available: true, category: "productivity", connection_count: 0,
  description: "Events", has_dark_icon: false, app_restrictions: { custom_only: true, block_untooled: false },
  connect: [
    {
      id: "token", kind: "fields", access_levels: [], default_scopes: [], scopes: [], recommended: true, label: "API token",
      fields: [{ name: "api_token", label: "API token", type: "secret", store: "credentials", required: true }],
    },
  ],
  credentials: [], hosts: [], metadata: [], oauth_clients: [], tools,
};
const CONN = id(90);

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("connect Indico → Give agents access → Allow all Read in one click → one custom grant", async ({ page }) => {
        const posted: unknown[] = [];
        await mockApi(page);
        await page.emulateMedia({ colorScheme: scheme });
        await page.route("**/v1/**", async (route) => {
          const p = new URL(route.request().url()).pathname;
          const m = route.request().method();
          const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
          if (p === "/v1/apps/indico" && m === "GET") return json(indico);
          if (p === "/v1/apps/indico/connect") return json({ kind: "connected", connection: { id: CONN }, missing_scopes: [] });
          if (p === `/v1/workspaces/${agent.workspace_id}/agents`) return json({ items: [agent], next_cursor: null });
          if (p === `/v1/connections/${CONN}/grants`) {
            if (m === "POST") {
              posted.push(route.request().postDataJSON());
              return json({ items: [{ agent_id: agent.id, grant: {}, status: "created" }] });
            }
            return json({ items: [] });
          }
          return route.fallback();
        });
        await page.goto("/o/acme/w/main/apps/indico/connect");
        await page.getByLabel("API token").fill("not-a-real-token");
        await page.getByRole("button", { name: "Connect", exact: true }).click();
        const panel = page.getByLabel("Give agents access");
        await panel.getByRole("checkbox", { name: `Give ${agent.name} access` }).check();
        await expect(panel.getByText(/This app only supports custom access/)).toBeVisible();
        await expect(panel.getByRole("button", { name: "Give access" })).toBeDisabled();
        await panel.getByRole("radiogroup", { name: "Set all read tools" }).getByText("Allow", { exact: true }).click();
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
        await page.screenshot({ path: `${SHOTS}/give-access-${scheme}-${width}.png` });
        expect(await serious(page)).toEqual([]);
        await panel.getByRole("button", { name: "Give access" }).click();
        await expect(panel.getByText("Access given to 1 agent.")).toBeVisible();
        expect(posted).toEqual([{ agent_ids: [agent.id], mode: "custom", allow_tools: ["get_event", "list_events"], ask_tools: [] }]);
      });
    });
  }
}
