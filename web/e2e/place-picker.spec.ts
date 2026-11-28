import { expect, test, type Page } from "@playwright/test";

import { serious } from "./support/axe";
import { id, mockApi, org, user, ws } from "./support/mock-api";
import { overflow, titled, type Scheme } from "./support/open";

// P5-NAV-3 (D-508 (f), (g), (k); docs/design/navigation nav-3a, nav-3, nav-3d): the place picker for three people, Manage
// at the rail's foot, and on a phone the place card in the top bar with the picker as a sheet. Both themes at 1256 and
// 390 px, axe, no sideways scroll. Set AURORA_SHOTS to a directory to keep screenshots.

const SHOTS = process.env.AURORA_SHOTS;
const NAME = "Switch workspace or organization";

const globex = { ...org, id: id(7), slug: "globex", name: "Globex", my_role: "member" };
const initech = { ...org, id: id(8), slug: "initech", name: "Initech", my_role: "member" };

const own = (n: number, slug: string, name: string, my_role: string, personal = false) => ({
  ...ws, id: slug === ws.slug ? ws.id : id(300 + n), slug, name, my_role, personal,
});
const membership = (o: typeof org, role: string, workspaces: unknown[]) => ({ org: o, role, status: "active", workspaces });
const me = (u: Partial<typeof user>, memberships: unknown[]) => ({ user: { ...user, ...u }, default_org_id: org.id, client_address: "203.0.113.7", memberships });

const bobsOwn = own(0, ws.slug, "Bob Builder", "owner", true);
const alices = [own(0, ws.slug, "Alice Admin", "owner", true), own(1, "default", "Default", "owner"), own(2, "platform", "Platform team", "member")];

const carols = [own(0, ws.slug, "Platform team", "member"), ...Array.from({ length: 11 }, (_, i) => own(10 + i, `team-${i}`, `Team ${i}`, "member"))];

const PEOPLE = {
  bob: {
    "/v1/me": me({ display_name: "Bob Builder", is_instance_admin: false }, [membership(org, "member", [bobsOwn])]),
    "/v1/instance": { org_creation: "anyone", email_delivery: "smtp", features: {} },
    [`/v1/orgs/${id(1)}`]: { ...org, my_role: "member" },
    [`/v1/orgs/${id(1)}/workspaces`]: { items: [{ ...bobsOwn, personal_of: { id: user.id, name: "Bob Builder" } }] },
  },
  alice: {
    "/v1/me": me({ display_name: "Alice Admin" }, [
      membership(org, "owner", alices),
      membership(globex, "member", [own(9, "main-globex", "Main", "member")]),
    ]),
    [`/v1/orgs/${id(1)}/workspaces`]: {
      items: [...alices, ...[3, 4, 5].map((n) => ({ ...own(n, `other-${n}`, `Someone else ${n}`, "member"), my_role: null }))],
    },
  },
  carol: {
    "/v1/me": me({ display_name: "Carol Chen", is_instance_admin: false }, [
      membership(org, "member", carols),
      membership(globex, "member", [own(30, "platform", "Platform", "member")]),
      membership(initech, "member", [own(31, "lab", "Lab", "member")]),
    ]),
    [`/v1/orgs/${id(1)}`]: { ...org, my_role: "member" },
    [`/v1/orgs/${id(1)}/workspaces`]: { items: carols },
  },
};

async function open(page: Page, who: keyof typeof PEOPLE, scheme: Scheme, path = `/o/${org.slug}/w/${ws.slug}`) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => m.type() === "error" && !/^Failed to load resource/.test(m.text()) && errors.push(m.text()));
  await mockApi(page, PEOPLE[who]);
  await page.route("**/v1/attention/counts*", (r) =>
    r.fulfill({
      json: {
        administration: who === "alice" ? { count: 1, topics: {} } : null,
        organization: who === "alice" ? { org_id: id(1), count: 2, topics: {} } : null,
        workspace: null,
      },
    }),
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

for (const scheme of ["light", "dark"] as const) {
  test.describe(`the place picker (${scheme}, 1256 px)`, () => {
    test.use({ viewport: { width: 1256, height: 900 } });
    const rail = (page: Page) => page.getByRole("complementary", { name: "Sidebar" });
    const openPicker = async (page: Page) => {
      await rail(page).getByTestId("scope-switcher").getByRole("button").click();
      return page.getByRole("dialog", { name: NAME });
    };

    test("Bob: only what he can use, and no Manage at the rail's foot", async ({ page }) => {
      const errors = await open(page, "bob", scheme);
      await expect(rail(page).getByRole("link", { name: /^Manage/ })).toHaveCount(0);
      const picker = await openPicker(page);
      await expect(picker.getByRole("link", { name: /Your workspace Owner · Acme Labs/ })).toBeVisible();
      await expect(picker.getByRole("button", { name: "New organization" })).toBeVisible();
      await expect(picker.getByRole("combobox")).toHaveCount(0);
      await expect(picker.getByRole("link", { name: /Manage|Administration/ })).toHaveCount(0);
      await clean(page, errors);
      await shot(page, `picker-bob-${scheme}`);
    });

    test("Alice: every place, labelled by kind; Manage at the rail's foot above Administration", async ({ page }) => {
      const errors = await open(page, "alice", scheme);
      const foot = rail(page).getByRole("link", { name: /^Manage Acme Labs/ });
      const admin = rail(page).getByRole("link", { name: /^Administration/ });
      await expect(foot).toBeVisible();
      expect((await foot.boundingBox())!.y).toBeLessThan((await admin.boundingBox())!.y);
      await clean(page, errors);
      await shot(page, `rail-foot-${scheme}`);
      const picker = await openPicker(page);
      await expect(picker.getByRole("combobox")).toBeFocused();
      for (const group of ["Workspaces in Acme Labs", "Organization", "Administration", "Other organizations"]) {
        await expect(picker.getByRole("group", { name: group, exact: true })).toBeVisible();
      }
      await expect(picker.getByRole("link", { name: "All 6 workspaces…" })).toBeVisible();
      await expect(picker.getByRole("link", { name: /Manage Acme Labs/ })).toContainText("People, apps, AI models, data");
      await expect(picker.getByRole("link", { name: /Globex/ })).toContainText("Member · opens your last workspace there");
      await expect(picker.getByText(/Someone else/)).toHaveCount(0);
      expect(await serious(page)).toEqual([]);
      await shot(page, `picker-alice-${scheme}`);
      // Escape returns the focus to the card.
      await page.keyboard.press("Escape");
      await expect(rail(page).getByTestId("scope-switcher").getByRole("button")).toBeFocused();
    });

    test("many places: search first, across organizations", async ({ page }) => {
      const errors = await open(page, "carol", scheme);
      const picker = await openPicker(page);
      await picker.getByRole("combobox").fill("plat");
      await expect(picker.getByRole("option")).toHaveCount(2);
      await expect(picker.getByText("14 workspaces, 3 organizations")).toBeVisible();
      await expect(picker.getByText("↑↓ to move · ⏎ to open · Esc to close")).toBeVisible();
      await clean(page, errors);
      await shot(page, `picker-search-${scheme}`);
      await page.keyboard.press("ArrowDown");
      await page.keyboard.press("Enter");
      await expect(page).toHaveURL(/\/o\/globex\/w\/platform$/);
    });

    test("Ctrl K finds Manage for Alice", async ({ page }) => {
      await open(page, "alice", scheme);
      await page.keyboard.press("Control+k");
      await page.getByRole("combobox", { name: "Search places and actions" }).fill("manage acme");
      await expect(page.getByRole("option", { name: "Manage Acme Labs" })).toBeVisible();
    });

    test("Ctrl K does not offer Manage to Bob", async ({ page }) => {
      await open(page, "bob", scheme);
      await page.keyboard.press("Control+k");
      await page.getByRole("combobox", { name: "Search places and actions" }).fill("manage acme");
      await expect(page.getByText(/Nothing matches/)).toBeVisible();
    });
  });

  test.describe(`places on a phone (${scheme}, 390 px)`, () => {
    test.use({ viewport: { width: 390, height: 844 } });

    test("the top bar shows the place card, which opens the picker as a sheet", async ({ page }) => {
      const errors = await open(page, "alice", scheme, `/o/${org.slug}`);
      const bar = page.getByTestId("topbar-place");
      await expect(bar).toHaveText(/Acme Labs\s*· Organization/);
      await clean(page, errors);
      await shot(page, `phone-topbar-${scheme}`);
      await bar.getByRole("button").click();
      const sheet = page.getByRole("dialog", { name: NAME });
      await expect(sheet.getByRole("link", { name: /Manage Acme Labs/ })).toHaveAttribute("aria-current", "true");
      // A sheet from the foot of the screen, the full width, inside the viewport.
      const box = (await sheet.boundingBox())!;
      expect(Math.round(box.x)).toBe(0);
      expect(Math.round(box.width)).toBe(390);
      expect(Math.round(box.y + box.height)).toBe(844);
      await expect(page.locator("select:visible")).toHaveCount(0);
      expect(await overflow(page)).toBeLessThanOrEqual(0);
      expect(await serious(page)).toEqual([]);
      await shot(page, `phone-sheet-${scheme}`);
      await page.keyboard.press("Escape");
      await expect(bar.getByRole("button")).toBeFocused();
      // The drawer opens from the menu button's side and shows the place's menu with its back line and the foot.
      await page.getByRole("button", { name: /^Open menu/ }).click();
      const drawer = page.getByRole("dialog", { name: "Navigation" });
      await expect(drawer.getByRole("navigation", { name: "Manage Acme Labs" })).toBeVisible();
      await expect(drawer.getByTestId("place-back")).toBeVisible();
      expect(Math.round((await drawer.boundingBox())!.x)).toBe(0);
      await shot(page, `phone-drawer-${scheme}`);
    });
  });
}
