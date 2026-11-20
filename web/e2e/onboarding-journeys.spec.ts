import { expect, test, type Browser, type Page } from "@playwright/test";

import { PLACEMENT_ADMIN } from "./support/placement";

// P4-ONB-2 (D-141): the three onboarding journeys on the kind dev stack with the --llm-mock profile
// (scripts/e2e/first-run.sh -- e2e/onboarding-journeys.spec.ts). Dashboard only, except where a step says it
// changes the instance's saved placement through the API (the journeys share one instance).
//   1. a fresh instance: the admin walks the wizard to a working chat;
//   2. a new user with a starter grant: Welcome -> first reply, with no LLM step;
//   3. a new user without one: the org key is visible but not granted, so the guide walks access -> reply
//      (the full key -> access -> reply walk, no org key at all, is first-run.spec.ts on a personal_org stack).
// The mock: endpoint `litellm-mock` defaults to the model alias `scn-pong`, which VidaiMock answers with "pong".
const MOCK_KEY = "sk-aurora-mock-dev"; // dev-only dummy master key (docs/operator/llm-endpoints.md)
const MOCK_REPLY = "pong";
// The endpoint is chosen by its id (deploy/kind/values-llm-mock.yaml), not its label: the label carries hints that
// change ("— Works with Claude Code" since P4-LLM-6), and a stale label waits out the whole test timeout.
const ENDPOINT_ID = "litellm-mock";

test.describe.configure({ mode: "serial" });

async function signInAs(page: Page, email: string) {
  await page.goto("/auth/login");
  await page.goto(`/v1/auth/oidc/dev/start?login_hint=${encodeURIComponent(email)}&return_to=%2F`);
}

/** Sends the suggested message from the Chat the guide opened and waits for the mock's answer. */
async function chatGetsAnswer(page: Page) {
  await expect(page.getByLabel("Message", { exact: true })).toHaveValue(/Hello/);
  await page.getByRole("button", { name: "Send" }).click();
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
}

/** The admin API, signed in as the placement admin in a context of its own. */
async function asAdmin<T>(browser: Browser, baseURL: string, fn: (req: Page["request"], origin: string) => Promise<T>): Promise<T> {
  const ctx = await browser.newContext({ baseURL });
  try {
    const page = await ctx.newPage();
    await signInAs(page, PLACEMENT_ADMIN);
    await page.waitForURL((u) => !u.pathname.startsWith("/v1/") && !u.pathname.startsWith("/dev-idp"), { timeout: 30_000 });
    return await fn(ctx.request, new URL(baseURL).origin);
  } finally {
    await ctx.close();
  }
}

test("1. fresh instance: the admin walks the five-step checklist to a working chat", async ({ page }) => {
  test.setTimeout(10 * 60_000);
  await signInAs(page, PLACEMENT_ADMIN);
  // A fresh instance sends its admin to the checklist; one that already has a placement is reached from Admin.
  await page.waitForURL((u) => !u.pathname.startsWith("/v1/") && !u.pathname.startsWith("/dev-idp"), { timeout: 30_000 });
  if (!page.url().includes("/admin")) await page.goto("/admin");
  await expect(page.getByRole("heading", { name: "Set up Aurora", level: 1 })).toBeVisible();
  const steps = page.getByRole("list", { name: "Setup steps" });
  // Five steps, by id (D-391: builtin apps arrive approved, so there is no approval step).
  for (const id of ["signin", "placement", "key", "try", "email"]) {
    await expect(steps.locator(`#setup-${id}`)).toHaveCount(1);
  }

  // 2. Where new people land: the row's button opens People › New users (no form on the checklist): shared
  // organization "Users" with personal workspaces (the recommended choice). On this instance the placement was once
  // personal_org (the pre-wizard default), so pick the mode, then create the shared organization "Users".
  await steps.locator("#setup-placement").getByRole("link", { name: /Choose|Change/ }).click();
  await expect(page).toHaveURL(/\/admin\/people\?tab=new-users/);
  const placement = page.getByRole("main");
  // The mode radio is `sr-only` inside its label: click the visible label text (a `check()` is not actionable).
  await placement.getByText("Shared organization", { exact: true }).click();
  await placement.getByLabel("Organization", { exact: true }).selectOption({ label: "Create a new organization…" });
  await placement.getByLabel("New organization name").fill("Users");
  // Personal workspaces are what makes the starter grant possible below (the AI-key dialog's starter grant needs a
  // shared organization WITH personal workspaces saved).
  await placement.getByRole("checkbox", { name: "Give every user a workspace of their own" }).check();
  await placement.getByRole("button", { name: "Save placement" }).click();
  await expect(placement.getByRole("status").filter({ hasText: "Saved" })).toBeVisible({ timeout: 30_000 });

  // 3. AI key: back on the checklist the row's button opens the dialog; saving stays on the checklist with a toast.
  await page.goto("/admin?setup=1");
  await expect(steps.locator("#setup-placement")).toHaveAttribute("data-done", "true", { timeout: 30_000 });
  await steps.locator("#setup-key").getByRole("button", { name: /Add key|Add another/ }).click();
  const dialog = page.getByRole("dialog", { name: "Add an organization AI key" });
  const form = dialog.getByRole("form", { name: "New LLM key" });
  await form.getByLabel("Name").fill("Mock LLM");
  await form.getByLabel("Provider endpoint").selectOption(ENDPOINT_ID);
  await form.getByLabel("API key", { exact: true }).fill(MOCK_KEY);
  await dialog.getByRole("button", { name: "Add key" }).click();
  await expect(page.getByTestId("setup-toast")).toContainText("AI key added", { timeout: 60_000 });
  await expect(page.getByRole("heading", { name: "Set up Aurora", level: 1 })).toBeVisible();
  await expect(steps.locator("#setup-key")).toHaveAttribute("data-done", "true");

  // 4. The Health tab says the runner and the gateway report ready on the stack.
  await page.getByRole("tab", { name: "Health" }).click();
  await expect(page.locator('[data-health="Runners"]')).toBeVisible();
  await page.getByRole("tab", { name: "Checklist" }).click();

  // 5. Try it: open the workspace and chat. The admin's own agent predates the placement (starter
  // grants apply to new users), so the guide finishes the job: the key step is done — the org key from
  // the checklist provides it — and access is one click away.
  await steps.locator("#setup-try").getByRole("link", { name: "Open chat" }).click();
  await expect(page.getByRole("navigation", { name: "Workspace" })).toBeVisible({ timeout: 30_000 });
  const base = /^\/o\/[^/]+\/w\/[^/]+/.exec(new URL(page.url()).pathname)![0];
  await page.goto(`${base}/welcome`);
  const guide = page.getByRole("region", { name: "Get Daneel talking" });
  await expect(guide.getByText("Give Daneel access to Mock LLM")).toBeVisible({ timeout: 30_000 }); // (C-927) only a key the agent cannot use
  // Access: Daneel is preselected, one click — done from the server's state.
  await expect(guide.getByRole("checkbox", { name: /Daneel/ })).toBeChecked();
  await guide.getByRole("button", { name: "Give Daneel access" }).click(); // (C-925) the button names who
  await expect(guide.getByText("1 of 2 done")).toBeVisible({ timeout: 30_000 });
  await guide.getByRole("link", { name: "Say hello to Daneel" }).click();
  await chatGetsAnswer(page);
});

test("2. new user with a starter grant: Welcome, no LLM step, first reply", async ({ page }) => {
  test.setTimeout(8 * 60_000);
  await signInAs(page, `starter-${Date.now().toString(36)}@example.test`);
  await expect(page).toHaveURL(/\/o\/[^/]+\/w\/[^/]+\/welcome$/, { timeout: 30_000 });
  const guide = page.getByRole("region", { name: "Get Daneel talking" });
  // The organization's key came with the starter grant: step 1 is collapsed and says who provides it.
  await expect(guide.getByText(/Your organization provides an AI key: Mock LLM/)).toBeVisible({ timeout: 30_000 });
  await expect(guide.getByRole("form", { name: "New LLM key" })).toHaveCount(0);
  await expect(guide.getByText("1 of 2 done")).toBeVisible();

  await guide.getByRole("link", { name: "Say hello to Daneel" }).click();
  await chatGetsAnswer(page);

  // Back on Overview: its sentence comes from real state (P5-POLISH-7; the guide line shows there only once Welcome
  // has been seen, never on both pages; first-run.spec.ts does the same).
  await page.goto(new URL(page.url()).pathname.replace(/\/agents\/.*$/, ""));
  await expect(page.getByText("Daneel is ready. Nothing needs you.")).toBeVisible({ timeout: 30_000 });
});

test("3. new user without a starter grant: the guide waits for access, then the first reply", async ({ page, browser, baseURL }) => {
  test.setTimeout(8 * 60_000);
  // The admin removes the starter grant (whole-document PUT of the saved placement).
  await asAdmin(browser, baseURL!, async (req, origin) => {
    const cur = await (await req.get("/v1/admin/placement")).json();
    const put = await req.put("/v1/admin/placement", {
      headers: { origin },
      data: {
        mode: cur.mode,
        default_org_id: cur.default_org?.id,
        default_org_role: cur.default_org_role,
        shared_workspaces: cur.shared_workspaces.map((w: { workspace_id: string; role: string }) => ({ workspace_id: w.workspace_id, role: w.role })),
        personal_workspace: cur.personal_workspace,
        starter_grants: [],
      },
    });
    expect(put.status(), await put.text()).toBe(200);
  });

  await signInAs(page, `plain-${Date.now().toString(36)}@example.test`);
  await expect(page).toHaveURL(/\/o\/[^/]+\/w\/[^/]+\/welcome$/, { timeout: 30_000 });
  const guide = page.getByRole("region", { name: "Get Daneel talking" });
  // No starter grant: the key step is not done by a grant. But this is a shared_org placement — as a
  // member the person sees the organization's key, so the guide says who provides it and waits for
  // access (a "0 of 2 done" + key-form walk is the personal_org world, covered by first-run.spec.ts).
  await expect(guide.getByText("Give Daneel access to Mock LLM")).toBeVisible({ timeout: 30_000 });
  await expect(guide.getByText("0 of 2 done")).toBeVisible();

  // Access: Daneel is preselected, one click — done from the server's state.
  await expect(guide.getByRole("checkbox", { name: /Daneel/ })).toBeChecked();
  await guide.getByRole("button", { name: "Give Daneel access" }).click(); // (C-925) the button names who
  await expect(guide.getByText("1 of 2 done")).toBeVisible({ timeout: 30_000 });

  // Reply.
  await guide.getByRole("link", { name: "Say hello to Daneel" }).click();
  await chatGetsAnswer(page);
});
