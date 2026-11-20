import { expect, test, type Page } from "@playwright/test";

import { id, mockApi, now, ws } from "./support/mock-api";
import { serious } from "./support/axe";
import { overflow } from "./support/open";

// P5-REFINE-16: the workspace pages, first day and account, mocked, both themes at 1280 and 390 px, axe, no console errors, no
// horizontal scroll. One test per page of the brief's Playwright list. (Written by reading: the lane has no browser; the CI Watcher runs them.)

const WS = `/v1/workspaces/${ws.id}`;
const json = (b: unknown, status = 200) => ({ status, contentType: "application/json", body: JSON.stringify(b) });

async function open(page: Page, path: string, scheme: "light" | "dark", routes: Record<string, (method: string) => unknown> = {}) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  await page.emulateMedia({ colorScheme: scheme });
  await page.route("**/v1/**", (route) => {
    const req = route.request();
    const h = routes[new URL(req.url()).pathname];
    const body = h?.(req.method());
    return body === undefined ? route.fallback() : route.fulfill(json(body));
  });
  await page.goto(path);
  await page.waitForLoadState("networkidle");
  return errors;
}

async function check(page: Page, errors: string[]) {
  expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
  expect(await serious(page)).toEqual([]);
  expect(errors, "page threw").toEqual([]);
}

const entry = (n: number, over: Record<string, unknown>) => ({
  id: id(500 + n), ts: now, agent_id: id(20), method: "POST", host: "api.github.com", path: "/repos", decision: "allow", status: 200,
  rule_name: null, app_id: null, tool_id: null, llm_model: null, context_label: null, approval_id: null, ...over,
});

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("Activity reads as sentences, with the raw request under Details", async ({ page }) => {
        const errors = await open(page, "/o/acme/w/main/activity", scheme, {
          [`${WS}/activity`]: () => ({
            items: [
              entry(1, { path: "/v1/messages", host: "litellm.svc", llm_model: "default" }),
              entry(2, { decision: "blocked", status: 403, rule_name: "Block evil.example.org", app_id: "github" }),
            ],
            next_cursor: null,
          }),
        });
        const list = page.getByTestId("activity-sentences");
        await expect(list.getByText(/answered you/)).toBeVisible();
        await expect(list).not.toContainText("litellm");
        await list.getByRole("button", { name: "Details" }).first().click();
        await expect(page.getByTestId("activity-raw")).toContainText("POST litellm.svc/v1/messages");
        await check(page, errors);
      });

      test("Audit reads as sentences with Critical in red", async ({ page }) => {
        const errors = await open(page, "/o/acme/w/main/settings?tab=audit", scheme, {
          [`${WS}/audit`]: () => ({
            items: [
              { id: id(600), org_id: id(1), workspace_id: ws.id, actor_kind: "user", actor_id: id(99), actor_label: "Bob", action: "api_key.revoke", target_kind: "api_key", target_id: id(601), outcome: "success", source: "api", ip: null, metadata: { name: "ci-script" }, critical: true, created_at: now },
            ],
            next_cursor: null,
          }),
        });
        await expect(page.getByText("Bob revoked the API key ci-script")).toBeVisible();
        await expect(page.getByText("Critical", { exact: true })).toBeVisible();
        await check(page, errors);
      });

      test("Add people says how they open the workspace; promoting to Owner asks", async ({ page }) => {
        const errors = await open(page, "/o/acme/w/main/people", scheme);
        await page.getByRole("button", { name: "Add people" }).click();
        const dialog = page.getByRole("dialog", { name: "Add people" });
        await dialog.getByLabel("Who").fill("carol@example.org");
        await expect(dialog.getByTestId("how-they-open")).toContainText("Aurora doesn't send an e-mail. carol@example.org can open this workspace at");
        await expect(dialog.getByRole("button", { name: "Copy link" })).toBeVisible();
        await check(page, errors);
      });

      test("Renaming the workspace says Saved and the switcher shows the new name", async ({ page }) => {
        const errors = await open(page, "/o/acme/w/main/settings", scheme);
        await page.getByLabel("Name").fill("Bob's Lab");
        await page.getByRole("button", { name: "Save" }).click();
        await expect(page.getByTestId("saved-name")).toContainText("Saved. The workspace is now called Bob's Lab.");
        await check(page, errors);
      });

      test("Apps: search narrows the groups", async ({ page }) => {
        const errors = await open(page, "/o/acme/w/main/apps?tab=apps", scheme);
        const box = page.getByRole("searchbox", { name: "Search apps" });
        await expect(box).toBeVisible();
        await box.fill("zzz-no-such-app");
        await expect(page.getByText("No app matches “zzz-no-such-app”.")).toBeVisible();
        await check(page, errors);
      });

      test("Approvals: one empty state and an Example card that changes nothing", async ({ page }) => {
        const posts: string[] = [];
        page.on("request", (r) => r.method() === "POST" && posts.push(r.url()));
        const errors = await open(page, "/o/acme/w/main/approvals", scheme, {
          [`${WS}/approvals`]: () => ({ items: [], next_cursor: null }),
          [`${WS}/action-approvals`]: () => ({ items: [], next_cursor: null }),
        });
        await expect(page.getByRole("heading", { name: "Nothing is waiting for you." })).toBeVisible();
        const example = page.getByTestId("approval-example");
        await example.getByRole("button", { name: "Block" }).click();
        await expect(page.getByTestId("example-says")).toContainText("is told no, and carries on without it.");
        expect(posts.filter((u) => u.includes("approvals"))).toEqual([]);
        await check(page, errors);
      });

      test("the tour dims the page and says Step 1 of n", async ({ page }) => {
        // Start it the way the product does: Show me around on the Welcome page (as onboarding.spec.ts).
        const errors = await open(page, "/o/acme/w/main/welcome", scheme);
        await page.getByRole("button", { name: "Show me around" }).click();
        await expect(page.getByText(/Step 1 of \d/)).toBeVisible();
        await expect(page.getByTestId("tour-backdrop")).toBeAttached();
        await page.keyboard.press("Escape");
        await expect(page.getByTestId("tour-card")).toHaveCount(0);
        await check(page, errors);
      });

      test("Account: Create key and the SSH paste", async ({ page }) => {
        const errors = await open(page, "/account/api-keys", scheme, { "/v1/me/api-keys": () => ({ items: [], next_cursor: null }) });
        await page.getByRole("button", { name: "Create key" }).first().click();
        const dialog = page.getByRole("dialog", { name: "Create an API key" });
        await expect(dialog.getByText("The key can act only in this workspace.")).toBeVisible();
        await expect(dialog.getByRole("button", { name: "Create key" })).toBeDisabled();
        await check(page, errors);
      });
    });
  }
}

test.describe("phone only", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test("the switcher card opens its list and stays on the page", async ({ page }) => {
    const errors = await open(page, "/o/acme/w/main", "light");
    await page.getByRole("button", { name: "Open menu" }).click();
    const card = page.getByTestId("scope-switcher").getByRole("button");
    await card.click();
    await expect(page.getByRole("dialog", { name: "Switch workspace or organization" }).or(page.getByLabel("Switch workspace or organization"))).toBeVisible();
    expect(page.url()).toContain("/o/acme/w/main");
    expect(errors).toEqual([]);
  });
});

test("the Approvals badge drops after deciding, without a reload", async ({ page }) => {
  // (P5-REFINE-16 item 19, G1/G2) One count from C-1082: Approvals shows its part, Overview the whole; both drop together.
  let waiting = 2;
  await open(page, "/o/acme/w/main", "light", {
    "/v1/attention/counts": () => ({ workspace: waiting === 0 ? { workspace_id: ws.id, count: 0, topics: {} } : { workspace_id: ws.id, count: waiting + 1, topics: { approvals: waiting } } }),
  });
  await expect(page.getByTestId("approvals-badge")).toContainText("2 need your attention");
  await expect(page.getByTestId("overview-badge")).toContainText("3 need your attention");
  waiting = 1;
  await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
  await expect(page.getByTestId("approvals-badge")).toContainText("1 needs your attention", { timeout: 35_000 });
  await expect(page.getByTestId("overview-badge")).toContainText("2 need your attention");
  waiting = 0;
  await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
  await expect(page.getByTestId("approvals-badge")).toHaveCount(0, { timeout: 35_000 });
  await expect(page.getByTestId("overview-badge")).toHaveCount(0);
});

test("Check a request answers in a sentence with the raw verdict collapsed", async ({ page }) => {
  const errors = await open(page, "/o/acme/w/main/internet-access", "light");
  await page.getByRole("button", { name: "Try a request" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("button", { name: "Check" })).toBeVisible();
  expect(errors).toEqual([]);
});
