import { expect, test, type Page } from "@playwright/test";

import { mockApi, org } from "./support/mock-api";
import { serious } from "./support/axe";
import { openRoute, overflow } from "./support/open";

// P5-POLISH-9 (D-390, E1–E3, D6): a member reads summaries instead of the owners' forms; the organization's Apps is one
// table whose switch saves when you flip it; People has no menu on your own row. Both themes at 1280 px and 390 px.

/** The viewer is a plain member of the organization (the owner's forms must not render). */
async function asMember(page: Page) {
  await page.route(`**/v1/orgs/${org.id}`, async (route) => {
    if (route.request().method() !== "GET") return route.fallback();
    return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ ...org, my_role: "member" }) });
  });
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("a member sees the AI models summary, never a disabled form", async ({ page }) => {
        // The member override is registered after the shared mock (a later `page.route` wins), so open the page by hand.
        await mockApi(page);
        await asMember(page);
        // A member may not read the organization's accounts (403); the shared key comes from the workspace's list.
        await page.route("**/v1/orgs/*/accounts", (route) => route.fulfill({ status: 403, contentType: "application/json", body: JSON.stringify({ error: { code: "forbidden", message: "no" } }) }));
        await page.route("**/v1/workspaces/*/accounts", (route) =>
          route.fulfill({
            status: 200,
            contentType: "application/json",
            body: JSON.stringify({ items: [{ ref: { type: "secret", id: "00000000-0000-4000-8000-000000000062" }, kind: "ai_key", name: "Users AI gateway", scope: "organization", levels: [], health: { state: "not_used" }, agents: [], can: { give_access: false, change_access: false, manage: false } }], next_cursor: null }),
          }),
        );
        await page.emulateMedia({ colorScheme: scheme });
        await page.goto("/o/acme/ai-models");
        await expect(page.getByTestId("member-ai-models")).toBeVisible();
        await expect(page.getByText("Users AI gateway", { exact: true })).toBeVisible();
        await expect(page.getByText("The organization has no key of its own.")).toHaveCount(0);
        await expect(page.getByRole("button", { name: "Save" })).toHaveCount(0);
        await expect(page.getByLabel("Allowed models")).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });

      test("a member's Agents page is a sentence, not a refusal", async ({ page }) => {
        await mockApi(page);
        await asMember(page);
        await page.emulateMedia({ colorScheme: scheme });
        await page.goto("/o/acme/agents");
        await expect(page.getByTestId("member-org-agents")).toBeVisible();
        await expect(page.getByText("Only organization owners and admins can open this page")).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });

      test("Apps is one table: switch an app off and on, saved at once", async ({ page }) => {
        await openRoute(page, "/o/acme/apps?tab=apps", scheme);
        const table = page.getByRole("table", { name: /Apps and whether people may connect them/ });
        await expect(table).toBeVisible();
        await expect(page.getByRole("radio", { name: /^All \d+/ })).toBeChecked();
        const first = table.getByRole("switch").first();
        await expect(first).toBeChecked();
        await first.click();
        await expect(page.getByTestId("apps-toast")).toContainText("switched off");
        await expect(first).not.toBeChecked();
        await page.getByRole("button", { name: "Undo" }).click();
        await expect(first).toBeChecked();
        await expect(page.getByRole("button", { name: "Save" })).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });

      test("People: your own row has no menu", async ({ page }) => {
        await openRoute(page, "/o/acme/people", scheme);
        await expect(page.getByRole("table", { name: "People" })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });
    });
  }
}
