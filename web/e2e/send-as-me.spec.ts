import { expect, test, type Page, type Route } from "@playwright/test";

import { mockApi } from "./support/mock-api";
import { overflow, titled, type Scheme } from "./support/open";
import { serious } from "./support/files";

// P4-UTOK (docs/14 §Account › Send as me, C-853..C-856): the token list, the Add token dialog (masked input, help text), the
// grant dialog and the owner's "Posts as you" with the full text. Zero serious/critical axe violations in both themes at
// 1280 and 390 px, no horizontal scroll at 390 px.

const T = "2026-01-01T00:00:00Z";
const TOKENS = [
  {
    id: "00000000-0000-4000-8000-0000000000a1", integration: { id: "00000000-0000-4000-8000-0000000000b1", type: "mattermost" },
    label: "a rather long descriptive name for my work token", platform_user_label: "ada.lovelace", status: "active",
    agents: [{ id: "00000000-0000-4000-8000-0000000000c1", name: "Daneel" }], created_at: T, last_used_at: "2026-01-02T10:00:00Z",
  },
  {
    id: "00000000-0000-4000-8000-0000000000a2", integration: { id: "00000000-0000-4000-8000-0000000000b1", type: "mattermost" },
    label: null, platform_user_label: "ada", status: "invalid", agents: [], created_at: T, last_used_at: null,
  },
];
const LINK = {
  id: "00000000-0000-4000-8000-0000000000d1", integration_id: "00000000-0000-4000-8000-0000000000b1", connector_id: "mattermost",
  tenant_label: "Acme Chat", platform_user_ref: "U123", user: { id: "u1", display_name: "Ada" }, method: "admin", created_at: T,
};
const SEND = {
  id: "00000000-0000-4000-8000-0000000000e1", workspace_id: "00000000-0000-4000-8000-0000000000f1",
  agent_id: "00000000-0000-4000-8000-0000000000c1", token_id: TOKENS[0]!.id,
  summary: "Post as Ada in General (96 characters)",
  text: "Hello team,\nthe quarterly numbers are ready for review in the shared folder. Ping me with questions.",
  status: "pending", error: null, expires_at: "2099-01-01T00:00:00Z", created_at: T,
};

async function mockSend(page: Page, over: { tokens?: unknown[]; sends?: unknown[] } = {}) {
  await mockApi(page);
  const reply = (route: Route, body: unknown, status = 200) =>
    route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
  await page.route("**/v1/me/channel-links", (route) => reply(route, { items: [LINK] }));
  await page.route("**/v1/me/dm-defaults", (route) => reply(route, { items: [] }));
  await page.route("**/v1/me/user-tokens", (route) => reply(route, { tokens: over.tokens ?? TOKENS }));
  await page.route("**/v1/me/user-token-approvals**", (route) => reply(route, { items: over.sends ?? [SEND] }));
}

async function open(page: Page, path: string, scheme: Scheme) {
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await titled(page);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("Account › Send as me: tokens, Add token and Grant agents", async ({ page }) => {
        await mockSend(page);
        await open(page, "/account/channels", scheme);
        await expect(page.getByRole("heading", { name: "Send as me", level: 2 })).toBeVisible();
        const list = page.getByRole("list", { name: "Send as me tokens" });
        await expect(list.getByText("ada.lovelace")).toBeVisible();
        await expect(list.getByText("invalid", { exact: true })).toBeVisible();
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
        // Add token: a masked input and the help text.
        await page.getByRole("button", { name: "Add token" }).first().click();
        const add = page.getByRole("dialog", { name: "Add token" });
        await expect(add).toBeVisible();
        await expect(add.getByLabel("Personal access token", { exact: true })).toHaveAttribute("type", "password");
        await expect(add.getByText(/Profile › Security › Personal Access Tokens/)).toBeVisible();
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
        await add.getByRole("button", { name: "Cancel" }).click();
        // Grant agents.
        await page.getByRole("button", { name: /Grant agents for a rather long/ }).click();
        const grant = page.getByRole("dialog", { name: "Grant agents" });
        await expect(grant).toBeVisible();
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
      });

      test("Account › Send as me: the empty state", async ({ page }) => {
        await mockSend(page, { tokens: [] });
        await open(page, "/account/channels", scheme);
        await expect(page.getByText("Agents post as themselves unless you add a token.")).toBeVisible();
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
      });

      test("Account › Send as me: Revoke asks first", async ({ page }) => {
        await mockSend(page);
        await open(page, "/account/channels", scheme);
        await page.getByRole("button", { name: "Revoke ada" }).click();
        const dialog = page.getByRole("dialog", { name: "Revoke token" });
        await expect(dialog).toContainText("Agents stop posting as you and pending posts are cancelled.");
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
      });

      test("Approvals › Posts as you: the full text and the decision controls", async ({ page }) => {
        await mockSend(page);
        await open(page, "/o/acme/w/main/approvals", scheme);
        const section = page.getByRole("region", { name: "Posts as you" });
        await expect(section).toBeVisible();
        await expect(section).toContainText("the quarterly numbers are ready for review");
        await expect(section.getByRole("button", { name: "Post as me" })).toBeVisible();
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
      });
    });
  }
}
