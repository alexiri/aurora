import { expect, test, type Page } from "@playwright/test";

import { mockApi } from "./support/mock-api";

// FIX-027: two stacked sidebar selectors — Organization, then (only with an active organization)
// Workspace. "All organizations" (instance admins, to /admin) is the one path to the admin area;
// the workspace nav has no Members item. The API is mocked (no backend needed); the dev server
// serves both themes, so the menu is checked in each.

async function open(page: Page, path: string, scheme: "light" | "dark") {
  await mockApi(page);
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
}

test.describe("sidebar selectors (FIX-027)", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("a workspace page: stacked organization above workspace, workspace reads its name", async ({ page }) => {
    await open(page, "/o/acme/w/main/agents", "light");
    const sidebar = page.getByRole("complementary", { name: "Sidebar" });
    const org = sidebar.getByTestId("org-selector");
    const ws = sidebar.getByTestId("workspace-selector");
    await expect(org.locator("p")).toHaveText("Organization");
    await expect(ws.locator("p")).toHaveText("Workspace");
    // Stacked: organization first, workspace below it, flush left — no `pl-8`
    // indent ancestor on either wrapper, and the two triggers share left edge and width (FIX-046).
    for (const sel of [org, ws]) {
      expect(await sel.getByRole("button").locator("xpath=ancestor::div[contains(@class,'pl-8')][1]").count()).toBe(0);
    }
    const orgBox = await org.getByRole("button").boundingBox();
    const wsBox = await ws.getByRole("button").boundingBox();
    expect(Math.abs(orgBox!.x - wsBox!.x)).toBeLessThanOrEqual(0.5);
    expect(Math.abs(orgBox!.width - wsBox!.width)).toBeLessThanOrEqual(0.5);
    // One line tall: the workspace name is the control's whole value (no second subtitle line).
    await expect(org.getByRole("button")).toHaveText(/Acme Labs/);
    await expect(ws.getByRole("button", { name: "Workspace Main workspace" })).toBeVisible();
    // The workspace nav has no Members item (it jumped context to the org's list).
    await expect(sidebar.getByRole("navigation", { name: "Workspace" }).getByRole("link", { name: "Members" })).toHaveCount(0);
  });

  for (const scheme of ["light", "dark"] as const) {
    test(`org root: workspace selector reads All workspaces, menu has the pinned entry (${scheme})`, async ({ page }) => {
      await open(page, "/o/acme", scheme);
      const ws = page.getByTestId("workspace-selector");
      await expect(ws.getByRole("button", { name: "Workspace All workspaces" })).toBeVisible();
      await ws.getByRole("button").click();
      const menu = page.getByRole("menu");
      await expect(menu.getByRole("menuitem", { name: "Main workspace" })).toHaveAttribute("href", "/o/acme/w/main");
      const all = menu.getByRole("menuitem", { name: "All workspaces…" });
      await expect(all).toHaveAttribute("href", "/o/acme");
      await expect(all).toHaveClass(/font-semibold/);
      // The highlighted entry is the current scope, not the link that leaves it.
      await expect(menu.getByRole("menuitem", { name: "Main workspace" })).not.toHaveClass(/font-semibold/);
    });

    test(`admin: organization reads All organizations, no workspace selector, menu entry to /admin highlighted (${scheme})`, async ({ page }) => {
      await open(page, "/admin/users", scheme);
      await expect(page.getByTestId("org-selector").getByRole("button", { name: "Organization All organizations" })).toBeVisible();
      await expect(page.getByTestId("workspace-selector")).toHaveCount(0);
      await page.getByTestId("org-selector").getByRole("button").click();
      const all = page.getByRole("menuitem", { name: "All organizations" });
      await expect(all).toHaveAttribute("href", "/admin");
      await expect(all).toHaveAttribute("data-active", "true");
      // Selecting an organization leaves the admin area for the org's workspace list.
      await page.getByRole("menuitem", { name: "Acme Labs" }).click();
      await expect(page).toHaveURL(/\/o\/acme$/);
    });

    test(`account: placeholder trigger, workspace selector absent (${scheme})`, async ({ page }) => {
      await open(page, "/account", scheme);
      await expect(page.getByTestId("org-selector").getByRole("button", { name: "Organization Select organization" })).toBeVisible();
      await expect(page.getByTestId("workspace-selector")).toHaveCount(0);
    });
  }

  test("narrow (390 px): the drawer carries the same two selectors", async ({ page }) => {
    await open(page, "/o/acme/w/main/agents", "light");
    await page.setViewportSize({ width: 390, height: 800 });
    const openButton = page.getByRole("button", { name: "Open menu" });
    // The panel opens on the trigger's side (FIX-046): the hamburger sits right of
    // centre and the open drawer is flush with the viewport's right edge. The trigger
    // is measured before the click — once the modal drawer opens, Radix aria-hides
    // the top bar and the button no longer resolves by role.
    const btnBox = await openButton.boundingBox();
    expect(btnBox!.x, "hamburger sits right of centre").toBeGreaterThan(390 / 2);
    await openButton.click();
    const drawer = page.getByRole("dialog");
    const drawerBox = await drawer.boundingBox();
    expect(drawerBox!.x + drawerBox!.width, "drawer flush with the right edge").toBeCloseTo(390, 0);
    await expect(drawer.getByTestId("org-selector").getByRole("button", { name: "Organization Acme Labs" })).toBeVisible();
    await expect(drawer.getByTestId("workspace-selector").getByRole("button", { name: "Workspace Main workspace" })).toBeVisible();
  });
});
