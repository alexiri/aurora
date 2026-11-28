import { expect, test, type Page } from "@playwright/test";

import { id, mockApi, org, user, ws } from "./support/mock-api";
import { overflow } from "./support/open";
import { serious } from "./support/axe";

// P5-UX-1 (C-915, C-916, C-981; docs/14 §Who can fix this): a refused page names who can open it, an empty Accounts
// list asks someone else only when the viewer cannot act. Both themes at 1280 px and 390 px, no horizontal scroll,
// axe with nothing disabled.

const admins = [{ name: "Ada Lovelace" }, { name: "Alan Turing" }];
const owners = [{ name: "Bob Builder" }, { name: "Ada Diaz" }];

async function open(page: Page, path: string, scheme: "light" | "dark", role: "owner" | "member") {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  await page.emulateMedia({ colorScheme: scheme });
  await page.route("**/v1/**", async (route) => {
    const p = new URL(route.request().url()).pathname;
    const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
    if (p === `/v1/orgs/${id(1)}`) return json({ ...org, my_role: role === "owner" ? "owner" : "member", contacts: { owners: [{ name: "Grace Hopper" }], admins } });
    if (p === `/v1/workspaces/${id(2)}`) return json({ ...ws, my_role: role, contacts: { owners } });
    if (p === `/v1/workspaces/${id(2)}/secrets`) return json({ items: [], next_cursor: null });
    if (p === `/v1/workspaces/${id(2)}/accounts`) return json({ items: [], next_cursor: null });
    return route.fallback();
  });
  await page.goto(path);
  await page.locator("h1, h2, [role='alert']").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, `${path} threw`).toEqual([]);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      // (D-508(e), C-1178) A member doesn't see Manage: the address no longer ends in a refusal.
      test("a member who opens People by its address lands in their workspace, not on a refusal or a network error", async ({ page }) => {
        await mockApi(page);
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
        await page.route(`**/v1/orgs/${id(1)}`, (route) =>
          route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ ...org, my_role: "member", contacts: { owners: [{ name: "Grace Hopper" }], admins } }) }),
        );
        await page.emulateMedia({ colorScheme: scheme });
        await page.goto("/o/acme/people");
        await expect(page).toHaveURL(/\/o\/acme\/w\/main$/);
        await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
        await expect(page.getByText("Only organization owners and admins can open this page.")).toHaveCount(0);
        await expect(page.getByText("Could not reach the server")).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });

      test("a workspace owner's empty Accounts list shows its Add action and no Ask line", async ({ page }) => {
        await open(page, "/o/acme/w/main/apps?tab=accounts&kind=website", scheme, "owner");
        await expect(page.getByRole("button", { name: "Add", exact: true })).toBeVisible();
        // (P5-UX-3) The empty state's button opens the connect journey; it was a link to the Apps tab.
        await expect(page.getByRole("button", { name: "Connect an app or website" })).toBeVisible();
        await expect(page.getByText("Nothing connected yet")).toBeVisible();
        await expect(page.getByText(/Ask /)).toHaveCount(0);
        await expect(page.getByText(/Only workspace owners/)).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });

      test("a workspace member's Accounts says who can add one, and has no button", async ({ page }) => {
        await open(page, "/o/acme/w/main/apps?tab=accounts", scheme, "member");
        await expect(page.getByText("Only workspace owners can add an account. Ask a workspace owner.")).toBeVisible();
        await expect(page.getByRole("button", { name: /^Add$/ })).toHaveCount(0);
        await expect(page.getByRole("button", { name: "Connect an app or website" })).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });
    });
  }
}
