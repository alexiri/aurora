import { expect, test, type Page } from "@playwright/test";

import { serious } from "./support/axe";
import { id, mockApi, now, org, user, ws } from "./support/mock-api";
import { overflow, titled, type Scheme } from "./support/open";

// P5-NAV-2 (D-508, C-1178; docs/design/navigation nav-3b, nav-4): the rail shows one place, with its place card and back
// line; Manage <org> replaces the rail; Workspaces has its own page. Both themes at 1256 and 390 px, axe, no sideways
// scroll. Set AURORA_SHOTS to a directory to keep full-page screenshots of each screen.

const SHOTS = process.env.AURORA_SHOTS;

const needs = (n: number, over: Record<string, unknown>) => ({
  id: `item-${n}`, level: "organization", state: "open", own: false, since: now, subject_id: `s${n}`, actors: [], actors_total: 0,
  can_decline: false, decision: true, ...over,
});

const FEED = {
  count: 2,
  items: [
    needs(1, {
      kind: "template_update", topic: "skills", title: "Anthropic skills has an update to review",
      detail: "Shared with every workspace in Acme Labs", href: "/o/acme/skills?tab=libraries", action: { label: "Review", href: "/o/acme/skills?tab=libraries" },
    }),
    needs(2, {
      kind: "broken_account", topic: "accounts", title: "The GitHub account for Acme Labs failed to refresh its token",
      detail: "Workspaces that use it stop until it is renewed", href: "/o/acme/apps?tab=accounts", action: { label: "Fix", href: "/o/acme/apps?tab=accounts" },
    }),
  ],
};

async function open(page: Page, path: string, scheme: Scheme) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => m.type() === "error" && !/^Failed to load resource/.test(m.text()) && errors.push(m.text()));
  await mockApi(page);
  await page.route(/\/v1\/orgs\/[^/]+\/attention(\?.*)?$/, (r) =>
    r.fulfill({ json: new URL(r.request().url()).searchParams.get("state") === "done" ? { count: 0, items: [] } : FEED }),
  );
  await page.route("**/v1/attention/counts*", (r) =>
    r.fulfill({ json: { administration: { count: 0, topics: {} }, organization: { org_id: id(1), count: 2, topics: { skills: 1, apps: 1 } }, workspace: null } }),
  );
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await titled(page);
  return errors;
}

async function shot(page: Page, name: string) {
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/${name}.png` });
}

async function clean(page: Page, errors: string[]) {
  expect(await overflow(page)).toBeLessThanOrEqual(0);
  expect(await serious(page)).toEqual([]);
  expect(errors).toEqual([]);
}

/** The rail: the sidebar on a desktop, the drawer on a phone. */
async function rail(page: Page, width: number) {
  if (width >= 768) return page.getByRole("complementary", { name: "Sidebar" });
  await page.getByRole("button", { name: /^Open menu/ }).click();
  return page.getByRole("dialog", { name: "Navigation" });
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1256, 390]) {
    test.describe(`places (${scheme}, ${width} px)`, () => {
      test.use({ viewport: { width, height: width === 390 ? 844 : 783 } });

      test("Manage › Overview: one menu in the rail, summary cards and Needs you", async ({ page }) => {
        const errors = await open(page, "/o/acme", scheme);
        await expect(page.getByRole("heading", { level: 1, name: "Acme Labs" })).toBeVisible();
        await expect(page.getByText("Everything Acme Labs's workspaces share")).toBeVisible();
        await expect(page.getByRole("list", { name: "Summary" }).getByRole("link")).toHaveCount(4);
        await expect(page.getByRole("link", { name: "The GitHub account for Acme Labs failed to refresh its token", exact: true })).toBeVisible();
        // The page itself draws no second menu and no select.
        await expect(page.getByRole("main").getByRole("navigation", { name: /^Manage / })).toHaveCount(0);
        await expect(page.getByRole("main").getByRole("combobox")).toHaveCount(0);
        await clean(page, errors);
        await shot(page, `overview-${scheme}-${width}`);
        const r = await rail(page, width);
        await expect(r.getByRole("navigation", { name: "Manage Acme Labs" }).getByRole("link")).toHaveCount(11);
        await expect(r.getByRole("navigation", { name: "Workspace" })).toHaveCount(0);
        await expect(r.getByRole("link", { name: /^Back to / })).toBeVisible();
        await expect(r.getByTestId("scope-switcher")).toContainText("Organization");
        expect(await serious(page)).toEqual([]);
        await shot(page, `overview-rail-${scheme}-${width}`);
      });

      test("Manage › Workspaces lists every workspace", async ({ page }) => {
        const errors = await open(page, "/o/acme/workspaces", scheme);
        await expect(page.getByRole("heading", { level: 1, name: "Workspaces in Acme Labs" })).toBeVisible();
        await expect(page.getByRole("table", { name: "Workspaces" })).toBeVisible();
        await expect(page.getByRole("button", { name: "New workspace" })).toBeVisible();
        await clean(page, errors);
        await shot(page, `workspaces-${scheme}-${width}`);
      });

      test("a workspace's manage page keeps Manage's menu, under Workspaces", async ({ page }) => {
        await open(page, `/o/acme/workspaces/${ws.slug}`, scheme);
        const r = await rail(page, width);
        await expect(r.getByRole("navigation", { name: "Manage Acme Labs" }).getByRole("link", { name: "Workspaces" })).toHaveAttribute("aria-current", "page");
        await expect(r.getByRole("navigation", { name: "Workspace" })).toHaveCount(0);
      });

      test("the back line returns to the workspace page you came from", async ({ page }) => {
        const errors = await open(page, `/o/acme/w/${ws.slug}/activity?tab=usage`, scheme);
        let r = await rail(page, width);
        await expect(r.getByTestId("place-back")).toHaveCount(0);
        await shot(page, `workspace-rail-${scheme}-${width}`);
        await r.getByRole("link", { name: /^Administration/ }).click();
        await expect(page).toHaveURL(/\/admin$/, { timeout: 30_000 });
        await page.waitForLoadState("networkidle");
        r = await rail(page, width);
        await expect(r.getByTestId("place-card")).toHaveText(/Administration\s+This Aurora/);
        await shot(page, `admin-rail-${scheme}-${width}`);
        await r.getByRole("link", { name: /^Back to / }).click();
        await expect(page).toHaveURL(new RegExp(`/o/acme/w/${ws.slug}/activity\\?tab=usage$`));
        expect(errors).toEqual([]);
      });

      test("Your account has the person's place card and a way back", async ({ page }) => {
        const errors = await open(page, "/account", scheme);
        await clean(page, errors);
        const r = await rail(page, width);
        await expect(r.getByTestId("place-card")).toHaveText(new RegExp(`${user.display_name}\\s+Your account`));
        await expect(r.getByRole("link", { name: /^Back to / })).toBeVisible();
        expect(await serious(page)).toEqual([]);
        await shot(page, `account-rail-${scheme}-${width}`);
      });
    });
  }
}

test.describe("members (D-508(e))", () => {
  test("a member of the organization is sent from Manage to their workspace", async ({ page }) => {
    await mockApi(page, {
      "/v1/me": {
        user: { ...user, is_instance_admin: false },
        memberships: [{ org, role: "member", status: "active", workspaces: [{ ...ws, my_role: "member", personal: false }] }],
        default_org_id: org.id,
        client_address: "203.0.113.7",
      },
      [`/v1/orgs/${id(1)}`]: { ...org, my_role: "member" },
    });
    await page.goto("/o/acme/people");
    await expect(page).toHaveURL(new RegExp(`/o/acme/w/${ws.slug}$`), { timeout: 30_000 });
    await expect(page.getByRole("complementary", { name: "Sidebar" }).getByRole("link", { name: /Manage/ })).toHaveCount(0);
  });
});
