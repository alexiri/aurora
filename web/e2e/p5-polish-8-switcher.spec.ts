import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { id, mockApi, org, user, ws } from "./support/mock-api";
import { overflow, type Scheme } from "./support/open";

// P5-POLISH-8 (D-389): one switcher at the top of the rail, personal workspaces named from the viewer's side, the
// Administration rail with "Back to <Org>", the account menu headed by the display name. The API is mocked.

const BOB = { id: id(99), name: "Ada Lovelace" };
const wsItem = (n: number, name: string, over: Record<string, unknown> = {}) => ({
  ...ws, id: id(100 + n), slug: `team-${n}`, name, my_role: "member", personal_of: null, ...over,
});
const mine = [
  wsItem(0, "Ada Lovelace", { id: id(2), slug: "main", my_role: "owner", personal_of: BOB }),
  ...Array.from({ length: 7 }, (_, i) => wsItem(i + 1, `Team ${i + 1}`)),
];
const other = { ...org, id: id(7), slug: "northwind", name: "Northwind", my_role: "member" };

/** 2 organizations and 8 + 4 workspaces. */
const many = {
  "/v1/me": {
    user,
    default_org_id: id(1),
    client_address: "203.0.113.7",
    memberships: [
      { org, role: "owner", status: "active", workspaces: mine.map((w) => ({ ...w, personal: w.personal_of !== null })) },
      {
        org: other,
        role: "member",
        status: "active",
        workspaces: Array.from({ length: 4 }, (_, i) => ({ ...ws, id: id(200 + i), slug: `n-${i}`, name: `North ${i}`, my_role: "member", personal: false })),
      },
    ],
  },
  "/v1/orgs": { items: [org, other] },
  [`/v1/orgs/${id(1)}/workspaces`]: { items: mine },
};

async function open(page: Page, path: string, scheme: Scheme, overrides: Record<string, unknown> = many) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => m.type() === "error" && !/Failed to load resource|favicon/.test(m.text()) && errors.push(m.text()));
  await mockApi(page, overrides);
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  return errors;
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`scope switcher (${scheme})`, () => {
    test.use({ viewport: { width: 1280, height: 900 } });

    test("switch workspaces, find an organization, no pill, axe-clean", async ({ page }) => {
      const errors = await open(page, "/o/acme/w/main/agents", scheme);
      const sw = page.getByRole("complementary", { name: "Sidebar" }).getByTestId("scope-switcher");
      await expect(sw).toContainText("Your workspace");
      await expect(sw).toContainText("Acme Labs");
      await expect(page.getByTestId("level-badge")).toHaveCount(0);
      await sw.getByRole("button").click();
      const search = page.getByPlaceholder("Find a workspace or organization");
      await expect(search).toBeFocused();
      // D4: the menu is sized to the room there is: it ends inside the viewport and the search box is in view.
      const box = await page.getByRole("dialog", { name: "Switch workspace or organization" }).boundingBox();
      expect(box).not.toBeNull();
      expect(box!.y + box!.height).toBeLessThanOrEqual(900);
      await expect(search).toBeInViewport();
      // D4: a personal workspace's avatar is its owner's initial, not the "Y" of "Your workspace".
      await expect(page.getByRole("link", { name: /Your workspace/ }).first()).toContainText("A");
      await expect(page.getByRole("link", { name: "All 8 in Acme Labs" })).toHaveAttribute("href", "/o/acme");
      await expect(page.getByRole("button", { name: "Northwind · 4 workspaces" })).toBeVisible();
      const { violations } = await new AxeBuilder({ page }).analyze();
      expect(violations.filter((v) => v.impact === "serious" || v.impact === "critical")).toEqual([]);
      // Switch to another workspace.
      await page.getByRole("link", { name: /Team 3/ }).click();
      await expect(page).toHaveURL(/\/o\/acme\/w\/team-3$/);
      await expect(sw).toContainText("Team 3");
      // Search for an organization, open its workspaces, go back.
      await sw.getByRole("button").click();
      await page.getByPlaceholder("Find a workspace or organization").fill("north");
      await page.getByRole("button", { name: /Northwind/ }).click();
      await expect(page.getByRole("link", { name: /North 2/ })).toHaveAttribute("href", "/o/northwind/w/n-2");
      await page.getByRole("button", { name: "Northwind", exact: true }).click();
      await expect(page.getByRole("link", { name: "All 8 in Acme Labs" })).toBeVisible();
      await page.keyboard.press("Escape");
      expect(await overflow(page)).toBeLessThanOrEqual(0);
      expect(errors).toEqual([]);
    });

    test("Administration and back", async ({ page }) => {
      const errors = await open(page, "/o/acme/w/team-3/agents", scheme);
      await page.getByRole("complementary", { name: "Sidebar" }).getByRole("link", { name: "Administration" }).click();
      await expect(page).toHaveURL(/\/admin$/, { timeout: 30_000 });
      const sidebar = page.getByRole("complementary", { name: "Sidebar" });
      await expect(sidebar.getByText("You're in")).toBeVisible();
      await expect(sidebar.getByTestId("scope-switcher")).toHaveCount(0);
      await expect(page.getByText("Select organization")).toHaveCount(0);
      await expect(page.getByTestId("level-badge")).toHaveCount(0);
      await expect(sidebar.getByRole("link", { name: "Administration" })).toHaveAttribute("aria-current", "page");
      await sidebar.getByRole("link", { name: "Back to Acme Labs" }).click();
      await expect(page).toHaveURL(/\/o\/acme\/w\/team-3$/);
      expect(errors).toEqual([]);
    });

    test("the account menu is headed by the display name", async ({ page }) => {
      await open(page, "/o/acme/w/main/agents", scheme);
      await page.getByRole("complementary", { name: "Sidebar" }).getByRole("button", { name: /Ada Lovelace/ }).click();
      const menu = page.getByRole("menu");
      await expect(menu).toContainText("ada@example.test");
      await expect(menu.getByRole("menuitem")).toHaveText(["Your account", "Administration", "Keyboard shortcuts", "Show me around", "Sign out"]);
    });

    test("Welcome says 'This is your workspace in <org>'", async ({ page }) => {
      await open(page, "/o/acme/w/main/welcome", scheme);
      await expect(page.getByText(/This is your workspace in Acme Labs/)).toBeVisible();
    });
  });
}

test.describe("scope switcher (390 px)", () => {
  test.use({ viewport: { width: 390, height: 800 } });

  test("the drawer carries the switcher and the same menu", async ({ page }) => {
    const errors = await open(page, "/o/acme/w/main/agents", "light");
    await page.getByRole("button", { name: "Open menu" }).click();
    const drawer = page.getByRole("dialog");
    await drawer.getByTestId("scope-switcher").getByRole("button").click();
    await expect(page.getByPlaceholder("Find a workspace or organization")).toBeVisible();
    const box = await page.getByPlaceholder("Find a workspace or organization").boundingBox();
    expect(box!.x + box!.width).toBeLessThanOrEqual(390);
    await page.getByRole("link", { name: /Team 2/ }).click();
    await expect(page).toHaveURL(/\/o\/acme\/w\/team-2$/);
    expect(await overflow(page)).toBeLessThanOrEqual(0);
    expect(errors).toEqual([]);
  });
});
