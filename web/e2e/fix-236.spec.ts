import { expect, test, type Page, type Route } from "@playwright/test";

import { AGENT_ROUTE, ANN_DIRECT, MINE, mockChat } from "./support/chat-mock";
import { id, mockApi, now, ws } from "./support/mock-api";
import { openThemed, overflow, titled } from "./support/open";

// FIX-236 (D-478, C-1164, C-1165): an organization owner opens emergency read access from the
// manage page (reason, then the danger step), lands in the workspace in emergency mode — a banner
// on every page, the rail reduced, a direct chat readable with no composer — and ends it from the
// banner. A member of the workspace sees the banner with the reason, at 390 px too. Mocked API.

const json = (body: unknown, status = 200) => ({ status, contentType: "application/json", body: JSON.stringify(body) });

const REASON = "Legal hold on the account's records";

/** `audience.emergency` while the grant is active (C-1165: the newest active-or-recent row). */
const grant = (active: boolean, over: Record<string, unknown> = {}) => ({
  id: id(500),
  by: { id: id(99), display_name: "Ada Lovelace" },
  reason: REASON,
  opened_at: now,
  expires_at: new Date(Date.now() + 3_600_000).toISOString(),
  ended_at: null,
  active,
  ...over,
});

/** The full history row (C-1164), as the POST answers it. */
const row = (active: boolean) => ({ ...grant(active), workspace: { id: ws.id, name: ws.name }, end_reason: null });

/** The workspace as the emergency reader sees it: no own-right role, `emergency_reader` only while the grant is active. */
const readerWs = (emergency: ReturnType<typeof grant>) => ({
  ...ws, my_role: null, administers: false, emergency_reader: emergency.active,
  audience: { people: 2, owners: [{ id: id(99), display_name: "Ada Lovelace" }], owners_total: 1, emergency },
});

/** The manage page's workspace (FIX-235's shape): the viewer manages it without own access. */
const managed = { ...ws, my_role: null, administers: true, emergency_reader: false, audience: { ...ws.audience, emergency: null } };

/** The owner journey's mock: a mutable grant, opened by the POST and closed by the end route. */
async function ownerMock(page: Page) {
  let emergency: ReturnType<typeof grant> | null = null;
  await mockApi(page);
  await page.route("**/v1/workspaces/" + ws.id, (route: Route) =>
    route.fulfill({ ...json(emergency ? readerWs(emergency) : managed) }),
  );
  await page.route("**/v1/workspaces/" + ws.id + "/emergency-access", (route: Route) => {
    if (route.request().method() === "POST") {
      const body = route.request().postDataJSON() as { reason: string; hours?: number | null };
      expect(body.reason).toBe(REASON);
      emergency = grant(true);
      return route.fulfill({ ...json(row(true), 201) });
    }
    return route.fulfill({ ...json({ items: emergency ? [row(true)] : [], next_cursor: null }) });
  });
  await page.route("**/v1/emergency-access/*/end", (route: Route) => {
    emergency = grant(false, { ended_at: new Date().toISOString(), active: false });
    return route.fulfill({ ...json(row(false)) });
  });
  return () => emergency;
}

test.describe("the owner's journey (FIX-236, D-478)", () => {
  test("opens it from the manage page, reads a direct chat with no composer, ends it", async ({ page }) => {
    const grantNow = await ownerMock(page);
    await mockChat(page, { direct: MINE, active: [MINE, ANN_DIRECT], archived: [], events: { [MINE.id]: [], [ANN_DIRECT.id]: [] } });
    await openThemed(page, "/o/acme/workspaces/main", "light");

    // The manage page offers the emergency read; the dialog asks why and for how long.
    await expect(page.getByTestId("open-emergency")).toBeVisible();
    await page.getByTestId("open-emergency").click();
    const form = page.getByRole("dialog", { name: "Open emergency read access" });
    await form.getByLabel("Reason").fill(REASON);
    await expect(form.getByLabel("How long")).toHaveValue("4");
    await form.getByRole("button", { name: "Continue" }).click();

    // The danger step says what happens; confirming opens the grant and goes to the workspace.
    const danger = page.getByRole("dialog", { name: "Open emergency read access to Main workspace?" });
    await expect(danger).toContainText(
      "Everyone in Main workspace and the organization's other owners will be told, with your reason. Everything you open is recorded. You can read, not act.",
    );
    await danger.getByRole("button", { name: "Open emergency read access to Main workspace" }).click();
    await page.waitForURL("**/o/acme/w/main");

    // Emergency mode: the banner with "End now", on the workspace's pages.
    await expect(page.getByTestId("emergency-reader-banner")).toContainText("Emergency read access until");
    await expect(page.getByTestId("emergency-reader-banner")).toContainText("Everything you open is recorded");
    expect(grantNow()).not.toBeNull();

    // The rail is the fixed read-only set: no Settings, no Approvals.
    const rail = page.locator('nav[aria-label="Workspace"]');
    await expect(rail.getByRole("link", { name: "People" })).toBeVisible();
    await expect(rail.getByRole("link", { name: "Settings" })).toHaveCount(0);

    // The chat: every conversation, direct ones included — read, with no way to act.
    await page.goto(AGENT_ROUTE);
    await titled(page);
    const chatTab = page.getByRole("tab", { name: "Chat" });
    await expect(chatTab).toBeEnabled();
    await chatTab.click();
    await page.locator('nav[aria-label="Conversations"]').waitFor({ state: "attached" });
    await page.getByRole("button", { name: /Grace Hopper/ }).click();
    await expect(page.getByTestId("emergency-thread-notice")).toBeVisible();
    await expect(page.getByTestId("emergency-thread-notice")).toContainText("everything you open is recorded");
    await expect(page.locator("#chat-input"), "no composer").toHaveCount(0);
    await expect(page.getByRole("button", { name: "Conversation actions" })).toHaveCount(0);

    // The thread's line names the emergency read (D-479's appended line).
    await expect(page.getByTestId("thread-meta")).toContainText("can read this workspace's conversations until");

    // End now: the banner goes, and the workspace answers without the grant again.
    await page.getByTestId("emergency-end-now").click();
    await expect(page.getByTestId("emergency-reader-banner")).toHaveCount(0);
  });
});

test.describe("the workspace's people are told (FIX-236, C-1165)", () => {
  test("a member's workspace shows the banner with the reason, and it is dismissible", async ({ page }) => {
    await mockApi(page);
    await page.route("**/v1/workspaces/" + ws.id, (route: Route) =>
      route.fulfill({
        ...json({
          ...ws, my_role: "member", administers: false, emergency_reader: false,
          audience: { people: 3, owners: [{ id: id(98), display_name: "Grace Hopper" }], owners_total: 1, emergency: grant(true) },
        }),
      }),
    );
    await openThemed(page, "/o/acme/w/main", "light");
    const banner = page.getByTestId("emergency-people-banner");
    await expect(banner).toContainText("Ada Lovelace, an organization owner, has emergency read access to this workspace until");
    await expect(banner).toContainText(`Reason: ${REASON}.`);
    // Dismissing remembers, per person, per grant.
    await page.getByTestId("emergency-banner-dismiss").click();
    await expect(page.getByTestId("emergency-people-banner")).toHaveCount(0);
  });
});

test.describe("390 px (FIX-236)", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test("a member's workspace holds with the banner: no sideways scroll", async ({ page }) => {
    await mockApi(page);
    await page.route("**/v1/workspaces/" + ws.id, (route: Route) =>
      route.fulfill({
        ...json({
          ...ws, my_role: "member", administers: false, emergency_reader: false,
          audience: { people: 3, owners: [{ id: id(98), display_name: "Grace Hopper" }], owners_total: 1, emergency: grant(true) },
        }),
      }),
    );
    await openThemed(page, "/o/acme/w/main", "light");
    await expect(page.getByTestId("emergency-people-banner")).toBeVisible();
    expect(await overflow(page), "no sideways scroll").toBe(0);
  });

  test("the manage page holds with the open dialog: no sideways scroll", async ({ page }) => {
    await ownerMock(page);
    await openThemed(page, "/o/acme/workspaces/main", "light");
    await page.getByTestId("open-emergency").click();
    const form = page.getByRole("dialog", { name: "Open emergency read access" });
    await form.getByLabel("Reason").fill(REASON);
    await expect(form).toBeVisible();
    expect(await overflow(page), "no sideways scroll").toBe(0);
  });
});
