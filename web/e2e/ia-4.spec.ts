import AxeBuilder from "@axe-core/playwright";
import { expect as baseExpect, test, type Page } from "@playwright/test";

import { ws } from "./support/mock-api";
import { openRoute, overflow } from "./support/open";

// The first request of a route compiles it in `next dev`; on a loaded host that outlasts the default 5 s.
const expect = baseExpect.configure({ timeout: 30_000 });

// P5-IA-4 (C-967..C-970): People, Data, Platform, Chat platforms and Settings as coherent pages. Old addresses landing
// on the right tabs are checked by ia-navigation.spec.ts.

async function axeClean(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("an organization changes the file retention on Data › Retention, under Administration's maximums", async ({ page }) => {
        await openRoute(page, "/o/acme/data", scheme);
        const above = page.getByRole("region", { name: "Set for you by an Aurora administrator" });
        await expect(above).toContainText("At most 365 days without activity");
        await expect(above).toContainText("At most 90 days");
        await expect(above).toContainText("Set for you by an Aurora administrator");
        const files = page.getByLabel(/Delete files after/);
        await files.fill("120");
        await page.getByRole("button", { name: "Save", exact: true }).click();
        await expect(page.getByText("Administration allows at most 90 days.")).toBeVisible();
        await expect(page.getByRole("alert").filter({ hasText: "1 field needs fixing" })).toBeVisible();
        await files.fill("60");
        const put = page.waitForRequest((r) => r.method() === "PUT" && /\/retention$/.test(r.url()));
        await page.getByRole("button", { name: "Save", exact: true }).click();
        expect((await put).postDataJSON()).toEqual({ conversations_days: 180, attachments_days: 60, attachment_quota_mib: null });
        await expect(page.getByText("Saved. Retention applies from now on.")).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
        // Storage shows usage only: the limits are the form on the first tab.
        await page.getByRole("tab", { name: "Storage" }).click();
        await expect(page.getByRole("heading", { name: "Usage" })).toBeVisible();
        await expect(page.getByLabel(/Storage quota/)).toHaveCount(0);
      });

      test("Administration › People › Sign-in is one page: providers, roles from the provider, directory sync", async ({ page }) => {
        if (width >= 768) {
          // Administration's sidebar leads to People; at phone width the sidebar is a drawer, so that part is wide only.
          await openRoute(page, "/admin", scheme);
          await page.getByRole("complementary", { name: "Sidebar" }).getByRole("link", { name: "People & sign-in", exact: true }).click();
        } else {
          await openRoute(page, "/admin/people", scheme);
        }
        await page.getByRole("tab", { name: "Sign-in" }).click();
        await expect(page).toHaveURL(/\/admin\/people\?tab=sign-in$/);
        await expect(page.getByRole("heading", { name: "How people sign in" })).toBeVisible();
        await expect(page.getByRole("region", { name: "Roles from the provider" })).toBeVisible();
        await expect(page.getByRole("heading", { name: "Directory sync", exact: true })).toBeVisible();
        await expect(page.getByRole("tab")).toHaveText(["Users", "Organizations", "Sign-in", "New users"]);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("a workspace owner opens People from the sidebar", async ({ page }) => {
        if (width >= 768) {
          await openRoute(page, "/o/acme/w/main/agents", scheme);
          await page.getByRole("complementary", { name: "Sidebar" }).getByRole("navigation", { name: "Workspace" }).getByRole("link", { name: "People" }).click();
        } else {
          await openRoute(page, "/o/acme/w/main/people", scheme);
        }
        await expect(page).toHaveURL(/\/o\/acme\/w\/main\/people$/);
        await expect(page.getByRole("heading", { name: "People", level: 1 })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("the Agents page has no BYO block; the owner chooses it in Settings › General (D-442) and it reads back", async ({ page }) => {
        await openRoute(page, "/o/acme/w/main/agents", scheme);
        await expect(page.getByRole("heading", { name: "Agents", level: 1 })).toBeVisible();
        await expect(page.getByText(/Set for you by/)).toHaveCount(0);
        await expect(page.getByText("Bring-your-own agents")).toHaveCount(0);
        await expect(page.getByRole("button", { name: "Restrict further…" })).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
        // The workspace remembers its PATCH (the shared mock's workspace is read-only).
        let own: string | null = null;
        const patches: Record<string, unknown>[] = [];
        await page.route(/\/v1\/workspaces\/[^/?]+$/, async (r) => {
          if (r.request().method() === "PATCH") {
            const body = r.request().postDataJSON() as Record<string, unknown>;
            patches.push(body);
            if ("byo_posture" in body) own = body.byo_posture as string | null;
          }
          await r.fulfill({ json: { ...ws, byo_posture: own, effective_byo_posture: own ? { value: own, level: "workspace" } : { value: "enabled", level: "org" } } });
        });
        await page.goto("/o/acme/w/main/settings");
        await expect(page.getByLabel("Time zone")).toBeVisible();
        await expect(page.getByLabel(/^Language .* remembers? in$/)).toBeVisible();
        const field = page.getByLabel("Bring-your-own agents");
        await expect(field).toBeVisible();
        await expect(page.getByText("Your organization allows: members can create BYO agents. A workspace can only be stricter.")).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
        await field.selectOption("disabled");
        // At 390 px `next dev`'s own indicator can sit over Save and take the pointer, so the click is dispatched.
        await page.getByRole("button", { name: "Save" }).dispatchEvent("click");
        await expect.poll(() => patches).toEqual([expect.objectContaining({ byo_posture: "disabled" })]);
        await page.reload();
        await expect(page.getByLabel("Bring-your-own agents")).toHaveValue("disabled");
      });
    });
  }
}
