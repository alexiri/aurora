import { expect as baseExpect, test, type Page } from "@playwright/test";

import { serious } from "./support/axe";
import { agent, id, mockApi, org, user, ws } from "./support/mock-api";
import { overflow, titled } from "./support/open";

// The first request of a route compiles it in `next dev`; on a loaded host that outlasts the default 5 s.
const expect = baseExpect.configure({ timeout: 30_000 });

// P5-REFINE-19 (D-406): the command palette (⌘K / Ctrl K), the keyboard shortcuts and their sheet. Nouns are for everyone;
// verbs and the letter shortcuts only with "Show more controls" on; a verb opens the button's own dialog and never acts.

const person = { ...user, is_instance_admin: false };
const me = {
  "/v1/me": {
    user: person,
    memberships: [{ org: { ...org, my_role: "owner" }, status: "active", workspaces: [{ ...ws, my_role: "owner", personal: false }] }],
    default_org_id: id(1),
    client_address: "203.0.113.7",
  },
  [`/v1/orgs/${id(1)}`]: { ...org, my_role: "owner" },
  [`/v1/workspaces/${id(2)}`]: { ...ws, my_role: "owner" },
};

async function open(page: Page, path: string, scheme: "light" | "dark", more: boolean) {
  const errors: string[] = [];
  const writes: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => {
    if (m.type() === "error" && !/^Failed to load resource/.test(m.text())) errors.push(m.text());
  });
  page.on("request", (r) => {
    if (r.method() !== "GET" && /freeze|pause/.test(r.url())) writes.push(`${r.method()} ${r.url()}`);
  });
  await mockApi(page, me);
  await page.route("**/v1/me/onboarding", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ welcome_seen: true, guide_dismissed: [], tour_completed_at: null, notices_seen: ["renames-2026-11"], more_controls: more }),
    }),
  );
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await titled(page);
  return { errors, writes };
}

const search = (page: Page) => page.getByRole("combobox", { name: "Search places and actions" });

for (const scheme of ["light", "dark"] as const) {
  for (const [width, label] of [[1256, "desktop"], [390, "phone"]] as const) {
    test.describe(`palette and shortcuts (${scheme}, ${label})`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("Ctrl K opens the palette with nouns only at Use; choosing an agent goes to it; axe, no scroll, no errors", async ({ page }) => {
        const { errors } = await open(page, "/o/acme/w/main/agents", scheme, false);
        await page.keyboard.press("Control+k");
        await expect(search(page)).toBeFocused();
        await search(page).fill("dan");
        await expect(page.getByRole("option", { name: /Daneel/ }).first()).toBeVisible();
        await expect(page.getByRole("option", { name: /Pause Daneel/ })).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
        await page.keyboard.press("Enter");
        await expect(page).toHaveURL(new RegExp(`/agents/${agent.handle}$`));
        expect(errors).toEqual([]);
      });

      test("says what matched nothing", async ({ page }) => {
        await open(page, "/o/acme/w/main/agents", scheme, false);
        await page.keyboard.press("Control+k");
        await search(page).fill("zzzzqqq");
        await expect(page.getByText("Nothing matches “zzzzqqq”.")).toBeVisible();
        await page.keyboard.press("Escape");
        await expect(search(page)).toHaveCount(0);
      });

      test("with Show more controls on, Pause <Agent> opens the Pause dialog and pauses nothing", async ({ page }) => {
        const { errors, writes } = await open(page, "/o/acme/w/main/agents", scheme, true);
        await page.keyboard.press("Control+k");
        await search(page).fill("pause dan");
        await expect(page.getByRole("option", { name: /Pause Daneel/ })).toBeVisible();
        expect(await serious(page)).toEqual([]);
        await page.keyboard.press("Enter");
        await expect(page).toHaveURL(/agents\/daneel\?tab=instructions/);
        await expect(page.getByRole("dialog")).toBeVisible();
        expect(writes).toEqual([]);
        expect(errors).toEqual([]);
      });

      test("? opens the shortcut sheet with the shortcuts by group; Escape closes it", async ({ page }) => {
        await open(page, "/o/acme/w/main/agents", scheme, true);
        await page.locator("body").press("?");
        const sheet = page.getByTestId("shortcut-sheet");
        await expect(sheet).toBeVisible();
        await expect(sheet.getByText("Go to Agents")).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
        await page.keyboard.press("Escape");
        await expect(sheet).toHaveCount(0);
      });

      test("g then a goes to the agents; typing in a field never does", async ({ page }) => {
        await open(page, "/o/acme/w/main", scheme, true);
        await page.locator("body").press("g");
        await page.locator("body").press("a");
        await expect(page).toHaveURL(/\/o\/acme\/w\/main\/agents$/);
        // In a text field the same keys are text.
        await page.keyboard.press("Control+k");
        await search(page).fill("ga");
        await expect(search(page)).toHaveValue("ga");
        await expect(page).toHaveURL(/\/o\/acme\/w\/main\/agents$/);
      });

      test("with the switch off the letter shortcuts do nothing and ? does not open the sheet", async ({ page }) => {
        await open(page, "/o/acme/w/main", scheme, false);
        await page.locator("body").press("g");
        await page.locator("body").press("a");
        await page.locator("body").press("?");
        await expect(page).toHaveURL(/\/o\/acme\/w\/main$/);
        await expect(page.getByTestId("shortcut-sheet")).toHaveCount(0);
      });
    });
  }
}

test.describe("the rail's Search button", () => {
  test.use({ viewport: { width: 1256, height: 900 } });
  test("is in the footer with the shortcut and opens the palette", async ({ page }) => {
    await open(page, "/o/acme/w/main/agents", "light", false);
    const rail = page.getByRole("complementary", { name: "Sidebar" });
    const button = rail.getByRole("button", { name: /^Search/ });
    await expect(button).toContainText(/Ctrl K|⌘K/);
    await button.click();
    await expect(search(page)).toBeFocused();
  });
});
