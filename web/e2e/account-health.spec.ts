import { expect, test, type Page } from "@playwright/test";

import { agent, id, mockApi, now, ws } from "./support/mock-api";
import { overflow } from "./support/open";
import { serious } from "./support/axe";

// P5-UX-14 (D-335, D-350, C-943..C-945, C-985; docs/14 §Agents, accounts and access §7): the health line on an account's
// row with its fix, Test in the row's menu replacing the line, and the agent's header counting the accounts that need
// attention. Both themes at 1280 px and 390 px, no horizontal scroll, axe (serious and critical) clean.

const C_PG = id(51);
const C_AWS = id(52);
const can = { give_access: true, change_access: true, manage: true };

/** A PostgreSQL account that could not be reached (stored result `unreachable`) and an AWS account whose sign-in expired. */
async function mockAccounts(page: Page) {
  const state = { tests: 0, pg: "unreachable" as "unreachable" | "ok" };
  await mockApi(page);
  await page.route("**/v1/**", async (route) => {
    const p = new URL(route.request().url()).pathname;
    const m = route.request().method();
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (p === `/v1/workspaces/${ws.id}/accounts`) {
      const given = (level: string) => [{ agent: { id: agent.id, name: agent.name, handle: agent.handle }, workspace: { id: ws.id, name: ws.name }, level }];
      return json({
        items: [
          {
            ref: { type: "connection", id: C_PG }, kind: "app", name: "analytics", scope: "workspace", levels: ["read", "ask", "act"], given: true,
            app: { app_id: "postgresql", name: "PostgreSQL", custom_only: false },
            health: state.pg === "unreachable" ? { state: "failing", last_error: "unreachable", detail: "db.platform.example.org", last_error_at: now, last_used_at: null } : { state: "working", last_ok_at: now, last_used_at: now },
            agents: given("read"), can,
          },
          {
            ref: { type: "connection", id: C_AWS }, kind: "app", name: "platform-prod", scope: "workspace", levels: ["read", "ask", "act"], given: true,
            app: { app_id: "aws-keys", name: "AWS", custom_only: false },
            health: { state: "sign_in_expired", last_used_at: now },
            agents: given("read"), can,
          },
        ],
        next_cursor: null,
      });
    }
    // (P5-REFINE-13, access-11) The menu offers Test the connection only for an app with a probe: PostgreSQL has one.
    if (p === "/v1/apps/postgresql" && m === "GET") return json({ app_id: "postgresql", name: "PostgreSQL", package_id: "postgresql", connect: [{ id: "conn", kind: "fields", has_probe: true, fields: [], scopes: [], access_levels: [] }], tools: [], hosts: [], credentials: [], metadata: [], oauth_clients: [] });
    if (p === `/v1/accounts/connection/${C_PG}/test` && m === "POST") {
      state.tests += 1;
      state.pg = "unreachable";
      return json({ result: "unreachable", detail: { host: "db.platform.example.org" }, checked_at: now });
    }
    if (p === `/v1/accounts/connection/${C_AWS}/test` && m === "POST") return json({ result: "not_testable", checked_at: now });
    if (p === `/v1/agents/${agent.id}/grants`) return json({ secrets: [], connections: [], converted_by_app: [] });
    return route.fallback();
  });
  return state;
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

/**
 * What sticks out of its box on the account cards: a card out of the list, or a health line's words or fix out of the
 * card's text column (the page can stay unscrolled while either reaches into the gap or the gutter).
 */
const sticksOut = (page: Page) =>
  page.getByRole("listitem", { name: /^(analytics|platform-prod)$/ }).evaluateAll((cards) =>
    cards.flatMap((card) => {
      const out: string[] = [];
      const past = (what: string, el: Element | Range, box: Element) => {
        const by = Math.max(...[...el.getClientRects()].map((r) => Math.max(r.right - box.getBoundingClientRect().right, box.getBoundingClientRect().left - r.left)));
        if (by > 0.5) out.push(`${card.getAttribute("aria-label")}: ${what} by ${Math.round(by)} px`);
      };
      past("the card", card, card.parentElement!);
      const column = card.querySelector("h3")!.parentElement!.parentElement!;
      for (const line of column.querySelectorAll("p.flex")) {
        const words = document.createTreeWalker(line, NodeFilter.SHOW_TEXT);
        for (let n = words.nextNode(); n; n = words.nextNode()) {
          const range = document.createRange();
          range.selectNodeContents(n);
          past(`"${n.textContent}"`, range, column);
        }
        for (const button of line.querySelectorAll("button")) past(`the ${button.textContent} button`, button, column);
      }
      return out;
    }),
  );

async function check(page: Page) {
  expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
  expect(await serious(page)).toEqual([]);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("a row says what stopped, with the fix beside it", async ({ page }) => {
        await mockAccounts(page);
        await open(page, "/o/acme/w/main/apps?tab=accounts", scheme);
        const pg = page.getByRole("listitem", { name: "analytics" });
        await expect(pg.getByText(/it could not be reached \(db\.platform\.example\.org\)/)).toBeVisible();
        await expect(pg.getByRole("button", { name: "Try again" })).toBeVisible();
        const aws = page.getByRole("listitem", { name: "platform-prod" });
        await expect(aws.getByText("Sign-in expired")).toBeVisible();
        await expect(aws.getByRole("button", { name: "Sign in again" })).toBeVisible();
        expect.soft(await sticksOut(page), "nothing sticks out of its box").toEqual([]);
        await check(page);
      });

      test("Test in the menu runs the check and an account that cannot be tested says so", async ({ page }) => {
        const state = await mockAccounts(page);
        await open(page, "/o/acme/w/main/apps?tab=accounts", scheme);
        const pg = page.getByRole("listitem", { name: "analytics" });
        await pg.getByRole("button", { name: "More for analytics" }).click();
        await page.getByRole("menuitem", { name: "Test the connection" }).click();
        await expect.poll(() => state.tests).toBe(1);
        await expect(pg.getByText(/it could not be reached \(db\.platform\.example\.org\)/)).toBeVisible();
        // The sign-in of the other account expired: it is fixed by Reconnect, there is nothing to test.
        const aws = page.getByRole("listitem", { name: "platform-prod" });
        await aws.getByRole("button", { name: "More for platform-prod" }).click();
        await expect(page.getByRole("menuitem", { name: "Test the connection" })).toHaveCount(0);
        await page.keyboard.press("Escape");
        await check(page);
      });

      test("the agent's header counts the accounts that need attention and links to the Access tab", async ({ page }) => {
        await mockAccounts(page);
        await open(page, `/o/acme/w/main/agents/${agent.handle}`, scheme);
        const link = page.getByRole("link", { name: "2 accounts need attention" });
        await expect(link).toBeVisible();
        await expect(link).toHaveAttribute("href", /tab=access/);
        await check(page);
      });
    });
  }
}
