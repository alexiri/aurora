import { expect as baseExpect, test, type Page } from "@playwright/test";

import { serious } from "./support/axe";
import { id, mattermostConnector, mockApi } from "./support/mock-api";
import { CANVAS, overflow } from "./support/open";

// FIX-167 Part B (D-370, C-1022): what needs an instance admin. The count on the Administration entry, the
// "Needs your attention" section on Overview, and the Connectors card of a connector whose widened update waits.
// The API is mocked; themed, no horizontal scroll at 390 px, no serious axe violation.
const expect = baseExpect.configure({ timeout: 30_000 });

const SINCE = "2026-11-18T10:00:00Z";
const ITEMS = [
  {
    kind: "connector_approval", subject_id: "mattermost", title: "Mattermost 1.3.0 is waiting for approval",
    detail: "It keeps running at the level you approved; only what is new waits. It adds: native_status, user_tokens.",
    since: SINCE, href: "/admin/chat-platforms", decision: true,
  },
  {
    kind: "package_approval", subject_id: id(700), title: "Indigo SSO 2.0.0 needs your approval",
    detail: "It asks for more than the approved version. The approved version keeps running.",
    since: SINCE, href: `/admin/apps/packages/${id(700)}`, decision: true,
  },
  {
    kind: "connector_down", subject_id: "mattermost", title: "Mattermost is not running",
    detail: "Its connector host reports an error.", since: SINCE, href: "/admin/chat-platforms", decision: false,
  },
];
const pendingConnector = { ...mattermostConnector, update_pending: true, pending_version: "1.3.0", widening: ["capability: native_status"] };

async function open(page: Page, path: string, scheme: "light" | "dark", overrides: Record<string, unknown> = {}) {
  await mockApi(page, { "/v1/admin/attention": { items: ITEMS }, ...overrides });
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await expect.poll(() => page.locator("html").getAttribute("data-theme")).toBe(scheme);
}

for (const width of [1280, 390]) {
  test.describe(`admin attention (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      test(`Overview lists what needs the admin, decisions first (${scheme})`, async ({ page }) => {
        await open(page, "/admin", scheme);
        const section = page.getByTestId("admin-attention");
        await expect(section.getByRole("heading", { name: "Needs your attention" })).toBeVisible();
        const rows = section.getByRole("listitem");
        await expect(rows).toHaveCount(3);
        await expect(rows.nth(0)).toContainText("Mattermost 1.3.0 is waiting for approval");
        await expect(rows.nth(0)).toContainText("Since");
        await expect(rows.nth(2)).toContainText("Mattermost is not running");
        await expect(section.getByRole("link", { name: "Review: Mattermost 1.3.0 is waiting for approval" })).toHaveAttribute("href", "/admin/chat-platforms");
        await expect(section.getByRole("link", { name: "Open: Mattermost is not running" })).toBeVisible();
        expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
        expect(await overflow(page), "Overview overflows horizontally").toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });

      test(`a connector with an update waiting shows Running and a second chip (${scheme})`, async ({ page }) => {
        await open(page, "/admin/chat-platforms", scheme, { "/v1/admin/connectors": { items: [pendingConnector] } });
        const card = page.getByRole("listitem").filter({ hasText: "Mattermost" }).first();
        await expect(card.getByTestId("status-chip")).toHaveText("Running");
        await expect(card.getByTestId("update-pending-chip")).toHaveText("Update waiting for approval");
        expect(await overflow(page), "Connectors overflows horizontally").toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });
    }
  });
}

test.describe("the Administration entry", () => {
  test.use({ viewport: { width: 1280, height: 900 } });
  test("carries the count of items", async ({ page }) => {
    await open(page, "/admin", "light");
    await expect(page.getByTestId("admin-attention-badge").first()).toHaveText(/3/);
    await expect(page.getByRole("complementary", { name: "Sidebar" }).getByRole("link", { name: /^Administration 3/ })).toBeVisible();
  });

  test("shows no badge and no section with nothing to attend to", async ({ page }) => {
    await open(page, "/admin", "light", { "/v1/admin/attention": { items: [] } });
    await expect(page.getByRole("heading", { name: "Overview", level: 1 })).toBeVisible();
    await expect(page.getByTestId("admin-attention")).toHaveCount(0);
    await expect(page.getByTestId("admin-attention-badge")).toHaveCount(0);
  });

  test("a failed fetch shows no badge and no error", async ({ page }) => {
    await open(page, "/admin", "light", { "/v1/admin/attention": { status: 500, body: { error: { code: "internal", message: "x" } } } });
    await expect(page.getByRole("heading", { name: "Overview", level: 1 })).toBeVisible();
    await expect(page.getByTestId("admin-attention-badge")).toHaveCount(0);
    await expect(page.getByTestId("admin-attention")).toHaveCount(0);
  });
});
