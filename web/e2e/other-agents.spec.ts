import { expect, test, type Page } from "@playwright/test";

import { openThemed, overflow, type Scheme } from "./support/open";
import { mockWithTerminal } from "./support/terminal";

// (P5-REFINE-12, channels-1, C-1085) Settings › Other agents: the empty state, Link another agent with the three direction
// cards and the disabled candidates, then the list with its row menu. The API is mocked.

const LINK = {
  id: "00000000-0000-4000-8000-0000000000d1", revision: 1, created_at: "2026-11-20T10:00:00Z", updated_at: "2026-11-20T10:00:00Z",
  self: { agent_id: "00000000-0000-4000-8000-0000000000c1", policy: "ask" },
  peer: { agent_id: "00000000-0000-4000-8000-0000000000c2", handle: "rex", name: "Rex", workspace_id: "00000000-0000-4000-8000-0000000000f2", workspace_name: "Ops", policy: "ask" },
  direction: "both", created_by: { kind: "person", display_name: "Ada" },
};
const LINKABLE = [
  { agent_id: LINK.peer.agent_id, name: "Rex", handle: "rex", workspace_id: LINK.peer.workspace_id, workspace_name: "Ops", linked: false, hosted: true, frozen: false },
  { agent_id: "00000000-0000-4000-8000-0000000000c3", name: "Sleepy", handle: "sleepy", workspace_id: LINK.peer.workspace_id, workspace_name: "Ops", linked: false, hosted: true, frozen: true },
];

async function mockPeers(page: Page, state: { links: unknown[] }) {
  await mockWithTerminal(page, { output: new Uint8Array(), resizes: [] });
  const reply = (body: unknown, status = 200) => ({ status, contentType: "application/json", body: JSON.stringify(body) });
  await page.route("**/v1/agents/*/links", async (route) => {
    if (route.request().method() === "POST") {
      state.links = [LINK];
      return route.fulfill(reply(LINK, 201));
    }
    return route.fulfill(reply({ items: state.links }));
  });
  await page.route("**/v1/agents/*/linkable", (route) => route.fulfill(reply({ items: LINKABLE })));
  await page.route("**/v1/agents/*/peer-tasks**", (route) => route.fulfill(reply({ items: [], next_cursor: null })));
  await page.route("**/v1/agent-links/*", (route) => {
    state.links = [];
    return route.fulfill({ status: 204 });
  });
}

for (const scheme of ["light", "dark"] as const satisfies readonly Scheme[]) {
  for (const width of [1280, 390]) {
    test.describe(`Other agents (${width} px, ${scheme})`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("link another agent, then remove the link", async ({ page }) => {
        const errors: string[] = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await mockPeers(page, { links: [] });
        await openThemed(page, "/o/acme/w/main/agents/daneel?tab=peers", scheme);
        await expect(page.getByText("Daneel isn't linked to another agent")).toBeVisible();
        await page.getByRole("button", { name: "Link another agent" }).click();
        const dialog = page.getByRole("dialog", { name: "Link another agent" });
        await expect(dialog.getByRole("option", { name: "Sleepy · Ops (Paused)" })).toBeDisabled();
        await dialog.getByLabel("Agent").selectOption({ label: "Rex · Ops" });
        await expect(dialog.getByText("Daneel only works on what Rex sends.")).toBeVisible();
        await dialog.getByRole("radio", { name: /Both ways/ }).check();
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
        await dialog.getByRole("button", { name: "Link agents" }).click();
        await expect(page.getByText("Daneel and Rex can each give the other tasks.")).toBeVisible();
        await expect(page.getByText(/Linked by Ada/)).toBeVisible();
        await page.getByRole("button", { name: "Actions for the link with Rex" }).click();
        await page.getByRole("menuitem", { name: "Remove link…" }).click();
        await page.getByRole("dialog").getByRole("button", { name: "Remove link" }).click();
        await expect(page.getByText("Daneel isn't linked to another agent")).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(errors).toEqual([]);
      });
    });
  }
}
