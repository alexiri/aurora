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
  // (P4-ONB-2, D-141) A first sign-in lands on the workspace's Welcome page with the getting-started guide.
  await expect(page).toHaveURL(/\/o\/[^/]+\/w\/[^/]+\/welcome$/);
  const base = new URL(page.url()).pathname.replace(/\/welcome$/, "");

  // The guide ticks from real state: the agent (created at first sign-in) has no key yet, so nothing is done.
  const guide = page.getByRole("region", { name: "Get Daneel talking" });
  await expect(guide.getByText("0 of 2 done")).toBeVisible();

  // 2. Step 1 opens the LLM key form in place: add the key for the LiteLLM endpoint.
  const form = guide.getByRole("form", { name: "New LLM key" });
  await form.getByLabel("Name").fill("Mock LLM");
  // By the endpoint's id (deploy/kind/values-llm-mock.yaml): the label carries hints that change ("— Works with
  // Claude Code" since P4-LLM-6), and a label that no longer matches waits out the whole test timeout.
  await form.getByLabel("Provider").selectOption("litellm-mock");
  await form.getByLabel("API key", { exact: true }).fill(MOCK_KEY);
  // (P5-UX-4, C-988) The form's "Who can use this key" has Daneel ticked (the workspace's only agent), so the key is saved
  // with its access in one request. Key and access are done from the server's state (the mock endpoint has a default
  // model: no model step).
  await expect(form.getByRole("checkbox", { name: "Daneel can use this key" })).toBeChecked();
  await form.getByRole("button", { name: "Add LLM key" }).click();
  await expect(guide.getByText("1 of 2 done")).toBeVisible({ timeout: 60_000 });

  // 4. Step "Say hello" opens Chat with a suggested message; send it, the reply is the mock's scripted answer.
  await guide.getByRole("link", { name: "Say hello to Daneel" }).click();
  await expect(page.getByLabel("Message", { exact: true })).toHaveValue(/Hello/);
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
  // (P5-REFINE-16) The model call is one sentence ("<agent> answered you"); its raw request is under Details.
  const row = page.getByTestId("activity-sentences").getByRole("listitem").filter({ hasText: "answered you" }).first();
  await expect(async () => {
    await page.goto(`${base}/activity`);
    await expect(row).toBeVisible({ timeout: 4_000 });
  }).toPass({ timeout: 60_000, intervals: [1_000] });
  await row.getByRole("button", { name: "Details" }).click();
  await expect(page.getByTestId("activity-raw")).toContainText(LITELLM_HOST);
  await expect(page.getByTestId("activity-raw")).toContainText("allow");
  await nav.getByRole("link", { name: "Overview" }).click();
  // (P5-POLISH-7) The Overview's sentence comes from real state: the agent has a key and has answered. (The guide line
  // shows on Overview only once Welcome has been seen, never on both pages.)
  await expect(page.getByText("Daneel is ready. Nothing needs you.")).toBeVisible({ timeout: 30_000 });
});
