import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { DEFAULT, MINE, openChat, type Ev, type Fixture } from "./support/chat-mock";
import { id, mockApi, ws } from "./support/mock-api";
import { overflow } from "./support/open";

// P5-UX-17 (D-348, SEC-132, C-984): an agent asks for an app in the conversation (a card with one button, "Not now"),
// the conversation says when access was given, and the Accounts page shows the open requests. The API is mocked; the
// journey against the real stack (the agent asks, the person finishes the journey, the agent starts again by itself) is
// the API tests' `giving_access_by_each_route_…` plus the hosted suite.

const at = (m: number) => new Date(Date.now() - m * 60_000).toISOString();
const asked: Ev = {
  seq: 2, type: "notice", ts: at(5),
  payload: {
    code: "app_connection_requested", request_id: id(901), app_id: "linear", app_name: "Linear", reason: "to file the bug you described",
    text: "Daneel asks you to connect Linear (linear).", connect_url: "https://x/o/acme/w/main/agents/daneel?tab=access&connect=linear",
  },
};
const said: Ev = { seq: 1, type: "turn.queued", ts: at(6), payload: { turn_id: "t1", speaker_label: "Ada Lovelace", door: "dashboard", text: "File the bug in Linear." } };
const given: Ev = { seq: 3, type: "notice", ts: at(1), payload: { code: "app_access_given", request_id: id(901), app_id: "linear", app_name: "Linear", level: "ask", text: "Daneel can now read Linear, and asks you before each change." } };

const fx = (events: Ev[]): Fixture => ({ ...DEFAULT, events: { [MINE.id]: events } });

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`access requests, ${scheme}, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("the request is a card with one button that opens the journey; Not now dismisses it", async ({ page }) => {
        const dismissed: string[] = [];
        await openChat(page, scheme, fx([said, asked]), `?c=${MINE.id}`);
        await page.route("**/v1/access-requests/*/dismiss", async (route) => {
          dismissed.push(new URL(route.request().url()).pathname);
          await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ id: id(901), status: "dismissed" }) });
        });
        const card = page.getByTestId("access-request-card");
        await expect(card).toContainText("Daneel needs Linear for this");
        await expect(card).toContainText("Daneel's words");
        await expect(card).toContainText("to file the bug you described");
        const give = card.getByRole("link", { name: "Give Daneel Linear" });
        await expect(give).toHaveAttribute("href", /\/agents\/[^/]+\?tab=access&connect=linear$/);
        expect(await overflow(page), "horizontal scroll").toBeFalsy();
        expect(await serious(page)).toEqual([]);
        await card.getByRole("button", { name: "Not now" }).click();
        await expect.poll(() => dismissed).toHaveLength(1);
      });

      test("once access was given the card folds away and the conversation says so", async ({ page }) => {
        await openChat(page, scheme, fx([said, asked, given]), `?c=${MINE.id}`);
        await expect(page.getByTestId("access-request-card")).toHaveCount(0);
        await expect(page.getByTestId("access-request-outcome")).toHaveText("Daneel can now read Linear, and asks you before each change.");
        expect(await overflow(page), "horizontal scroll").toBeFalsy();
        expect(await serious(page)).toEqual([]);
      });
    });
  }

  test(`the Accounts page lists open requests above the list (${scheme})`, async ({ page }) => {
    await mockApi(page);
    await page.route(`**/v1/workspaces/${ws.id}/access-requests**`, (route) =>
      route.fulfill({
        status: 200, contentType: "application/json",
        body: JSON.stringify({
          items: [{
            id: id(901), agent_id: id(11), agent_name: "Daneel", agent_handle: "daneel", conversation_id: id(201), kind: "app",
            app_id: "linear", app_name: "Linear", secret_id: null, host: null, reason: "r", status: "open", created_at: at(3),
          }],
        }),
      }),
    );
    await page.emulateMedia({ colorScheme: scheme });
    await page.goto("/o/acme/w/main/apps?tab=accounts");
    const banner = page.getByTestId("requests-banner");
    await expect(banner).toContainText("Daneel asked for Linear");
    await expect(banner.getByRole("link", { name: "Give Daneel Linear" })).toHaveAttribute("href", `/o/acme/w/main/agents/${id(11)}?tab=access&connect=linear`);
    expect(await overflow(page), "horizontal scroll").toBeFalsy();
    expect(await serious(page)).toEqual([]);
  });
}
