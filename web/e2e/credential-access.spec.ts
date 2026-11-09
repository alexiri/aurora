import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";

import { agent, id, mockApi, now } from "./support/mock-api";
import { overflow } from "./support/open";

// FIX-021 (D-136, D-137; docs/14 §Credentials, §Agent page › Access): the "Give agents access" step after creating
// an LLM key, the row Access dialog, and the Model readiness section. Both themes at 1280 px and 390 px, no
// horizontal scroll, axe with no rule disabled and nothing excluded. Screenshots stay outside the repo.

const SHOTS = "/tmp/aurora-screens/fix-021";
mkdirSync(SHOTS, { recursive: true });

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
}

const second = { ...agent, id: id(12), handle: "giskard", name: "Giskard", is_default: false };
const llmKey = {
  id: id(60), scope: "workspace", kind: "llm", name: "Team key", status: "active", host_pattern: "ai.example.test", path_pattern: null,
  injection: { kind: "bearer" }, value_source: "value", value_version: 1, llm: { endpoint_id: "aigw", provider: "litellm", auth_mode: "api_key" },
  origin: { status: "verified", method: "endpoint", value_version: 1 }, created_at: now, updated_at: now,
};
const genericKey = { ...llmKey, id: id(61), kind: "generic", name: "GitHub token", host_pattern: "api.github.com", llm: null, origin: null };
const grantView = (a: typeof agent) => ({ id: id(70), agent_id: a.id, agent_name: a.name, workspace_id: a.workspace_id, workspace_name: "Main workspace", secret_id: llmKey.id, created_at: now });

/** Extra routes on top of the shared mock (a later `page.route` wins; `fallback` hands over to the shared one). */
async function mock(page: Page, scheme: "light" | "dark") {
  await mockApi(page);
  await page.emulateMedia({ colorScheme: scheme });
  await page.route("**/v1/**", async (route) => {
    const p = new URL(route.request().url()).pathname;
    const m = route.request().method();
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (p === "/v1/llm/providers")
      return json({
        providers: [{ id: "litellm", display_name: "LiteLLM", dialects: [], auth_modes: ["api_key"] }],
        endpoints: [{ id: "aigw", provider: "litellm", display_name: "AI gateway", base_url: "https://ai.example.test", dialects: [], permitted: true, key_scopes: ["workspace", "organization"] }],
      });
    if (p === `/v1/workspaces/${agent.workspace_id}/secrets`) return m === "POST" ? json(llmKey, 201) : json({ items: [llmKey, genericKey] });
    if (p === `/v1/workspaces/${agent.workspace_id}/agents`) return json({ items: [agent, second], next_cursor: null });
    if (p === `/v1/secrets/${llmKey.id}/grants` || p === `/v1/secrets/${genericKey.id}/grants`)
      return m === "POST" ? json({ items: [{ agent_id: agent.id, grant: {}, status: "created" }] }) : json({ items: [grantView(agent)] });
    if (p === `/v1/agents/${agent.id}/model-readiness`)
      return json({
        harness: "claude-code", override_dropped: false, default: null,
        keys: [
          { secret_id: llmKey.id, name: "Team key", scope: "workspace", endpoint_id: "aigw", provider: "litellm", usable: true, reason: null },
          { secret_id: id(62), name: "Corporate key", scope: "organization", endpoint_id: "anthropic", provider: "anthropic", usable: false, reason: "origin_unverified" },
        ],
      });
    if (p === `/v1/agents/${agent.id}/grants`) return json({ secrets: [], connections: [] });
    if (p === `/v1/agents/${agent.id}/effective`)
      return json({ credentials: [{ id: llmKey.id, name: "Team key", kind: "llm", scope: "workspace", selected: true, selected_by: [], depends_on_speaker: false }] });
    return route.fallback();
  });
}

async function open(page: Page, path: string, scheme: "light" | "dark") {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mock(page, scheme);
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, `${path} threw`).toEqual([]);
}

async function check(page: Page, name: string, scheme: string, width: number) {
  expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
  await page.screenshot({ path: `${SHOTS}/${name}-${scheme}-${width}.png` });
  expect(await serious(page)).toEqual([]);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("?tab=llm opens the LLM keys tab; creating a key shows the Give agents access step", async ({ page }) => {
        await open(page, "/o/acme/w/main/credentials?tab=llm", scheme);
        await expect(page.getByRole("tab", { name: "LLM keys" })).toHaveAttribute("aria-selected", "true");
        await page.getByLabel("Name").fill("Team key");
        await page.getByLabel("Provider endpoint").selectOption("aigw");
        await page.getByLabel("API key").fill("sk-not-a-real-key");
        await page.getByRole("button", { name: "Add LLM key" }).click();
        const step = page.getByRole("region", { name: "Give agents access" });
        await expect(step).toBeVisible();
        await expect(step.getByRole("checkbox")).toHaveCount(2);
        // Nothing is preselected; the agent that already has the key (the mock's grant list) shows as such.
        await expect(step.getByRole("checkbox", { name: "Give Giskard access" })).not.toBeChecked();
        await expect(step.getByRole("checkbox", { name: "Give Daneel access" })).toBeDisabled();
        await expect(step.getByText("Has access")).toBeVisible();
        await check(page, "give-access-step", scheme, width);
        await step.getByRole("checkbox", { name: "Give Giskard access" }).check();
        await step.getByRole("button", { name: "Give access" }).click();
        await expect(step.getByText("Access given to 1 agent.")).toBeVisible();
        await check(page, "give-access-done", scheme, width);
      });

      test("the row Access dialog lists, removes and adds", async ({ page }) => {
        await open(page, "/o/acme/w/main/credentials?tab=llm", scheme);
        await page.getByRole("button", { name: "Access to Team key" }).click();
        const dlg = page.getByRole("dialog", { name: "Access to Team key" });
        await expect(dlg.getByRole("button", { name: "Remove access for Daneel" })).toBeVisible();
        await expect(dlg.getByRole("checkbox", { name: "Give Giskard access" })).toBeVisible();
        await check(page, "access-dialog", scheme, width);
      });

      test("an unknown ?tab= falls back to Secrets", async ({ page }) => {
        await open(page, "/o/acme/w/main/credentials?tab=nope", scheme);
        await expect(page.getByRole("tab", { name: "Secrets" })).toHaveAttribute("aria-selected", "true");
        await page.getByRole("tab", { name: "Connections" }).click();
        await expect(page).toHaveURL(/\?tab=connections$/);
      });

      test("the agent Access tab shows Model readiness, addressed by ?tab=access", async ({ page }) => {
        await open(page, "/o/acme/w/main/agents/daneel?tab=access", scheme);
        await expect(page.getByRole("tab", { name: "Access" })).toHaveAttribute("aria-selected", "true");
        const section = page.getByRole("region", { name: "Model readiness" });
        await expect(section.getByText("Ready")).toBeVisible();
        await expect(section.getByText("Not usable")).toBeVisible();
        await expect(section.getByRole("link", { name: "Re-verify the key." })).toBeVisible();
        await expect(section.getByText(/No model chosen/)).toBeVisible();
        await expect(page.getByText("(LLM key, workspace)")).toBeVisible();
        await check(page, "model-readiness", scheme, width);
      });
    });
  }
}
