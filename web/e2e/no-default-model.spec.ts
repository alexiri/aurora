import { expect, test } from "@playwright/test";

// FIX-019 (D-136): the owner's production endpoint shape on the kind dev stack with --llm-mock.
// The endpoint `aigw-mock` (deploy/kind/values-llm-mock.yaml) has only `base_url` and
// no `default_model`, `models` or `dialects` (and no key_origin rule, which behaves as `endpoint`).
//   store a key, grant it, chat -> the turn fails `no_default_model` (not `no_model_key`) and no
//   sandbox starts; choose the model on the agent -> the next message gets the mock's answer.
// Setup goes through the API (this journey is about the server's decision; first-run.spec.ts is the
// dashboard-only journey). Run with scripts/e2e/first-run.sh -- e2e/no-default-model.spec.ts.
const MOCK_KEY = "sk-aurora-mock-dev"; // dev-only dummy master key
const ENDPOINT = "aigw-mock";

test("owner's endpoint shape: no_default_model, then a reply after choosing a model", async ({ page }) => {
  test.setTimeout(8 * 60_000);
  const email = `nodefault-${Date.now().toString(36)}@example.test`;
  await page.goto("/auth/login");
  await page.goto(`/v1/auth/oidc/dev/start?login_hint=${encodeURIComponent(email)}&return_to=%2F`);
  await expect(page.getByRole("navigation", { name: "Workspace" })).toBeVisible({ timeout: 30_000 });
  await expect(page).toHaveURL(/\/o\/[^/]+\/w\/([^/]+)$/);
  const [, orgSlug, wsSlug] = /\/o\/([^/]+)\/w\/([^/]+)$/.exec(new URL(page.url()).pathname)!;
  const base = new URL(page.url()).pathname;
  const origin = new URL(page.url()).origin;
  const h = { origin };
  const api = page.request;

  // URL slugs -> ids, then the default agent (created at first sign-in).
  const resolved = await (await api.get(`/v1/resolve?org=${orgSlug}&workspace=${wsSlug}`)).json();
  const ws = resolved.workspace.id as string;
  const agents = await (await api.get(`/v1/workspaces/${ws}/agents`)).json();
  const agent = (agents.items ?? agents)[0].id as string;

  const key = await api.post(`/v1/workspaces/${ws}/secrets`, {
    headers: h,
    data: { name: "AIGW key", kind: "llm", llm: { endpoint_id: ENDPOINT }, value: MOCK_KEY },
  });
  expect(key.status(), await key.text()).toBe(201);
  const keyId = (await key.json()).id as string;
  const grant = await api.put(`/v1/agents/${agent}/grants/secrets/${keyId}`, { headers: h });
  expect(grant.ok(), await grant.text()).toBeTruthy();

  // The key is usable, the model is not: the readiness route says so.
  const rd = await (await api.get(`/v1/agents/${agent}/model-readiness`)).json();
  expect(rd.keys[0].usable).toBe(true);
  expect(rd.default).toBeNull();

  // Chat: the dashboard's conversation page, stream status observed.
  await page.goto(`${base}/agents`);
  await page.getByRole("link", { name: /Daneel/ }).first().click();
  await page.getByRole("tab", { name: "Chat" }).click();
  await page.getByLabel("Message", { exact: true }).fill("hello");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page.getByText(/no model is chosen|no_default_model/i).first()).toBeVisible({ timeout: 60_000 });
  // Stream: the API stream opens in milliseconds, delivers `turn.queued`, `error` and `turn.finished`,
  // and stays open (probed with fetch). The dashboard pill nevertheless flips to "Reconnecting…" on any
  // failed turn: `web/src/lib/chat/use-transcript.ts` sets `es.onerror`, which also fires for an SSE event
  // NAMED `error` (readyState stays OPEN). A client bug, not the API stream, so not asserted here.
  const sandbox = await (await api.get(`/v1/agents/${agent}/sandbox`)).json();
  expect(["unprovisioned", "stopped"]).toContain(sandbox.state);

  // Choose the model on the agent; the next message starts the sandbox and the mock answers.
  const patch = await api.patch(`/v1/agents/${agent}`, {
    headers: h,
    data: { model_endpoint_id: ENDPOINT, model_id: "scn-pong" },
  });
  expect(patch.ok(), await patch.text()).toBeTruthy();
  await page.getByLabel("Message", { exact: true }).fill("hello again");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page.getByText("pong", { exact: true }).first()).toBeVisible({ timeout: 4 * 60_000 });
});
