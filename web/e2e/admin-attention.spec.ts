import { expect as baseExpect, test, type Page } from "@playwright/test";

import { serious } from "./support/axe";
import { id, mattermostConnector, mockApi } from "./support/mock-api";
import { CANVAS, overflow } from "./support/open";

// FIX-167 Part B (D-370, C-1022): what needs an instance admin. The count on the Administration entry, the
// "Needs you" card on Overview, and the Connectors card of a connector whose widened update waits.
// The API is mocked; themed, no horizontal scroll at 390 px, no serious axe violation.
const expect = baseExpect.configure({ timeout: 30_000 });

const SINCE = "2026-11-18T10:00:00Z";
// (C-1080) The shape every level's feed shares.
const common = { level: "administration", actors: [], actors_total: 0, reason: null, own: false, can_decline: false, state: "open", closed_at: null, message: null, closed_by_role: null, closed_by: null };
const ITEMS = [
  {
    ...common, id: "connector_approval:mattermost", topic: "chat-platforms", kind: "connector_approval", subject_id: "mattermost", title: "Mattermost 1.3.0 is waiting for approval",
    detail: "It keeps running at the level you approved; only what is new waits. It adds: native_status, user_tokens.",
    since: SINCE, href: "/admin/chat-platforms", decision: true, action: { label: "Review", href: "/admin/chat-platforms" },
  },
  {
    ...common, id: `package_approval:${id(700)}`, topic: "apps", kind: "package_approval", subject_id: id(700), title: "Indigo SSO 2.0.0 needs your approval",
    detail: "It asks for more than the approved version. The approved version keeps running.",
    since: SINCE, href: `/admin/apps/packages/${id(700)}`, decision: true, action: { label: "Review", href: `/admin/apps/packages/${id(700)}` },
  },
  {
    ...common, id: "connector_down:mattermost", topic: "chat-platforms", kind: "connector_down", subject_id: "mattermost", title: "Mattermost is not running",
    detail: "Its connector host reports an error.", since: SINCE, href: "/admin/chat-platforms", decision: false, action: { label: "Open", href: "/admin/chat-platforms" },
  },
];
/** (C-1082) The rails' numbers for a list of items: the total and the open items per topic. */
const countsOf = (items: typeof ITEMS) => ({
  administration: { count: items.length, topics: items.reduce<Record<string, number>>((t, i) => ({ ...t, [i.topic]: (t[i.topic] ?? 0) + 1 }), {}) },
  organization: null,
  workspace: null,
});
const pendingConnector = { ...mattermostConnector, update_pending: true, pending_version: "1.3.0", widening: ["capability: native_status"] };

async function open(page: Page, path: string, scheme: "light" | "dark", overrides: Record<string, unknown> = {}) {
  await mockApi(page, { "/v1/admin/attention": { items: ITEMS, count: ITEMS.length }, "/v1/attention/counts": countsOf(ITEMS), ...overrides });
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
        await expect(section.getByRole("heading", { name: /^Needs you/ })).toBeVisible();
        const rows = section.getByRole("listitem");
        await expect(rows).toHaveCount(3);
        await expect(rows.nth(0)).toContainText("Mattermost 1.3.0 is waiting for approval");
        await expect(rows.nth(2)).toContainText("Mattermost is not running");
        await expect(section.getByRole("link", { name: "Review: Mattermost 1.3.0 is waiting for approval" })).toHaveAttribute("href", "/admin/chat-platforms");
        await expect(section.getByRole("link", { name: "Open: Mattermost is not running" })).toBeVisible();
        expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
        expect(await overflow(page), "Overview overflows horizontally").toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });

      test(`a connector with an update waiting shows Running and a second chip (${scheme})`, async ({ page }) => {
        await open(page, "/admin/chat-platforms", scheme, { "/v1/admin/connectors": { items: [pendingConnector] } });
        // (P5-REFINE-18 #7) The connector cards sit in the closed Advanced fold under the platforms list.
        await page.getByText(/Advanced · connector hosts/).click();
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
    await open(page, "/admin", "light", { "/v1/admin/attention": { items: [], count: 0 }, "/v1/attention/counts": countsOf([]) });
    // The page's own heading (the mock's setup status decides between "Set up Aurora" and "Overview").
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    await expect(page.getByTestId("admin-attention")).toHaveCount(0);
    await expect(page.getByTestId("admin-attention-badge")).toHaveCount(0);
  });

  test("a failed fetch shows no badge and no error", async ({ page }) => {
    await open(page, "/admin", "light", {
      "/v1/admin/attention": { status: 500, body: { error: { code: "internal", message: "x" } } },
      "/v1/attention/counts": { status: 500, body: { error: { code: "internal", message: "x" } } },
    });
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    await expect(page.getByTestId("admin-attention-badge")).toHaveCount(0);
    await expect(page.getByTestId("admin-attention")).toHaveCount(0);
  });
});

// (P5-REFINE-18 #18, D-417) The count is on every entry (Overview in the Administration rail, the sidebar foot) and
// drops when an item is resolved, without a reload.
test.describe("the Administration count follows what is resolved", () => {
  test.use({ viewport: { width: 1280, height: 900 } });
  test("approving the package drops the count on Overview and on the foot entry", async ({ page }) => {
    const pkgId = id(700);
    let waiting = true;
    const pkg = (status: string) => ({
      id: pkgId, package_id: "indigo-sso", version: "2.0.0", contract: 1, publisher: "acme", digest: "d".repeat(64), signer: { kind: "unsigned" },
      source: "upload", origin: "uploaded", summary: null, reaches: [], reaches_more: 0, apps: [{ app_id: "indigo", name: "Indigo SSO" }],
      status, status_reason: status === "active" ? null : "first_install", violations: [], widenings: [], host_claims: [],
      permissions: { hooks: [], egress: {}, context: [] }, impact: null, approved_by: null, approved_at: null, created_at: SINCE,
    });
    await mockApi(page, { "/v1/admin/attention": { items: ITEMS, count: ITEMS.length } });
    await page.route("**/v1/**", async (route) => {
      const p = new URL(route.request().url()).pathname;
      const method = route.request().method();
      const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
      const now = waiting ? ITEMS : ITEMS.filter((i) => i.kind !== "package_approval");
      if (p === "/v1/admin/attention") return json({ items: now, count: now.length });
      if (p === "/v1/attention/counts") return json(countsOf(now));
      if (p === `/v1/admin/plugins/${pkgId}` && method === "GET") return json(pkg(waiting ? "pending_approval" : "active"));
      if (p === `/v1/admin/plugins/${pkgId}/approve` && method === "POST") {
        waiting = false;
        return json({ ...pkg("active"), connections_reauth: 0 });
      }
      if (p === "/v1/admin/plugins" && method === "GET") return json({ items: [pkg(waiting ? "pending_approval" : "active")], next_cursor: null });
      return route.fallback();
    });
    await page.goto(`/admin/apps/packages/${pkgId}`);
    const rail = page.getByRole("navigation", { name: "Administration" });
    await expect(rail.getByTestId("admin-attention-badge")).toHaveText(/3/);
    await expect(page.getByRole("complementary", { name: "Sidebar" }).getByRole("link", { name: /^Administration 3/ })).toBeVisible();
    await page.getByRole("button", { name: "Approve" }).click();
    await expect(rail.getByTestId("admin-attention-badge")).toHaveText(/2/);
    await expect(page.getByRole("complementary", { name: "Sidebar" }).getByRole("link", { name: /^Administration 2/ })).toBeVisible();
  });
});
