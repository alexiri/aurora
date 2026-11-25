import { expect, test, type Page, type Route } from "@playwright/test";

import { AGENT_ROUTE, DEFAULT, MINE, mockChat } from "./support/chat-mock";
import { agent, id, mockApi, org, user, ws } from "./support/mock-api";
import { openThemed, overflow, titled } from "./support/open";

// FIX-235 (D-477, D-479): an organization admin who is not one of a workspace's people reaches
// it through the org Overview's manage link, pauses an agent there (never opens the agent); a
// member's own chat shows the visibility line (who uses the agent, who reads the chat); both
// pages hold together at 390 px. Mocked API throughout.

const json = (body: unknown) => ({ status: 200, contentType: "application/json", body: JSON.stringify(body) });

const NOW = new Date().toISOString();

/** `my_role: null` everywhere the viewer's own access is read: the session, the resolve, the workspace, the org list. */
const managed = { ...ws, my_role: null, administers: true, audience: { people: 2, owners: [{ id: id(99), display_name: "Ada Lovelace" }], owners_total: 1, emergency: null } };

/** The Overview's workspace row (the org agents' row links to the manage page too). */
const row = (page: Page) => page.getByRole("link", { name: "Main workspace Manage" });

async function adminMock(page: Page) {
  await mockApi(page);
  // The org's workspace list is fetched with query parameters; `*` keeps the glob on the path.
  await page.route("**/v1/workspaces/" + ws.id + "*", (route) => route.fulfill({ ...json(managed) }));
  await page.route("**/v1/orgs/" + org.id + "/workspaces*", (route) =>
    route.fulfill({ ...json({ items: [{ ...managed, personal_of: null }], next_cursor: null }) }),
  );
}

test.describe("the admin's journey (FIX-235, D-477)", () => {
  test("Overview → Manage → pause an agent, without ever opening the agent", async ({ page }) => {
    await adminMock(page);
    await openThemed(page, "/o/acme", "light");
    // The workspace is not the viewer's own: the row links to the manage page with the quiet label.
    const link = row(page);
    await expect(link).toBeVisible();
    await expect(link.getByTestId("manage-row")).toHaveText("Manage");
    await link.click();
    await page.waitForURL("**/o/acme/workspaces/main");
    await titled(page);

    // The manage page: the notice, the people, the agents.
    await expect(page.getByTestId("manage-notice")).toContainText("You manage this workspace as an organization admin.");
    await expect(page.getByRole("link", { name: "Daneel" })).toHaveCount(0);
    const pause = page.getByRole("button", { name: "Pause…" });
    await expect(pause).toBeVisible();

    // Pause: the confirm asks and says what it does; the POST carries the reason.
    let frozen = false;
    await page.route("**/v1/workspaces/" + ws.id + "/agents", (route: Route) => {
      const a = frozen ? { ...agent, frozen: { at: NOW, authority: "org_admin", by: { id: id(99), display_name: "Ada Lovelace" }, by_label: "ada@example.test", level: "agent", reason: "deploy" } } : agent;
      return route.fulfill({ ...json({ items: [a] }) });
    });
    await page.route("**/v1/agents/" + agent.id + "/freeze", (route) => {
      frozen = true;
      return route.fulfill({ ...json({ ...agent, frozen: { at: NOW, authority: "org_admin", by: { id: id(99), display_name: "Ada Lovelace" }, by_label: "ada@example.test", level: "agent", reason: "deploy" } }) });
    });
    await pause.click();
    const dialog = page.getByRole("dialog", { name: "Pause Daneel?" });
    await expect(dialog).toContainText("An emergency stop. Nothing is deleted and you can resume at any time.");
    await dialog.getByLabel("Reason").fill("deploy");
    await dialog.getByRole("button", { name: "Pause agent", exact: true }).click();
    // The list refetches: the row says Paused and offers Resume; the name is still not a link.
    await expect(page.getByRole("button", { name: "Resume agent" })).toBeVisible();
    await expect(page.getByText("Paused", { exact: true })).toBeVisible();
    await expect(page.getByRole("link", { name: "Daneel" })).toHaveCount(0);
  });
});

const ANN = { id: id(100), display_name: "Ann Appleton" };
// The reader (Ada, id 99) holds no own access beyond membership: the owners are two other people.
const memberAudience = { people: 3, owners: [{ id: id(98), display_name: "Grace Hopper" }, ANN], owners_total: 2, emergency: null };

test.describe("the member's visibility line (FIX-235, D-479)", () => {
  test("a member's own chat names the workspace's people and its owners", async ({ page }) => {
    await mockApi(page);
    await mockChat(page, { ...DEFAULT, events: { [MINE.id]: [] } });
    await page.route("**/v1/me", (route) =>
      route.fulfill({
        ...json({
          user,
          memberships: [{ org, status: "active", workspaces: [{ ...ws, my_role: "member", administers: false, personal: false }] }],
          default_org_id: org.id,
          client_address: "203.0.113.7",
        }),
      }),
    );
    await page.route("**/v1/workspaces/" + ws.id, (route) =>
      route.fulfill({ ...json({ ...ws, my_role: "member", administers: false, audience: memberAudience }) }),
    );
    await page.emulateMedia({ colorScheme: "light" });
    await page.goto(AGENT_ROUTE);
    await page.waitForLoadState("networkidle");
    const line = "All 3 people in this workspace can use this agent. This chat is visible to you and the workspace's owners: Grace Hopper, Ann Appleton.";
    // The line renders twice — the thread header's meta and the empty state under the greeting.
    await expect(page.getByText(line)).toHaveCount(2);
  });
});

test.describe("390 px (FIX-235)", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test("the manage page holds: no sideways scroll, the pause control visible", async ({ page }) => {
    await adminMock(page);
    await openThemed(page, "/o/acme/workspaces/main", "light");
    await expect(page.getByTestId("manage-notice")).toBeVisible();
    await expect(page.getByRole("button", { name: "Pause…" })).toBeVisible();
    expect(await overflow(page), "no sideways scroll").toBe(0);
  });

  test("the org Overview holds: the manage row visible, no sideways scroll", async ({ page }) => {
    await adminMock(page);
    await openThemed(page, "/o/acme", "light");
    const link = row(page);
    await expect(link).toBeVisible();
    await expect(link.getByTestId("manage-row")).toHaveText("Manage");
    expect(await overflow(page), "no sideways scroll").toBe(0);
  });
});
