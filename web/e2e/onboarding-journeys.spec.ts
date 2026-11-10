import { expect, test, type Browser, type Page } from "@playwright/test";

import { PLACEMENT_ADMIN } from "./support/placement";

// P4-ONB-2 (D-141): the three onboarding journeys on the kind dev stack with the --llm-mock profile
// (scripts/e2e/first-run.sh -- e2e/onboarding-journeys.spec.ts). Dashboard only, except where a step says it
// changes the instance's saved placement through the API (the journeys share one instance).
//   1. a fresh instance: the admin walks the wizard to a working chat;
//   2. a new user with a starter grant: Welcome -> first reply, with no LLM step;
//   3. a new user without one: the guide walks key -> access -> (model) -> reply.
// The mock: endpoint `litellm-mock` defaults to the model alias `scn-pong`, which VidaiMock answers with "pong".
const MOCK_KEY = "sk-aurora-mock-dev"; // dev-only dummy master key (docs/operator/llm-endpoints.md)
const MOCK_REPLY = "pong";
const ENDPOINT_LABEL = "LiteLLM (VidaiMock, dev only) (litellm)";

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

test("1. fresh instance: the admin walks the setup wizard to a working chat", async ({ page }) => {
  test.setTimeout(10 * 60_000);
  await signInAs(page, PLACEMENT_ADMIN);
  // A fresh instance sends its admin to the wizard; one that already has a placement is reached from Admin.
  await page.waitForURL((u) => !u.pathname.startsWith("/v1/") && !u.pathname.startsWith("/dev-idp"), { timeout: 30_000 });
  if (!page.url().includes("/admin/setup")) await page.goto("/admin/setup");
  await expect(page.getByRole("heading", { name: "Set up Aurora", level: 1 })).toBeVisible();
  const steps = page.getByRole("list", { name: "Setup steps" });
  // Six steps (the placement step also embeds a "Personal organizations from before" region, so count the <li>s).
  await expect(steps.getByRole("listitem")).toHaveCount(6);

  // 2. Where new users land: shared organization "Users" with personal workspaces (the recommended choice).
  const placement = steps.getByRole("region", { name: "Where new users land" });
  await placement.getByRole("button", { name: "Save placement" }).click();
  await expect(placement.getByText("Saved. New sign-ins follow it from now on.")).toBeVisible({ timeout: 30_000 });

  // 3. LLM access: add an organization key for the default org, and give it to every new user's agent.
  const llm = steps.getByRole("region", { name: "LLM access" });
  const form = llm.getByRole("form", { name: "New LLM key" });
  await form.getByLabel("Name").fill("Mock LLM");
  await form.getByLabel("Provider endpoint").selectOption({ label: ENDPOINT_LABEL });
  await form.getByLabel("API key").fill(MOCK_KEY);
  await form.getByRole("button", { name: "Add LLM key" }).click();
  // The key is saved when the wizard shows it from the server's state: the form gives way to the starter
  // switch ("Saved …" is transient — the Setup query refetch replaces the form as soon as the key exists).
  const starter = llm.getByRole("checkbox", { name: "Give it to every new user's agent" });
  await expect(starter).toBeVisible({ timeout: 60_000 });
  await starter.check();
  await expect(starter).toBeChecked({ timeout: 30_000 });

  // 4. Agents can run: the runner and the gateway report ready on the stack.
  await expect(steps.getByRole("region", { name: "Agents can run" }).getByText("Ready").first()).toBeVisible();

  // 6. Try it: open the workspace and chat. (The admin lands in the shared organization's workspace.)
  await steps.getByRole("link", { name: "Open your workspace" }).click();
  await expect(page.getByRole("navigation", { name: "Workspace" })).toBeVisible({ timeout: 30_000 });
  const base = new URL(page.url()).pathname.replace(/\/welcome$/, "");
  await page.goto(`${base}/agents`);
  await page.getByRole("link", { name: /Daneel/ }).first().click();
  await page.getByRole("tab", { name: "Chat" }).click();
  await page.getByLabel("Message", { exact: true }).fill("hello");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page.getByText(MOCK_REPLY, { exact: true }).first()).toBeVisible({ timeout: 4 * 60_000 });
});

test("2. new user with a starter grant: Welcome, no LLM step, first reply", async ({ page }) => {
  test.setTimeout(8 * 60_000);
  await signInAs(page, `starter-${Date.now().toString(36)}@example.test`);
  await expect(page).toHaveURL(/\/o\/[^/]+\/w\/[^/]+\/welcome$/, { timeout: 30_000 });
  const guide = page.getByRole("region", { name: "Get Daneel talking" });
  // The organization's key came with the starter grant: step 1 is collapsed and says who provides it.
  await expect(guide.getByText(/Your organization provides an LLM key: Mock LLM/)).toBeVisible({ timeout: 30_000 });
  await expect(guide.getByRole("form", { name: "New LLM key" })).toHaveCount(0);
  await expect(guide.getByText("2 of 3 done")).toBeVisible();

  await guide.getByRole("link", { name: "Say hello to Daneel" }).click();
  await chatGetsAnswer(page);

  // Back on Overview the guide is one line.
  await page.goto(new URL(page.url()).pathname.replace(/\/agents\/.*$/, ""));
  await expect(page.getByText(/Getting started — 3\/3/)).toBeVisible({ timeout: 30_000 });
});

test("3. new user without a starter grant: the guide walks key, access and reply", async ({ page, browser, baseURL }) => {
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
  await expect(guide.getByText("0 of 3 done")).toBeVisible({ timeout: 30_000 });

  // Key: the form opens in place.
  const form = guide.getByRole("form", { name: "New LLM key" });
  await form.getByLabel("Name").fill("My key");
  await form.getByLabel("Provider endpoint").selectOption({ label: ENDPOINT_LABEL });
  await form.getByLabel("API key").fill(MOCK_KEY);
  await form.getByRole("button", { name: "Add LLM key" }).click();
  // The key is saved when the guide shows it from the server's state: the key step collapses ("Saved …" is
  // transient — the form unmounts the moment the invalidated queries refetch and the step ticks done).
  await expect(guide.getByText("1 of 3 done")).toBeVisible({ timeout: 60_000 });

  // Access: Daneel is preselected.
  await expect(guide.getByRole("checkbox", { name: /Daneel/ })).toBeChecked();
  await guide.getByRole("button", { name: "Give access" }).click();
  await expect(guide.getByText("2 of 3 done")).toBeVisible({ timeout: 30_000 });

  // Reply.
  await guide.getByRole("link", { name: "Say hello to Daneel" }).click();
  await chatGetsAnswer(page);
});
