import { expect, test, type Locator, type Page } from "@playwright/test";

import { AGENT_ROUTE, DEFAULT, MINE, mockChat } from "./support/chat-mock";
import { id, mockApi } from "./support/mock-api";
import { overflow } from "./support/open";

// P5-REFINE-20 batch 9 (access-3 at 390, the agent header at tablet widths): at every width from 390 up the agent's name is
// visible (h1 wider than 0), its state ("Can't answer: no AI key", the longest one) shows whole and is covered by nothing (not
// Status, not Settings, not the section tabs), the page never scrolls sideways, and the give-key card's button wraps inside
// its card. Mocked API, both themes.

const GIVE = { action: "give_key", key: { secret_id: id(60), name: "Company key", scope: "organization" } };

async function open(page: Page, scheme: "light" | "dark") {
  await mockApi(page);
  await mockChat(page, { ...DEFAULT, events: { [MINE.id]: [] } });
  await page.route("**/v1/**", (route) => {
    const req = route.request();
    if (req.method() === "GET" && /^\/v1\/agents\/[^/]+\/answer-readiness$/.test(new URL(req.url()).pathname)) {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ can_answer: false, reason: "no_ai_key", usable_keys: 0, fix: GIVE }),
      });
    }
    return route.fallback();
  });
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(AGENT_ROUTE);
  await page.getByRole("tab", { name: "Chat" }).click();
  await page.waitForLoadState("networkidle");
}

const box = async (l: Locator) => (await l.boundingBox())!;
const overlaps = (a: { x: number; y: number; width: number; height: number }, b: typeof a) =>
  a.x < b.x + b.width - 1 && b.x < a.x + a.width - 1 && a.y < b.y + b.height - 1 && b.y < a.y + a.height - 1;

for (const scheme of ["light", "dark"] as const) {
  for (const width of [390, 768, 820, 900, 1024, 1280]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("the agent header keeps the name and the whole state visible, with no sideways scroll", async ({ page }) => {
        await open(page, scheme);
        const header = page.locator("header").filter({ has: page.getByTestId("agent-state") });
        const h1 = header.getByRole("heading", { name: "Daneel", level: 1 });
        const state = page.getByTestId("agent-state");
        await expect(state).toContainText("Can't answer: no AI key");
        expect((await box(h1)).width, "the name is not squeezed out").toBeGreaterThan(40);
        // The h1 shows all of its text (not truncated to an ellipsis).
        expect(await h1.evaluate((e) => e.scrollWidth - e.clientWidth), "the name is not clipped").toBeLessThanOrEqual(0);
        // The state's text is whole: its own box is not clipped and no button or tab covers it.
        const text = state.locator("span[aria-live]");
        expect(await text.evaluate((e) => e.scrollWidth - e.clientWidth), "the state text is not clipped").toBeLessThanOrEqual(0);
        const stateBox = await box(text);
        const covers = [header.getByRole("button", { name: "Settings" }), header.getByRole("tablist")];
        if (width < 768) covers.push(header.getByRole("button", { name: "Status" }));
        for (const c of covers) expect(overlaps(stateBox, await box(c)), "the state is covered").toBe(false);
        // The state's own words are what is on top at their corners.
        for (const [px, py] of [
          [stateBox.x + 2, stateBox.y + 2],
          [stateBox.x + stateBox.width - 2, stateBox.y + stateBox.height - 2],
        ]) {
          expect(await page.evaluate(([x, y]) => !!document.elementFromPoint(x, y)?.closest("[data-testid=agent-state]"), [px, py]), "the state is on top").toBe(true);
        }
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
        // The card's (and the header row's) give-key button stays inside what holds it.
        const give = page.getByRole("region", { name: "Daneel has no AI model yet" }).getByRole("button", { name: "Give Daneel the Company key" });
        await expect(give).toBeVisible();
        const card = await box(page.getByRole("region", { name: "Daneel has no AI model yet" }));
        const gb = await box(give);
        expect(gb.x + gb.width, "the card's button stays in its card").toBeLessThanOrEqual(card.x + card.width + 1);
        const docWidth = await page.evaluate(() => document.documentElement.scrollWidth);
        const clientWidth = await page.evaluate(() => document.documentElement.clientWidth);
        expect(docWidth).toBe(clientWidth);
      });
    });
  }
}
