import { expect, test } from "@playwright/test";
import { mkdirSync } from "node:fs";

import { agent, id, mockApi } from "./support/mock-api";
import { overflow } from "./support/open";
import { serious } from "./support/axe";

// FIX-136 (D-241; docs/14 §Connect window): connect Indico with a personal token → "Which scopes did you give this
// token?" (detect read scopes, or tick them) → "Give agents access" lists the tools the token cannot serve collapsed with
// the scope they need, unselectable. Both themes at 1280 px and 390 px, no horizontal scroll, axe with no rule disabled.
// Screenshots stay outside the repo.

const SHOTS = "/tmp/aurora-screens/fix-136";
mkdirSync(SHOTS, { recursive: true });

const tool = (tid: string, name: string, group: "read" | "write", scopes: string[]) => ({
  id: tid, name, group, host: "indico.example.org", path: `/${tid}`, methods: group === "read" ? ["GET"] : ["POST"], connect_methods: [], scopes, wildcard: false,
});
const tools = [
  tool("get_event", "Read an event", "read", ["read:legacy_api"]),
  tool("search_rooms", "Search rooms free in a time slot (Indico web API)", "read", ["read:everything", "full:everything"]),
  tool("book_room", "Book a room: one slot (official API)", "write", ["write:legacy_api"]),
  tool("create_booking", "Book a room: recurring, with all options (Indico web API)", "write", ["full:everything"]),
];
const indico = {
  app_id: "indico", name: "Indico", package_id: "indico", package_version: "1.2.0", available: true, category: "productivity", connection_count: 0,
  description: "Events", has_dark_icon: false, app_restrictions: { custom_only: true, block_untooled: true },
  connect: [
    {
      id: "token", kind: "fields", access_levels: [], default_scopes: [], recommended: true, label: "Personal API token",
      audience: "person", show_context: [], scope_presets: { read: ["read:legacy_api"], write: ["write:legacy_api"] },
      fields: [{ name: "access_token", label: "Personal API token", type: "secret", store: "credentials", required: true }],
      probe_scopes: ["read:legacy_api", "read:everything"],
      scopes: [
        { scope: "read:legacy_api", name: "Read events, timetables, rooms and bookings (official API)", access: "read" },
        { scope: "write:legacy_api", name: "Book rooms through the official API", access: "write" },
        { scope: "read:everything", name: "Read everything you can see (Indico web API)", access: "read" },
        { scope: "full:everything", name: "Change everything you can change (Indico web API)", access: "write" },
      ],
    },
  ],
  credentials: [], hosts: [], metadata: [], oauth_clients: [], tools,
};
const CONN = id(92);

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("token scopes: tick them → unavailable tools are listed collapsed, not selectable", async ({ page }) => {
        const connected: Record<string, unknown>[] = [];
        await mockApi(page);
        await page.emulateMedia({ colorScheme: scheme });
        await page.route("**/v1/**", async (route) => {
          const p = new URL(route.request().url()).pathname;
          const m = route.request().method();
          const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
          if (p === "/v1/apps/indico" && m === "GET") return json(indico);
          if (p === "/v1/apps/indico/connect") {
            const body = route.request().postDataJSON() as Record<string, unknown>;
            connected.push(body);
            return json({ kind: "connected", connection: { id: CONN, scopes: body.scopes ?? ["read:legacy_api"] }, grants: [], missing_scopes: [] });
          }
          if (p === `/v1/workspaces/${agent.workspace_id}/agents`) return json({ items: [agent], next_cursor: null });
          if (p === `/v1/connections/${CONN}/grants`) return json({ items: [] });
          return route.fallback();
        });
        await page.goto("/o/acme/w/main/credentials?tab=apps&connect=indico");
        // The connect journey (P5-UX-3): no agent now (the unavailable tools are listed in the Access dialog from Done), and the
        // token's scopes are under Advanced.
        const journey = page.getByTestId("ws-connect-journey");
        await journey.getByRole("checkbox", { name: `Give ${agent.name} access` }).uncheck();
        await journey.getByRole("button", { name: "Next" }).click();
        await journey.getByLabel("Personal API token", { exact: true }).fill("not-a-real-token");
        // (P5-POLISH-5) The token's scopes are under "More options" (was "Advanced: other ways to connect").
        await journey.getByText("More options", { exact: true }).click();
        const scopes = page.getByRole("group", { name: "Which scopes did you give this token?" });
        await expect(scopes.getByText("Aurora checks each read scope")).toBeVisible();
        await scopes.getByText("I will tick them", { exact: true }).click();
        await scopes.getByRole("radiogroup", { name: "Read events, timetables, rooms and bookings (official API)" }).getByText("On", { exact: true }).click();
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
        await page.screenshot({ path: `${SHOTS}/connect-scopes-${scheme}-${width}.png` });
        await page.getByRole("button", { name: "Connect", exact: true }).click();
        await expect.poll(() => connected.length).toBe(1);
        expect(connected[0]).toMatchObject({ method_id: "token", scopes: ["read:legacy_api"] });

        await journey.getByRole("button", { name: "Give access" }).click();
        const panel = page.getByLabel("Give agents access");
        await panel.getByRole("checkbox", { name: `Give ${agent.name} access` }).check();
        const box = panel.getByText(/Not available with this connection's token \(3\)/);
        await expect(box).toBeVisible();
        await box.click();
        await expect(panel.getByText("Needs full:everything")).toBeVisible();
        await expect(panel.getByRole("radiogroup", { name: "Access for Read an event" })).toBeVisible();
        await expect(panel.getByRole("radiogroup", { name: /Access for Search rooms/ })).toHaveCount(0);
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
        await page.screenshot({ path: `${SHOTS}/unavailable-${scheme}-${width}.png` });
        expect(await serious(page)).toEqual([]);
      });
    });
  }
}
