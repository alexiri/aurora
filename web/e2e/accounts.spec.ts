import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";

import { agent, id, mockApi, now, ws } from "./support/mock-api";
import { overflow } from "./support/open";
import { serious } from "./support/axe";

// P5-UX-4 (C-917, C-925, C-938, C-980; docs/14 §Agents, accounts and access): the one list of accounts on the
// workspace's Accounts tab (ws-accounts) and on the agent's Access tab (agent-access): rows, the access choice as one
// click and one save, two agents with different access and no limit, a one-agent workspace's AI key saved with its access
// in one request. Both themes at 1280 px and 390 px, no horizontal scroll, axe with no rule disabled.
// Screenshots stay outside the repo.

const SHOTS = "/tmp/aurora-screens/p5-ux-4";
mkdirSync(SHOTS, { recursive: true });

const C_AWS = id(50);
const second = { id: id(12), name: "Giskard", handle: "giskard" };
const who = (a: { id: string; name: string; handle: string }) => ({ agent: { id: a.id, name: a.name, handle: a.handle }, workspace: { id: ws.id, name: ws.name } });
const can = { give_access: true, change_access: true, manage: true };

/** A workspace whose one AWS account (the walkthrough's) is read only with its writes set to ask, kept in the mock. */
async function mockAws(page: Page, agents: "one" | "two") {
  const state = { daneel: "custom" as "custom" | "read" | "ask", puts: [] as unknown[] };
  await mockApi(page);
  await page.route("**/v1/**", async (route) => {
    const p = new URL(route.request().url()).pathname;
    const m = route.request().method();
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (p === `/v1/workspaces/${ws.id}/agents`) return json({ items: agents === "one" ? [agent] : [agent, { ...agent, id: second.id, handle: second.handle, name: second.name, is_default: false }], next_cursor: null });
    if (p === `/v1/workspaces/${ws.id}/accounts`) {
      // The walkthrough: writes set to ask on a read-only account. A saved choice that is not a level reads as custom.
      const daneel =
        state.daneel === "custom"
          ? { ...who(agent), level: "custom", custom: { differs: 3, base: "read" } }
          : { ...who(agent), level: state.daneel };
      return json({
        items: [
          {
            ref: { type: "connection", id: C_AWS }, kind: "app", name: "platform-prod", scope: "workspace", levels: ["read", "ask", "act"],
            app: { app_id: "aws-keys", name: "AWS", custom_only: false }, health: { state: "not_used", last_used_at: now },
            agents: agents === "one" ? [daneel] : [daneel, { ...who(second), level: "act" }], can,
          },
        ],
        next_cursor: null,
      });
    }
    if (p === `/v1/agents/${agent.id}/grants/connections/${C_AWS}` && m === "PUT") {
      const body = route.request().postDataJSON() as { level?: string };
      state.puts.push(body);
      state.daneel = body.level as "read" | "ask";
      return json({});
    }
    if (p === `/v1/agents/${agent.id}/grants`) return json({ secrets: [], connections: [], converted_by_app: [] });
    if (p === "/v1/llm/providers")
      return json({
        providers: [{ id: "litellm", display_name: "LiteLLM", dialects: [], auth_modes: ["api_key"] }],
        endpoints: [{ id: "aigw", provider: "litellm", display_name: "AI gateway", base_url: "https://ai.example.test", dialects: [], permitted: true, key_scopes: ["workspace"] }],
      });
    return route.fallback();
  });
  return state;
}

async function check(page: Page, name: string, scheme: string, width: number) {
  expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
  await page.screenshot({ path: `${SHOTS}/${name}-${scheme}-${width}.png` });
  expect(await serious(page)).toEqual([]);
}

async function open(page: Page, path: string, scheme: "light" | "dark") {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, `${path} threw`).toEqual([]);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("ws-accounts: one list of apps, websites and AI keys with who can use each", async ({ page }) => {
        await mockApi(page);
        await open(page, "/o/acme/w/main/apps?tab=accounts", scheme);
        const list = page.getByRole("list", { name: "Accounts" });
        await expect(list.getByRole("listitem", { name: "acme-bot" }).getByText("App")).toBeVisible();
        await expect(list.getByRole("listitem", { name: "Weather API" }).getByText("Website")).toBeVisible();
        await expect(list.getByRole("listitem", { name: "Team key" }).getByText("AI key")).toBeVisible();
        await expect(list.getByRole("listitem", { name: "Weather API" }).getByText("No agent can use this")).toBeVisible();
        await expect(list.getByRole("listitem", { name: "Product team" }).getByText(/The sign-in to .* expired/)).toBeVisible();
        // No rule id, host pattern or "secret" on a row.
        await expect(list).not.toContainText(/secret|host pattern|rule/i);
        await check(page, "ws-accounts", scheme, width);
      });

      test("agent-access: the agent's accounts first with their access, the others collapsed", async ({ page }) => {
        await mockApi(page);
        await open(page, "/o/acme/w/main/agents/daneel?tab=access", scheme);
        const used = page.getByRole("list", { name: "Accounts Daneel can use" });
        await expect(used.getByRole("listitem", { name: "acme-bot" }).getByRole("radio", { name: /Read only/ })).toBeChecked();
        await expect(used.getByText("Daneel thinks with claude-sonnet through this key.")).toBeVisible();
        await expect(page.getByText("Not given to Daneel (1)")).toBeVisible();
        await check(page, "agent-access", scheme, width);
      });

      test("a read-only AWS account with writes set to ask: choosing Ask before changes is one click and one save", async ({ page }) => {
        const state = await mockAws(page, "one");
        await open(page, "/o/acme/w/main/agents/daneel?tab=access", scheme);
        const row = page.getByRole("listitem", { name: "platform-prod" });
        // The saved choice is not one of the three: nothing is chosen and the row says how it differs.
        await expect(row.getByText(/3 actions differ/)).toBeVisible();
        await expect(row.getByRole("radio", { name: /Ask before changes/ })).not.toBeChecked();
        await check(page, "walkthrough-custom", scheme, width);
        await row.getByText("Ask before changes", { exact: true }).click(); // the card is the label of the (visually hidden) radio
        await expect(row.getByRole("radio", { name: /Ask before changes/ })).toBeChecked();
        expect(state.puts).toEqual([{ level: "ask" }]);
        await check(page, "walkthrough-ask", scheme, width);
      });

      test("two agents use one account with different access, and no limit is shown", async ({ page }) => {
        await mockAws(page, "two");
        await open(page, "/o/acme/w/main/apps?tab=accounts", scheme);
        const row = page.getByRole("listitem", { name: "platform-prod" });
        await expect(row.getByRole("button", { name: "Daneel · Custom: change access" })).toBeVisible();
        await expect(row.getByRole("button", { name: "Giskard · Change without asking: change access" })).toBeVisible();
        await expect(row).not.toContainText(/limit/i);
        await check(page, "two-agents", scheme, width);
      });

      test("a one-agent workspace: the AI key form has the agent ticked and saves it with its access in one request", async ({ page }) => {
        const posted: { grant?: { agent_ids: string[] } }[] = [];
        await mockAws(page, "one");
        await open(page, "/o/acme/w/main/apps?tab=accounts", scheme);
        await page.route(`**/v1/workspaces/${ws.id}/secrets`, async (route) => {
          if (route.request().method() !== "POST") return route.fallback();
          posted.push(route.request().postDataJSON());
          return route.fulfill({
            status: 201, contentType: "application/json",
            body: JSON.stringify({ id: id(60), scope: "workspace", kind: "llm", name: "Team key", status: "active", host_pattern: "ai.example.test", path_pattern: null, injection: { kind: "bearer" }, value_source: "value", value_version: 1, llm: { endpoint_id: "aigw", provider: "litellm", auth_mode: "api_key" }, origin: null, created_at: now, updated_at: now }),
          });
        });
        await page.getByRole("button", { name: "Add", exact: true }).click();
        await page.getByRole("menuitem", { name: "Add an AI key" }).click();
        const form = page.getByRole("form", { name: "New LLM key" });
        await form.getByLabel("Name").fill("Team key");
        await form.getByLabel("Provider endpoint").selectOption("aigw");
        await form.getByLabel("API key", { exact: true }).fill("sk-not-a-real-key");
        await expect(form.getByRole("checkbox", { name: "Daneel can use this key" })).toBeChecked();
        await form.getByRole("button", { name: "Add LLM key" }).click();
        await expect(page.getByText("Saved Team key")).toBeVisible();
        // No "Give agents access" second step.
        await expect(page.getByRole("region", { name: "Give agents access" })).toHaveCount(0);
        expect(posted).toHaveLength(1);
        expect(posted[0].grant).toEqual({ agent_ids: [agent.id] });
        await check(page, "ai-key-one-request", scheme, width);
      });

      test("FIX-163: an account whose app instance was deleted is not offered Turn on and says who can add the instance again", async ({ page }) => {
        await mockApi(page);
        await page.route("**/v1/**", async (route) => {
          const p = new URL(route.request().url()).pathname;
          const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
          if (p === `/v1/workspaces/${ws.id}/accounts`) {
            const row = (n: number, name: string, extra: Record<string, unknown>) => ({
              ref: { type: "connection", id: id(n) }, kind: "app", name, scope: "workspace", levels: ["read", "ask", "act"],
              app: { app_id: "aws-keys", name: "AWS", custom_only: false }, health: { state: "disabled" }, agents: [{ ...who(agent), level: "read" }], can, ...extra,
            });
            return json({ items: [row(51, "desk", { status_reason: "instance_deleted" }), row(52, "staging", {})], next_cursor: null });
          }
          return route.fallback();
        });
        await page.emulateMedia({ colorScheme: scheme });
        await page.goto("/o/acme/w/main/apps?tab=accounts");
        const gone = page.getByRole("listitem", { name: "desk" });
        await expect(gone.getByText("Turned off: the instance of this app was removed")).toBeVisible();
        await expect(gone.getByText(/Only organization owners and admins can add the app's instance again\./)).toBeVisible();
        await gone.getByRole("button", { name: "More for desk" }).click();
        await expect(page.getByRole("menuitem", { name: "Rename" })).toBeVisible();
        await expect(page.getByRole("menuitem", { name: "Turn on" })).toHaveCount(0);
        await page.keyboard.press("Escape");
        // Another reason for being off keeps Turn on.
        await page.getByRole("listitem", { name: "staging" }).getByRole("button", { name: "More for staging" }).click();
        await expect(page.getByRole("menuitem", { name: "Turn on" })).toBeVisible();
        await page.keyboard.press("Escape");
        await check(page, "instance-deleted", scheme, width);
      });
    });
  }
}
