import { expect, test, type Page } from "@playwright/test";

import { AGENT_ROUTE, DEFAULT, MINE, mockChat } from "./support/chat-mock";
import { serious } from "./support/axe";
import { agent, id, mockApi } from "./support/mock-api";
import { overflow } from "./support/open";

// (FIX-249, C-1168) Giving an agent an AI key that already exists: the header's one-click "Give Daneel
// the Company key" gains the quiet "Use another key" menu (the other usable keys by name with their
// scope in words; a key that cannot be used is a disabled row with its reason), and the agent's
// "Add an AI key" dialog opens on the first choice — use a key you already have, or add a new key.
// Mocked API, both themes at 1280 and 390 px, axe clean.

const COMPANY = id(60);
const LAB = id(63);
const OLD = id(64);
const KEYS = [
  { secret_id: COMPANY, name: "Company key", scope: "organization", starter: true, usable: true, reason: null },
  { secret_id: LAB, name: "Lab key", scope: "workspace", starter: false, usable: true, reason: null },
  { secret_id: OLD, name: "Old key", scope: "organization", starter: false, usable: false, reason: "Its service is not allowed here." },
];

async function mock(page: Page, scheme: "light" | "dark") {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  await mockChat(page, { ...DEFAULT, events: { [MINE.id]: [] } });
  const granted = new Set<string>();
  await page.route("**/v1/**", async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const m = req.method();
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (m === "GET" && /\/answer-readiness$/.test(p)) {
      return json(
        granted.size
          ? { can_answer: true, reason: null, usable_keys: granted.size, fix: null }
          : { can_answer: false, reason: "no_ai_key", usable_keys: 0, fix: { action: "give_key", key: { secret_id: COMPANY, name: "Company key", scope: "organization" } } },
      );
    }
    if (m === "GET" && /\/ai-keys$/.test(p)) return json({ items: KEYS.filter((k) => !granted.has(k.secret_id)) });
    // "Add a new key" is today's flow: it needs a service to choose, like the unit tests' mock.
    if (m === "GET" && p === "/v1/llm/providers")
      return json({
        providers: [{ id: "litellm", display_name: "LiteLLM", dialects: [], auth_modes: ["api_key"] }],
        endpoints: [{ id: "aigw", provider: "litellm", display_name: "AI gateway", base_url: "https://x", dialects: [], permitted: true, key_scopes: ["workspace", "organization"] }],
      });
    if (m === "POST" && /^\/v1\/secrets\/[^/]+\/grants$/.test(p)) {
      granted.add(p.split("/")[2]!);
      return json({ items: [{ agent_id: agent.id, status: "created" }] });
    }
    if (m === "GET" && /\/agents\/[^/]+\/grants$/.test(p)) return json({ secrets: [], connections: [], converted_by_app: [] });
    if (m === "GET" && /\/workspaces\/[^/]+\/agents$/.test(p)) return json({ items: [agent], next_cursor: null });
    return route.fallback();
  });
  await page.emulateMedia({ colorScheme: scheme });
  return { errors };
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("the header's Use another key menu gives the second key, and the agent can answer", async ({ page }) => {
        const { errors } = await mock(page, scheme);
        await page.goto(AGENT_ROUTE);
        await page.getByRole("tab", { name: "Chat" }).click();
        await expect(page.getByTestId("agent-state")).toContainText("Can't answer: no AI key");
        const header = page.locator("header").filter({ has: page.getByTestId("agent-state") });
        await expect(header.getByRole("button", { name: "Give Daneel the Company key" })).toBeVisible();
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);

        await header.getByRole("button", { name: "Use another key" }).click();
        // The other usable key by name with its scope in words; the unusable one is a disabled row with its reason.
        const menu = page.getByRole("menu");
        await expect(menu.getByRole("menuitem", { name: "Lab key Workspace key" })).toBeVisible();
        const old = menu.getByRole("menuitem", { name: /Old key/ });
        await expect(old).toContainText("Its service is not allowed here.");
        await expect(old).toHaveAttribute(`data-disabled`, /.*/);
        expect(await serious(page)).toEqual([]);
        await menu.getByRole("menuitem", { name: "Lab key Workspace key" }).click();

        await expect(page.getByTestId("agent-state")).toContainText("Needs you");
        await expect(page.getByRole("button", { name: /Use another key/ })).toHaveCount(0);
        expect(errors, "the page threw").toEqual([]);
      });

      test("the empty chat's card offers the other keys too", async ({ page }) => {
        await mock(page, scheme);
        await page.goto(AGENT_ROUTE);
        await page.getByRole("tab", { name: "Chat" }).click();
        const card = page.getByRole("region", { name: "Daneel has no AI model yet" });
        await expect(card).toBeVisible();
        await expect(page.getByTestId("no-model-bar")).toBeVisible();
        await card.getByRole("button", { name: "Use another key" }).click();
        await page.getByRole("menuitem", { name: "Lab key Workspace key" }).click();
        await expect(page.getByRole("region", { name: "Daneel has no AI model yet" })).toHaveCount(0);
        await expect(page.getByLabel("Message", { exact: true })).toBeVisible();
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });

      test("Add an AI key opens on the first choice: give an existing key, or add a new one", async ({ page }) => {
        await mock(page, scheme);
        await page.goto(`${AGENT_ROUTE}?tab=access`);
        await page.getByRole("button", { name: "Add an AI key" }).click();
        const dialog = page.getByRole("dialog", { name: "Add an AI key" });
        await expect(dialog.getByText("Give Daneel an AI key")).toBeVisible();
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);

        // Use a key you already have: the list, the starter marked, the unusable one disabled with its reason.
        await dialog.getByText("Use a key you already have").click();
        const starter = dialog.locator("label", { hasText: "Company key" });
        await expect(starter).toContainText("Organization key");
        await expect(starter).toContainText("Starter");
        const old = dialog.locator("label", { hasText: "Old key" });
        await expect(old).toContainText("Its service is not allowed here.");
        await expect(old.locator("input")).toBeDisabled();
        expect(await serious(page)).toEqual([]);

        // Picking one and pressing Give <Agent> this key grants it and closes with the toast.
        await dialog.locator("label", { hasText: "Lab key" }).click();
        await dialog.getByRole("button", { name: "Give Daneel this key" }).click();
        await expect(page.getByRole("dialog", { name: "Add an AI key" })).toHaveCount(0);

        // Add a new key is today's flow, unchanged.
        await page.getByRole("button", { name: "Add an AI key" }).click();
        await expect(dialog.getByText("Give Daneel an AI key")).toBeVisible();
        await dialog.getByText("Add a new key").click();
        await expect(dialog.getByText("Which service is this key for?")).toBeVisible();
        expect(await serious(page)).toEqual([]);
      });
    });
  }
}
