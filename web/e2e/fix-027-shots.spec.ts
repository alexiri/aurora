import { expect, test, type Page } from "@playwright/test";

import { mockApi } from "./support/mock-api";
import { CANVAS, overflow, type Scheme } from "./support/open";

// FIX-027: owner-review screenshots of the two stacked sidebar selectors — org page, workspace
// page, admin and account, in both themes — under /tmp/aurora-screens/fix-027/. Same mocked-API
// open as the stage-4 specs: themed, no horizontal scroll, page rendered (no page errors).

const SHOTS = "/tmp/aurora-screens/fix-027";

const SCREENS: { name: string; path: string }[] = [
  { name: "org", path: "/o/acme" },
  { name: "workspace", path: "/o/acme/w/main/agents" },
  { name: "admin", path: "/admin/users" },
  { name: "account", path: "/account" },
];

for (const scheme of ["light", "dark"] as const) {
  test.describe(`FIX-027 screenshots (${scheme})`, () => {
    test.use({ viewport: { width: 1280, height: 900 } });

    for (const s of SCREENS) {
      test(`${s.name}: sidebar screenshot, themed, no horizontal scroll`, async ({ page }) => {
        await openShell(page, s.path, scheme);
        const sidebar = page.getByRole("complementary", { name: "Sidebar" });
        await expect(sidebar.getByTestId("org-selector").getByRole("button")).toBeVisible();
        // Let lists settle before the picture.
        await page.waitForTimeout(300);
        await page.screenshot({ path: `${SHOTS}/${s.name}-${scheme}-1280.png` });
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
      });
    }
  });
}

async function openShell(page: Page, path: string, scheme: Scheme) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, `${path} threw`).toEqual([]);
}
