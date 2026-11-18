import { expect, test, type Page } from "@playwright/test";

import { openJourney } from "./support/journey";
import { id, mockApi, readinessEmpty, readinessReady, ws } from "./support/mock-api";
import { overflow } from "./support/open";
import { serious } from "./support/axe";

// P5-UX-7 (C-927, C-986; docs/14 §Agents, accounts and access §8 and §9): Welcome's two-step guide and the agent's one
// summary. A person in an organization that provides an AI key has one step left and a summary that says the agent can't
// use any app yet; a person with no key adds one and goes straight to Say hello; after giving the agent an app with Ask
// before changes, Welcome and the Access tab both say that it asks before changing anything there. Both themes at 1280 px
// and 390 px, no horizontal scroll, axe with no rule disabled.

async function check(page: Page) {
  expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
  expect(await serious(page)).toEqual([]);
}

/** Welcome with the mock; `noAccounts` serves an empty accounts list (a new agent) and `noKey` a workspace without an AI key. */
async function openWelcome(page: Page, scheme: "light" | "dark", o: { noAccounts?: boolean; noKey?: boolean } = {}) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  await page.emulateMedia({ colorScheme: scheme });
  let keyAdded = false;
  await page.route("**/v1/**", async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (o.noAccounts && req.method() === "GET" && p === `/v1/workspaces/${ws.id}/accounts`) return json({ items: [], next_cursor: null });
    if (o.noKey && /^\/v1\/agents\/[^/]+\/model-readiness$/.test(p)) return json(keyAdded ? readinessReady : readinessEmpty);
    if (o.noKey && p === `/v1/workspaces/${ws.id}/secrets` && req.method() === "POST") {
      keyAdded = true;
      return json({ id: id(310), name: "Mock LLM", kind: "llm", scope: "workspace", status: "active", created_at: new Date().toISOString() }, 201);
    }
    if (o.noKey && p === "/v1/llm/providers") {
      return json({
        providers: [{ id: "litellm", display_name: "LiteLLM", auth_modes: ["bearer"] }],
        endpoints: [{ id: "litellm", provider: "litellm", display_name: "AIGW", permitted: true, key_scopes: [] }],
      });
    }
    return route.fallback();
  });
  await page.goto("/o/acme/w/main/welcome");
  await page.getByRole("heading", { name: "Welcome to Main workspace" }).waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, "Welcome threw").toEqual([]);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("an organization that provides an AI key: one step left, and the agent can't use any app yet", async ({ page }) => {
        await openWelcome(page, scheme, { noAccounts: true });
        await expect(page.getByText("1 of 2 done")).toBeVisible();
        await expect(page.getByText("Your organization provides an AI key: AIGW")).toBeVisible();
        const summary = page.getByRole("region", { name: "Daneel is your agent" });
        await expect(summary).toContainText("Daneel can't use any app yet. It can reach any website.");
        await expect(page.getByRole("region", { name: "How it stays safe" })).toContainText("puts anything that needs your say in Approvals.");
        await expect(page.getByRole("link", { name: "Say hello to Daneel" })).toBeVisible();
        await check(page);
      });

      test("no AI key: adding one goes straight to Say hello", async ({ page }) => {
        await openWelcome(page, scheme, { noAccounts: true, noKey: true });
        await expect(page.getByText("0 of 2 done")).toBeVisible();
        const form = page.getByRole("form", { name: "New LLM key" });
        await expect(form.getByRole("checkbox", { name: "Daneel can use this key" })).toBeChecked();
        await check(page);
        await form.getByLabel("Name").fill("Mock LLM");
        await form.getByLabel("Provider endpoint").selectOption({ index: 1 });
        await form.getByLabel("API key").fill("not-a-real-key");
        await form.getByRole("button", { name: "Add LLM key" }).click();
        await expect(page.getByText("1 of 2 done")).toBeVisible();
        await expect(page.getByRole("link", { name: "Say hello to Daneel" })).toBeVisible();
      });

      test("giving the agent an app with Ask before changes: Welcome and the Access tab both say it asks", async ({ page }) => {
        const saved = await openJourney(page, "/o/acme/w/main/welcome", scheme);
        await page.getByRole("link", { name: "Give Daneel an app" }).click();
        const dialog = page.getByTestId("ws-connect-journey");
        await expect(page.getByRole("dialog", { name: "Give Daneel an app" })).toBeVisible();
        await dialog.getByRole("button", { name: /^GitHub/ }).click();
        await dialog.getByText("Ask before changes", { exact: true }).click();
        await dialog.getByRole("button", { name: "Next" }).click();
        await dialog.getByLabel("Personal access token", { exact: true }).fill("not-a-real-token");
        await dialog.getByRole("button", { name: "Connect", exact: true }).click();
        await expect(dialog.getByText("Daneel can now read GitHub, and asks you before each change.")).toBeVisible();
        expect(saved[0]!.body).toMatchObject({ grant: { level: "ask" } });
        await dialog.getByRole("button", { name: "Done" }).click();

        // The Access tab (where the journey ran), above the accounts list.
        const box = page.getByRole("region", { name: "What Daneel can do" });
        await expect(box).toContainText("It asks you before changing anything in GitHub (acme-prod).");
        await check(page);

        // Welcome says the same sentence.
        await page.goto("/o/acme/w/main/welcome");
        await expect(page.getByRole("region", { name: "Daneel is your agent" })).toContainText("It asks you before changing anything in GitHub (acme-prod).");
      });
    });
  }
}
