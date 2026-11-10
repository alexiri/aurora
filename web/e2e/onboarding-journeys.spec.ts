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
  // Six steps, by id (nested check lists make a role/li count meaningless).
  for (const id of ["roles", "placement", "llm", "runners", "optional", "try"]) {
    await expect(steps.locator(`#setup-${id}`)).toHaveCount(1);
  }

  // 2. Where new users land: shared organization "Users" with personal workspaces (the recommended choice).
  // On this instance the placement was once personal_org (the pre-wizard default), so pick the mode, then
  // create the shared organization "Users" from the wizard.
  const placement = steps.getByRole("region", { name: "Where new users land" });
  // The mode radio is `sr-only` inside its label: click the visible label text (a `check()` is not actionable).
  await placement.getByText("Shared organization", { exact: true }).click();
  await placement.getByLabel("Organization", { exact: true }).selectOption({ label: "Create a new organization…" });
  await placement.getByLabel("New organization name").fill("Users");
  // Personal workspaces are what makes the starter grant possible below (the LLM step's switch stays
  // disabled until a shared organization WITH personal workspaces is saved).
  await placement.getByRole("checkbox", { name: "Give every user a workspace of their own" }).check();
  await placement.getByRole("button", { name: "Save placement" }).click();
  // The step is saved when the wizard shows it from the server's state (the "Saved…" status is transient:
  // the form is re-keyed on the saved revision, so it remounts the moment the placement refetch lands).
  await expect(placement.getByText("Ready", { exact: true })).toBeVisible({ timeout: 30_000 });

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
  // The checkbox is controlled: it only renders checked once the PUT and the refetch have round-tripped,
  // so click once and then wait for the server's state (a `check()` verifies the state straight away).
  await starter.click();
  await expect(starter).toBeChecked({ timeout: 30_000 });

  // 4. Agents can run: the runner and the gateway report ready on the stack.
  await expect(steps.getByRole("region", { name: "Agents can run" }).getByText("Ready").first()).toBeVisible();

  // 6. Try it: open the workspace and chat. The admin's own agent predates the placement (starter
  // grants apply to new users), so the guide finishes the job: the key step is done — the org key from
  // the wizard provides it — and access is one click away.
  await steps.getByRole("link", { name: "Open your workspace" }).click();
  await expect(page.getByRole("navigation", { name: "Workspace" })).toBeVisible({ timeout: 30_000 });
  const base = new URL(page.url()).pathname.replace(/\/welcome$/, "");
  await page.goto(`${base}/welcome`);
  const guide = page.getByRole("region", { name: "Get Daneel talking" });
  await expect(guide.getByText(/Your organization provides an LLM key: Mock LLM/)).toBeVisible({ timeout: 30_000 });
  // Access: Daneel is preselected, one click — done from the server's state.
  await expect(guide.getByRole("checkbox", { name: /Daneel/ })).toBeChecked();
  await guide.getByRole("button", { name: "Give access" }).click();
  await expect(guide.getByText("2 of 3 done")).toBeVisible({ timeout: 30_000 });
  await guide.getByRole("link", { name: "Say hello to Daneel" }).click();
  await chatGetsAnswer(page);
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
  // access (a "0 of 3 done" + key-form walk is the personal_org world, covered by first-run.spec.ts).
  await expect(guide.getByText(/Your organization provides an LLM key: Mock LLM/)).toBeVisible({ timeout: 30_000 });
  await expect(guide.getByText("1 of 3 done")).toBeVisible();

  // Access: Daneel is preselected, one click — done from the server's state.
  await expect(guide.getByRole("checkbox", { name: /Daneel/ })).toBeChecked();
  await guide.getByRole("button", { name: "Give access" }).click();
  await expect(guide.getByText("2 of 3 done")).toBeVisible({ timeout: 30_000 });

  // Reply.
  await guide.getByRole("link", { name: "Say hello to Daneel" }).click();
  await chatGetsAnswer(page);
});
