import { expect as baseExpect, test, type Page } from "@playwright/test";

import { serious } from "./support/axe";
import { id } from "./support/mock-api";
import { overflow } from "./support/open";
import { mockAgentTemplates } from "./support/agent-templates";
import { mockWorkspaceTemplates } from "./support/new-agent-templates";

// FIX-245: the whole template journey follows real links, with the API mocked. The administration
// Overview's Review button lands on the review page (the address the backend really emits, pinned by
// the Rust test in `dash_paths.rs`), the waiting version's badge sits on What agents may do and not
// on Apps, and a person reaches the gallery from the Agents page and deploys from it. Themed, no
// horizontal scroll at 390 px, no serious axe violation on the pages the journeys touch.
const expect = baseExpect.configure({ timeout: 30_000 });

const SINCE = "2026-11-25T10:00:00Z";
// The shape every level's feed shares (C-1080).
const common = { level: "administration", actors: [], actors_total: 0, reason: null, own: false, can_decline: false, state: "open", closed_at: null, message: null, closed_by_role: null, closed_by: null };
// The item exactly as `GET /v1/admin/attention` returns it for a waiting template version: the
// `internet-access` topic and the review-page href are what the backend builds (FIX-245).
const waitingVersion = {
  ...common, id: `agent_template_version:${id(900)}`, topic: "internet-access", kind: "agent_template_version", subject_id: id(900),
  title: "Schedule manager 1.1.0 is waiting for review",
  detail: "A new agent template version was read from its source. Review and approve or reject it; people cannot use it until you approve.",
  since: SINCE, href: "/admin/internet-access/templates/schedule-manager/1.1.0", decision: true,
  action: { label: "Review", href: "/admin/internet-access/templates/schedule-manager/1.1.0" },
};
const attentionRoutes = (items: unknown[]) => ({
  "/v1/admin/attention": { items, count: items.length },
  "/v1/attention/counts": {
    administration: {
      count: items.length,
      topics: items.reduce<Record<string, number>>((t, i) => ({ ...t, [(i as { topic: string }).topic]: (t[(i as { topic: string }).topic] ?? 0) + 1 }), {}),
    },
    organization: null,
    workspace: null,
  },
});

/** Registers the attention routes on top of the Agent templates mocks (later routes win). */
async function mockAdminAttention(page: Page, items: unknown[]) {
  const counts = attentionRoutes(items)["/v1/attention/counts"];
  const routes: [RegExp, unknown][] = [
    [/\/v1\/admin\/attention(\?.*)?$/, { items, count: items.length }],
    [/\/v1\/attention\/counts(\?.*)?$/, counts],
  ];
  for (const [pattern, body] of routes) {
    await page.route(pattern, (route) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) }));
  }
}

async function openAdmin(page: Page, scheme: "light" | "dark", path: string) {
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await expect.poll(() => page.locator("html").getAttribute("data-theme")).toBe(scheme);
  await page.waitForLoadState("networkidle");
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`the template journey in Administration (${scheme})`, () => {
    test.use({ viewport: { width: 1280, height: 900 } });

    // (i) Overview lists the waiting version → Review → the review page (not a 404) → approve.
    test(`Overview's Review button lands on the review page and approves (${scheme})`, async ({ page }) => {
      const mock = await mockAgentTemplates(page);
      await mockAdminAttention(page, [waitingVersion]);
      await openAdmin(page, scheme, "/admin");

      const section = page.getByTestId("admin-attention");
      await expect(section.getByText("Schedule manager 1.1.0 is waiting for review")).toBeVisible();
      const review = section.getByRole("link", { name: "Review: Schedule manager 1.1.0 is waiting for review" });
      await expect(review).toHaveAttribute("href", "/admin/internet-access/templates/schedule-manager/1.1.0");
      await review.click();

      // The review page, reached through the link: not a 404.
      await expect(page).toHaveURL(/\/admin\/internet-access\/templates\/schedule-manager\/1\.1\.0$/);
      await expect(page.getByRole("heading", { name: "Schedule manager 1.1.0" })).toBeVisible();
      expect(await serious(page)).toEqual([]);

      await page.getByRole("button", { name: "Approve" }).click();
      await expect.poll(() => mock.writes.at(-1)).toEqual({ key: "POST /v1/admin/agent-templates/schedule-manager/versions/1.1.0/approve", body: { content_sha256: "c".repeat(64) } });
    });

    // (ii) The badge sits on What agents may do, not on Apps.
    test(`the waiting version is counted on What agents may do, not on Apps (${scheme})`, async ({ page }) => {
      await mockAgentTemplates(page);
      await mockAdminAttention(page, [waitingVersion]);
      await openAdmin(page, scheme, "/admin");

      const rail = page.getByRole("navigation", { name: "Administration" });
      await expect(rail.getByRole("link", { name: /^What agents may do/ })).toContainText("1");
      await expect(page.getByTestId("admin-attention-badge-internet-access").getByText("1", { exact: true })).toBeVisible();
      // Nothing on Apps: the Apps page resolves none of the waiting versions.
      await expect(page.getByTestId("admin-attention-badge-apps")).toHaveCount(0);
      expect(await serious(page)).toEqual([]);
    });
  });
}

// (iii) A person's way in: the Agents page → Start from a template → gallery → Use this template →
// the four-step page. The person is not an instance administrator: the gallery lists the approved
// template without mentioning Administration.
for (const width of [1280, 390]) {
  test.describe(`the way in from Agents (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      test(`Agents page → Start from a template → the four-step page (${scheme})`, async ({ page }) => {
        await mockWorkspaceTemplates(page);
        // A person, not an instance administrator.
        await page.route(/\/v1\/me(\?.*)?$/, (route) =>
          route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ id: id(99), display_name: "Pat Person", email: "pat@example.test" }) }),
        );
        await page.emulateMedia({ colorScheme: scheme });
        await page.goto("/o/acme/w/main/agents");
        await page.locator("h1, h2").first().waitFor();
        await expect.poll(() => page.locator("html").getAttribute("data-theme")).toBe(scheme);
        expect(await overflow(page), "Agents overflows horizontally").toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);

        await page.getByRole("button", { name: "Start from a template" }).click();
        const dlg = page.getByRole("dialog");
        await expect(dlg.getByText("Schedule manager").first()).toBeVisible();
        expect(await serious(page)).toEqual([]);

        await dlg.getByRole("button", { name: /Schedule manager/ }).click();
        await expect(dlg.getByRole("region", { name: "About this template" })).toBeVisible();
        await dlg.getByRole("button", { name: "Use this template" }).click();
        await expect(page).toHaveURL(/\/o\/acme\/w\/main\/agents\/new\/schedule-manager$/);
        await expect(page.getByRole("heading", { name: "New agent from Schedule manager" })).toBeVisible();
        expect(await serious(page)).toEqual([]);
      });
    }
  });
}
