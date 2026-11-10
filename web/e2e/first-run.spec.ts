import { expect, test, type Page } from "@playwright/test";

import { ensurePlacement } from "./support/placement";

// P2-E2E-1 (decision D-78): the owner's phase-2 exit bar, driven through the dashboard only.
//   new user signs in -> adds a LiteLLM key -> grants it to the default agent -> says hello -> the answer
//   comes back from the mock LLM (VidaiMock behind LiteLLM) through the gateway, with the REAL claude-code
//   harness in the sandbox (not the fake one).
// Runs against the kind dev stack with the --llm-mock profile (scripts/e2e/first-run.sh). No API seeding,
// no database shortcuts: the stack is brought up by the script, everything else is a click.
//
// The scripted answer: the endpoint `litellm-mock` defaults to the model alias `scn-pong`, which VidaiMock
// (deploy/dev/llm-mock/vidaimock/scenarios/anthropic/pong.*) answers with the fixed text "pong". LiteLLM
// itself is `litellm.aurora-dev-mock.svc.cluster.local`, reached by the gateway at 10.96.100.40.
const MOCK_KEY = "sk-aurora-mock-dev"; // dev-only dummy master key (docs/operator/llm-endpoints.md)
const MOCK_REPLY = "pong";
const LITELLM_HOST = /litellm\.aurora-dev-mock\.svc\.cluster\.local|10\.96\.100\.40/;

// Phase 1 (RFE-004): all sign-in is OIDC; the dev IdP signs a user in when the authorize request carries
// `login_hint=<email>`, which the API forwards from `/v1/auth/oidc/dev/start?login_hint=`. The "Development
// sign-in" link on the login page is what a person clicks; the hint only picks the (fresh) user.
async function signInAs(page: Page, email: string) {
  await page.goto("/auth/login");
  await expect(page.getByRole("link", { name: "Development sign-in" })).toBeVisible();
  await page.goto(`/v1/auth/oidc/dev/start?login_hint=${encodeURIComponent(email)}&return_to=%2F`);
}

test("first run: sign in, add a LiteLLM key, grant it, say hello, get the mock's answer", async ({ page, browser, baseURL }) => {
  test.setTimeout(8 * 60_000);
  // A fresh instance admits only its admins until a placement is saved (P4-ONB-1); the journey
  // itself stays "a new user signs in and lands in a workspace".
  await ensurePlacement(browser, baseURL!);
  const email = `first-run-${Date.now().toString(36)}@example.test`;

  // 1. A new user signs in and lands in their new organization/workspace.
  await signInAs(page, email);
  const nav = page.getByRole("navigation", { name: "Workspace" });
  await expect(nav).toBeVisible({ timeout: 30_000 });
  await expect(page).toHaveURL(/\/o\/[^/]+\/w\/[^/]+$/);
  const base = new URL(page.url()).pathname;

  // The checklist starts with the agent (created at first sign-in) done and the rest open.
  const checklist = page.getByRole("region", { name: "Getting started" });
  await expect(checklist.getByText("1 of 3 done")).toBeVisible();

  // 2. Credentials -> LLM keys -> add the key for the LiteLLM endpoint.
  await nav.getByRole("link", { name: "Credentials" }).click();
  await page.getByRole("tab", { name: "LLM keys" }).click();
  const form = page.getByRole("form", { name: "New LLM key" });
  await form.getByLabel("Name").fill("Mock LLM");
  await form.getByLabel("Provider endpoint").selectOption({ label: "LiteLLM (VidaiMock, dev only) (litellm)" });
  await form.getByLabel("API key").fill(MOCK_KEY);
  await form.getByRole("button", { name: "Add LLM key" }).click();
  await expect(page.getByText("Saved Mock LLM")).toBeVisible({ timeout: 60_000 });
  await expect(page.getByRole("list", { name: "LLM keys" }).getByText("Mock LLM")).toBeVisible();

  // 3. Grant it to the default agent (Access tab).
  await page.goto(`${base}/agents`);
  await page.getByRole("link", { name: /Daneel/ }).first().click();
  await page.getByRole("tab", { name: "Access" }).click();
  // The checkbox is controlled by the grants query: it flips when the server has the grant, so click and assert.
  await page.getByRole("checkbox", { name: "Grant Mock LLM" }).click();
  await expect(page.getByRole("checkbox", { name: "Grant Mock LLM" })).toBeChecked();
  await expect(page.getByRole("list", { name: "Effective credentials" }).getByText("Mock LLM")).toBeVisible();

  // 4. Chat: say hello, the reply is the mock's scripted answer.
  await page.getByRole("tab", { name: "Chat" }).click();
  await page.getByLabel("Message", { exact: true }).fill("hello");
  await page.getByRole("button", { name: "Send" }).click();
  // (A cold start boots a sandbox Pod and the claude-code CLI: allow minutes. A failed turn shows an alert,
  // which fails the test at once with the platform's message instead of waiting out the timeout.)
  const reply = page.getByText(MOCK_REPLY, { exact: true });
  const failed = page.getByRole("alert").filter({ hasText: "Something went wrong" });
  await expect
    .poll(
      async () => {
        if (await reply.first().isVisible()) return "reply";
        if (await failed.first().isVisible()) return `turn failed: ${await failed.first().innerText()}`;
        return "waiting for the reply";
      },
      { timeout: 4 * 60_000, intervals: [1_000, 2_000, 5_000] },
    )
    .toBe("reply");

  // 5. Evidence: an allowed LLM call to the LiteLLM host through the gateway, and the checklist is done.
  await nav.getByRole("link", { name: "Activity" }).click();
  await expect(page).toHaveURL(/\/activity$/);
  // The gateway batches its request log (flush every 5 s) and the pages do not poll: reload until it shows.
  const row = page.getByRole("row").filter({ hasText: LITELLM_HOST }).first();
  await expect(async () => {
    await page.goto(`${base}/activity`);
    await expect(row).toBeVisible({ timeout: 4_000 });
  }).toPass({ timeout: 60_000, intervals: [1_000] });
  await expect(row.getByText("allow", { exact: true })).toBeVisible();
  await nav.getByRole("link", { name: "Overview" }).click();
  await expect(page.getByRole("region", { name: "Getting started" }).getByText("3 of 3 done")).toBeVisible();
});
