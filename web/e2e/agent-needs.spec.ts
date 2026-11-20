import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { agent, id, mockApi, ws } from "./support/mock-api";
import { overflow } from "./support/open";

// P5-REFINE-11 item 11 (P5-REFINE-20 G5): the agent's waiting approvals as a count on its row in the Agents list: none at
// zero, the right number, gone after deciding without a reload (the 30 s poll), fits at 390 px. The API is mocked.

const pending = (n: number) => ({
  id: id(700 + n), created_at: new Date().toISOString(), deadline: new Date(Date.now() + 3_600_000).toISOString(), status: "pending",
  method: "POST", host: "x.test", path: "/", parked: false, can_always_allow: false,
  agent: { id: agent.id, handle: agent.handle, name: agent.name },
  sentence: { agent: { id: agent.id, name: agent.name }, action: "do a thing" }, summary: { source: "generic", title: "t", fields: [] },
});

for (const [name, viewport] of [["1280 px", { width: 1280, height: 900 }], ["390 px", { width: 390, height: 844 }]] as const) {
  for (const scheme of ["light", "dark"] as const) {
    test(`the agent's row shows its waiting count and drops it after deciding, ${scheme}, ${name}`, async ({ page }) => {
      await page.setViewportSize(viewport);
      let waiting = 2;
      const errors: string[] = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await mockApi(page);
      await page.route("**/v1/**", async (route) => {
        const p = new URL(route.request().url()).pathname;
        const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
        if (p === `/v1/workspaces/${ws.id}/approvals`) return json({ items: Array.from({ length: waiting }, (_, i) => pending(i)), next_cursor: null });
        if (p === `/v1/workspaces/${ws.id}/action-approvals`) return json({ items: [], next_cursor: null });
        return route.fallback();
      });
      await page.emulateMedia({ colorScheme: scheme });
      await page.goto("/o/acme/w/main/agents");
      const chip = page.getByTestId("agent-needs-count");
      await expect(chip).toHaveText("2");
      await expect(chip).toHaveAttribute("aria-label", "2 approvals waiting for you");
      expect(await overflow(page), "horizontal scroll").toBeFalsy();
      const axe = await new AxeBuilder({ page }).analyze();
      expect(axe.violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id)).toEqual([]);
      waiting = 0;
      await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
      await expect(chip).toHaveCount(0, { timeout: 35_000 });
      expect(errors).toEqual([]);
    });
  }
}
