import { expect, test, type Page } from "@playwright/test";

import { mockApi, org, user, ws } from "./support/mock-api";
import { serious } from "./support/axe";
import { openRoute, overflow } from "./support/open";

// P5-POLISH-9 (E3, D6): the organization's Apps is one table whose switch saves when you flip it; People has no menu on
// your own row. D-508(e): a member doesn't see Manage. Both themes at 1280 px and 390 px.

/** The viewer is a plain member of the organization, not an instance admin (Manage must not render). */
async function asMember(page: Page) {
  await page.route("**/v1/me", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        user: { ...user, is_instance_admin: false },
        memberships: [{ org, role: "member", status: "active", workspaces: [{ ...ws, my_role: "member", personal: false }] }],
        default_org_id: org.id,
        client_address: "203.0.113.7",
      }),
    }),
  );
  await page.route(`**/v1/orgs/${org.id}`, async (route) => {
    if (route.request().method() !== "GET") return route.fallback();
    return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ ...org, my_role: "member" }) });
  });
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      for (const path of ["/o/acme/ai-models", "/o/acme/agents"]) {
        test(`a member who opens ${path} lands in their workspace, not on Manage or a refusal`, async ({ page }) => {
          // The member override is registered after the shared mock (a later `page.route` wins), so open the page by hand.
          await mockApi(page);
          await asMember(page);
          await page.emulateMedia({ colorScheme: scheme });
          await page.goto(path);
          await expect(page).toHaveURL(/\/o\/acme\/w\/main$/);
          await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
          await expect(page.getByRole("navigation", { name: /^Manage / })).toHaveCount(0);
          await expect(page.getByText("Only organization owners and admins can open this page")).toHaveCount(0);
          expect(await overflow(page)).toBeLessThanOrEqual(0);
          expect(await serious(page)).toEqual([]);
        });
      }

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
