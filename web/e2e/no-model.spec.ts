import { expect, test, type Page } from "@playwright/test";

import { AGENT_ROUTE, DEFAULT, MINE, mockChat } from "./support/chat-mock";
import { serious } from "./support/axe";
import { id, mockApi } from "./support/mock-api";
import { overflow } from "./support/open";

// P5-REFINE-11 item 8 and P5-REFINE-12 item 7 (D-414, access-3): an agent with no AI key says so. The empty chat shows
// "<Agent> has no AI model yet" with the viewer's one action and the composer is off; the header's "Can't answer: no AI key" carries
// Give <Agent> the Company key (one click, the header turns Ready), Add a key, or Ask an admin with its optional reason.
// Mocked API, both themes at 1280 and 390 px. (Written by reading: the lane has no browser; the CI Watcher runs it.)

type Fix = Record<string, unknown>;
const GIVE: Fix = { action: "give_key", key: { secret_id: id(60), name: "Company key", scope: "organization" } };
const ASK: Fix = { action: "ask_admin", who_can: { roles: ["organization owner", "organization admin"] } };

async function open(page: Page, scheme: "light" | "dark", fix: Fix, events: "empty" | "some" = "empty") {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  await mockChat(page, events === "empty" ? { ...DEFAULT, events: { [MINE.id]: [] } } : DEFAULT);
  let granted = false;
  const asks: unknown[] = [];
  await page.route("**/v1/**", async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const reply = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (req.method() === "GET" && /^\/v1\/agents\/[^/]+\/answer-readiness$/.test(p)) {
      return reply(granted ? { can_answer: true, reason: null, usable_keys: 1, fix: null } : { can_answer: false, reason: "no_ai_key", usable_keys: 0, fix });
    }
    if (req.method() === "POST" && p === `/v1/secrets/${id(60)}/grants`) {
      granted = true;
      return reply({ items: [{ agent_id: id(11), status: "created" }] });
    }
    if (req.method() === "GET" && /^\/v1\/workspaces\/[^/]+\/requests$/.test(p)) return reply({ items: [] });
    if (req.method() === "POST" && /^\/v1\/workspaces\/[^/]+\/requests$/.test(p)) {
      asks.push(req.postDataJSON());
      return reply({ id: id(61), subject: "ai_key" }, 201);
    }
    return route.fallback();
  });
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(AGENT_ROUTE);
  await page.getByRole("tab", { name: "Chat" }).click();
  await page.waitForLoadState("networkidle");
  return { errors, asks };
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("no AI key: the empty chat says so, the composer is off, the owner gives the Company key in one click", async ({ page }) => {
        const { errors } = await open(page, scheme, GIVE);
        const card = page.getByRole("region", { name: "Daneel has no AI model yet" });
        await expect(card).toBeVisible();
        await expect(page.getByLabel("Message", { exact: true })).toHaveCount(0);
        await expect(page.getByTestId("no-model-bar")).toBeVisible();
        await expect(page.getByTestId("agent-state")).toContainText("Can't answer: no AI key");
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
        // The header's action (its own row under the name and state, at both widths) and the card's are the same one-click
        // grant. The name stays visible and nothing runs under Status and Settings.
        const header = page.locator("header").filter({ has: page.getByTestId("agent-state") });
        const headerGive = header.getByRole("button", { name: "Give Daneel the Company key" });
        await expect(headerGive).toBeVisible();
        await expect(header.getByRole("heading", { name: "Daneel", level: 1 })).toBeVisible();
        const nameBox = (await header.getByRole("heading", { name: "Daneel", level: 1 }).boundingBox())!;
        const stateBox = (await page.getByTestId("agent-state").boundingBox())!;
        const settingsBox = (await header.getByRole("button", { name: "Settings" }).boundingBox())!;
        expect(stateBox.x + stateBox.width, "the state line stays left of Settings").toBeLessThanOrEqual(settingsBox.x + 1);
        expect(nameBox.width, "the name is not squeezed out").toBeGreaterThan(30);
        await headerGive.click();
        // The header leaves "Can't answer" (the shared mock has approvals waiting, so it then reads "Needs you", not Ready).
        await expect(page.getByTestId("agent-state")).not.toContainText("Can't answer");
        await expect(card).toHaveCount(0);
        await expect(page.getByLabel("Message", { exact: true })).toBeVisible();
        expect(errors, "page threw").toEqual([]);
      });

      test("no AI key, not an owner: Ask an admin carries the optional reason", async ({ page }) => {
        const { errors, asks } = await open(page, scheme, ASK);
        const card = page.getByRole("region", { name: "Daneel has no AI model yet" });
        await expect(card).toContainText("Only organization owners and admins can add an AI key.");
        await card.getByLabel(/Why do you need it/).fill("So Daneel can answer in #ops");
        await card.getByRole("button", { name: "Send request" }).click();
        await expect(card.getByText("Asked an admin")).toBeVisible();
        expect(asks).toEqual([{ subject: "ai_key", reason: "So Daneel can answer in #ops" }]);
        expect(errors, "page threw").toEqual([]);
      });

      test("with messages already in the chat the composer is replaced by one line", async ({ page }) => {
        await open(page, scheme, ASK, "some");
        await expect(page.getByTestId("no-model-bar")).toContainText("has no AI model yet");
        await expect(page.getByLabel("Message", { exact: true })).toHaveCount(0);
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
      });
    });
  }
}
