import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { ROUTES } from "./support/mock-api";
import { openRoute } from "./support/open";

// P2-UI stage 4 (docs/18 §Dashboard visual design): zero serious/critical axe violations,
// including color-contrast, in BOTH themes. Every route at 1280 px, and the five pages named in
// docs/18 (Overview, Approvals, the agent Chat tab, an org page, sign-in) also at 390 px.
// No rule is disabled and nothing is excluded.

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
}

const NAMED = ["ws-overview", "ws-approvals", "ws-agent", "org-workspaces", "sign-in"];

for (const scheme of ["light", "dark"] as const) {
  test.describe(`${scheme} theme, 1280 px`, () => {
    test.use({ viewport: { width: 1280, height: 800 } });
    for (const r of ROUTES) {
      test(`${r.name} has no serious or critical violations`, async ({ page }) => {
        await openRoute(page, r.path, scheme);
        expect(await serious(page)).toEqual([]);
      });
    }
  });

  test.describe(`${scheme} theme, 390 px`, () => {
    test.use({ viewport: { width: 390, height: 800 } });
    for (const r of ROUTES.filter((x) => NAMED.includes(x.name))) {
      test(`${r.name} has no serious or critical violations`, async ({ page }) => {
        await openRoute(page, r.path, scheme);
        expect(await serious(page)).toEqual([]);
      });
    }
  });

  // The tabs that show the real built-in package icons (AppIcon), with the icons loaded.
  test.describe(`${scheme} theme, credentials tabs`, () => {
    for (const path of ["/o/acme/w/main/credentials", "/o/acme/credentials"]) {
      for (const tab of ["Secrets", "LLM keys", "Proposals", "Apps", "Connections", "OAuth clients"]) {
        test(`${path} › ${tab}`, async ({ page }) => {
          await page.setViewportSize({ width: 1280, height: 900 });
          await openRoute(page, path, scheme);
          const t = page.getByRole("tab", { name: tab, exact: true });
          if (await t.count()) await t.click();
          await page.waitForLoadState("networkidle");
          if (tab === "Apps" && path.includes("/w/main")) await expect(page.locator(`img[src="/v1/apps/github/icon${scheme === "dark" ? "?variant=dark" : ""}"]`).first()).toBeVisible();
          if (tab === "Apps" && path.includes("/w/main")) await page.screenshot({ path: `/tmp/aurora-screens/p2-ui-4/apps-tab-${scheme}-1280.png` });
          expect(await serious(page)).toEqual([]);
        });
      }
    }
  });

  test.describe(`${scheme} theme, agent Chat tab`, () => {
    for (const width of [1280, 390]) {
      test(`at ${width} px`, async ({ page }) => {
        await page.setViewportSize({ width, height: 800 });
        await openRoute(page, "/o/acme/w/main/agents/daneel", scheme);
        const chat = page.getByRole("tab", { name: "Chat" });
        if (await chat.count()) await chat.first().click();
        await expect(page.getByRole("tab", { name: "Chat" }).first()).toHaveAttribute("aria-selected", "true");
        expect(await serious(page)).toEqual([]);
      });
    }
  });
}
