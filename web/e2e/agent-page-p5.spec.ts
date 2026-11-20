import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";

import { AGENT_ROUTE, DEFAULT, MINE, NIGHTLY, mockChat } from "./support/chat-mock";
import { id, mockApi, org, ws } from "./support/mock-api";
import { overflow } from "./support/open";
import { serious } from "./support/axe";

// P5-AGENT-2 (D-200, D-201; docs/14 §Agent page (phase 5)): the agent page's status rail. The API is mocked.
// Screenshots of the four states (expanded, collapsed, pop-over open, phone sheet) go outside the repo.

const SHOTS = "/tmp/aurora-screens/p5-agent-2";
mkdirSync(SHOTS, { recursive: true });
const KEY = "aurora.agent-status-rail";

const future = (m: number) => new Date(Date.now() + m * 60_000).toISOString();
const approvalOf = (n: number, conversation: string, title: string, m: number) => ({
  id: id(400 + n), org_id: org.id, workspace_id: ws.id, status: "pending", created_at: new Date().toISOString(), deadline: future(m),
  host: "api.github.com", method: "PUT", path: `/repos/acme/web/pulls/${n}/merge`, conversation_id: conversation, app_id: "github",
  agent: { id: id(11), handle: "daneel", name: "Daneel" }, summary: { source: "app", title, fields: [] },
  sentence: { agent: { id: id(11), name: "Daneel" }, action: title, app: { app_id: "github", name: "GitHub" } },
  can_always_allow: false,
});
const IN_OPEN = approvalOf(42, MINE.id, "Merge pull request #42", 9);
const ELSEWHERE = approvalOf(43, NIGHTLY.id, "Merge pull request #43", 4);

const row = (n: number, decision: string, tool: string | null, over: Record<string, unknown> = {}) => ({
  id: id(500 + n), agent_id: id(11), ts: new Date().toISOString(), decision, method: "GET", host: "api.github.com", path: "/x", status: 200,
  latency_ms: 80 * n, bytes_up: 0, bytes_down: 0, injected: 0, lane: "mitm", credential_kind: "agent", tool_id: tool, ...over,
});
const ACTIVITY = [row(1, "allow", "github.list_prs"), row(2, "blocked", "linear.create_issue"), row(3, "allow", null)];

/** Pending approvals of the agent, the request log, and the decision route; records every decision. */
async function mockRail(page: Page, opts: { approvals?: unknown[]; role?: string } = {}) {
  const decisions: string[] = [];
  const approvals = opts.approvals ?? [IN_OPEN, ELSEWHERE];
  await page.route("**/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const p = url.pathname;
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (route.request().method() === "GET" && p === `/v1/workspaces/${ws.id}/approvals`) return json({ items: approvals, next_cursor: null });
    if (route.request().method() === "GET" && p === `/v1/workspaces/${ws.id}/action-approvals`) return json({ items: [], next_cursor: null });
    if (route.request().method() === "GET" && p === `/v1/workspaces/${ws.id}/activity`) return json({ items: ACTIVITY, next_cursor: null });
    if (route.request().method() === "POST" && /^\/v1\/approvals\/[^/]+\/decision$/.test(p)) {
      decisions.push(p.split("/")[3]!);
      return json({ id: p.split("/")[3], status: "approved" });
    }
    if (opts.role && route.request().method() === "GET" && p === `/v1/workspaces/${ws.id}`) return json({ ...ws, my_role: opts.role });
    return route.fallback();
  });
  return decisions;
}

/** `openChat` with the rail's routes registered after the shared mocks (Playwright matches the latest route first). */
async function open(page: Page, scheme: "light" | "dark", opts: { approvals?: unknown[]; role?: string } = {}, query = "") {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  await mockChat(page, DEFAULT);
  const decisions = await mockRail(page, opts);
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(`${AGENT_ROUTE}${query}`);
  await page.getByRole("tab", { name: "Chat" }).click();
  await page.locator('nav[aria-label="Conversations"]').waitFor({ state: "attached" });
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, "the chat page threw").toEqual([]);
  return decisions;
}

const rail = (page: Page) => page.getByRole("complementary", { name: "Status" });
/** The page header (the one holding the agent's h1), and the chat card (the rounded panel around the conversation list). */
const pageHeader = (page: Page) => page.locator("header", { has: page.getByRole("heading", { level: 1 }) });
const chatCard = (page: Page) => page.locator('nav[aria-label="Conversations"]').locator("xpath=ancestor::div[contains(@class,'rounded-card')][1]");
const sizeOf = async (loc: ReturnType<Page["locator"]>) => (await loc.boundingBox())!;
const fits = (page: Page) => page.evaluate(() => document.scrollingElement!.scrollHeight - window.innerHeight);

test.describe("1280×800", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("collapsed by default: no page scroll, five icons with state in words, the choice persists across reloads", async ({ page }) => {
    await open(page, "light");
    const strip = rail(page);
    await expect(strip.getByRole("button", { name: "Expand status panel" })).toBeVisible();
    await expect(strip.getByRole("button")).toHaveCount(6); // expand + the five parts
    await expect(strip.getByRole("button", { name: "Needs you: 2 approvals waiting" })).toBeVisible();
    await expect(strip.getByRole("button", { name: /^Recent actions: some were denied or failed/ })).toBeVisible();
    await expect(strip.getByRole("button", { name: /^Computer: Sandbox running/ })).toBeVisible();
    expect(await fits(page), "the page scrolls").toBeLessThanOrEqual(0);
    expect(await overflow(page)).toBeLessThanOrEqual(0);
    expect(await page.evaluate((k) => window.localStorage.getItem(k), KEY)).toBeNull();
    await page.screenshot({ path: `${SHOTS}/collapsed-1280.png` });

    // Expand: 288 px, the heading and the parts; it is remembered (and never sent to the API).
    const requests: string[] = [];
    page.on("request", (r) => requests.push(`${r.method()} ${new URL(r.url()).pathname}`));
    await strip.getByRole("button", { name: "Expand status panel" }).click();
    const panel = rail(page);
    await expect(panel.getByRole("heading", { name: "Status", level: 2 })).toBeVisible();
    await expect(panel.getByRole("heading", { level: 3 })).toHaveText(["Needs you2", "Doing now", "Recent actions", "Computer", "About"]);
    expect(await fits(page), "the page scrolls with the rail open").toBeLessThanOrEqual(0);
    expect(await page.evaluate((k) => window.localStorage.getItem(k), KEY)).toBe("expanded");
    expect(requests.filter((r) => r.startsWith("PUT ") || r.startsWith("PATCH ") || r.startsWith("POST "))).toEqual([]);
    await page.reload();
    await expect(rail(page).getByRole("heading", { name: "Status", level: 2 })).toBeVisible();
    await rail(page).getByRole("button", { name: "Collapse status panel" }).click();
    await page.reload();
    await expect(rail(page).getByRole("button", { name: "Expand status panel" })).toBeVisible();
  });

  test("the Needs you icon opens a pop-over; Esc closes it and focus returns to the icon", async ({ page }) => {
    await open(page, "light");
    const icon =rail(page).getByRole("button", { name: "Needs you: 2 approvals waiting" });
    await icon.click();
    const pop = page.getByRole("dialog", { name: "Needs you" });
    await expect(pop).toBeVisible();
    await expect(pop.getByText("Merge pull request #43")).toBeVisible();
    await page.screenshot({ path: `${SHOTS}/popover-1280.png` });
    await page.keyboard.press("Escape");
    await expect(pop).toHaveCount(0);
    await expect(icon).toBeFocused();
    await icon.click();
    await pop.getByRole("button", { name: "Open the status panel" }).click();
    await expect(rail(page).getByRole("heading", { name: "Status", level: 2 })).toBeVisible();
  });

  test("Approve from the rail posts the decision; the open conversation's item links to its inline card", async ({ page }) => {
    const decisions = await open(page, "light", {}, `?c=${MINE.id}`);
    await rail(page).getByRole("button", { name: "Expand status panel" }).click();
    const needs = rail(page).getByRole("region", { name: /Needs you/ });
    // #42 belongs to the open conversation: no buttons, a link to the card. #43 is elsewhere: Approve and Deny.
    await expect(needs.getByRole("button", { name: "Allow once" })).toHaveCount(1);
    await needs.getByRole("button", { name: "Allow once" }).click();
    await expect.poll(() => decisions).toEqual([ELSEWHERE.id]);
  });

  for (const scheme of ["light", "dark"] as const) {
    test(`axe: Chat with the rail collapsed and expanded, one Settings page (${scheme})`, async ({ page }) => {
      await open(page, scheme);
      expect(await serious(page), "collapsed").toEqual([]);
      await rail(page).getByRole("button", { name: "Expand status panel" }).click();
      await expect(rail(page).getByRole("heading", { name: "Status", level: 2 })).toBeVisible();
      expect(await serious(page), "expanded").toEqual([]);
      await page.screenshot({ path: `${SHOTS}/expanded-1280-${scheme}.png` });
      await page.getByRole("button", { name: "Settings" }).click();
      await expect(page.getByRole("tablist", { name: "Settings pages" })).toBeVisible();
      expect(await serious(page), "behaviour").toEqual([]);
    });
  }

  test("non-approvers see no Needs you part, icon, banner or Waiting on you line", async ({ page }) => {
    await open(page, "light", { role: "member" });
    await expect(rail(page).getByRole("button", { name: /Needs you/ })).toHaveCount(0);
    await rail(page).getByRole("button", { name: "Expand status panel" }).click();
    await expect(rail(page).getByRole("heading", { name: /Needs you/ })).toHaveCount(0);
    await expect(page.getByText("Waiting on you")).toHaveCount(0);
  });

  test("the list shows Waiting on you for approvers", async ({ page }) => {
    await open(page, "light");
    const list =page.getByRole("navigation", { name: "Conversations" });
    await expect(list.getByRole("button", { name: /Waiting on you/ })).toHaveCount(2);
    // One line in the row (the visual review: it wrapped beside the time).
    const line = list.getByText("Waiting on you").first();
    expect((await sizeOf(line)).height).toBeLessThan(20);
  });

  // FIX-187: the rail's top cell and the thread header end on one line, collapsed or expanded.
  for (const scheme of ["light", "dark"] as const) {
    test(`the status rail's top cell lines up with the thread header (${scheme})`, async ({ page }) => {
      await open(page, scheme);
      const bottom = async (loc: ReturnType<Page["locator"]>) => {
        const b = await sizeOf(loc);
        return b.y + b.height;
      };
      const threadHeader = page.getByRole("region", { name: "Chat" }).locator("header").first();
      const line = await bottom(threadHeader);
      const expand = rail(page).getByRole("button", { name: "Expand status panel" });
      const cell = expand.locator("xpath=..");
      expect(Math.round((await sizeOf(cell)).height), "collapsed top cell height").toBe(64);
      expect(Math.abs((await bottom(cell)) - line), "collapsed cell bottom vs thread header").toBeLessThanOrEqual(1);
      await expand.click();
      const panelHeader = rail(page).getByRole("heading", { name: "Status", level: 2 }).locator("xpath=ancestor::header[1]");
      expect(Math.abs((await bottom(panelHeader)) - line), "expanded header bottom vs thread header").toBeLessThanOrEqual(1);
    });
  }

  for (const scheme of ["light", "dark"] as const) {
    test(`header is one ~56 px row, the card fills the height, the expanded rail pushes the thread (${scheme})`, async ({ page }) => {
      await page.addInitScript((k) => window.localStorage.setItem(k, "expanded"), KEY);
      await open(page, scheme);
      const header = pageHeader(page);
      const h = await sizeOf(header);
      expect(h.height, "header height").toBeLessThanOrEqual(64);
      // Breadcrumb above the name, beside the avatar; switcher and the Settings button on the same row.
      const crumb = await sizeOf(page.getByRole("navigation", { name: "Breadcrumb" }));
      const name = await sizeOf(header.getByRole("heading", { level: 1 }));
      expect(crumb.y + crumb.height, "breadcrumb sits above the name").toBeLessThanOrEqual(name.y + 2);
      const switcher = await sizeOf(page.getByRole("tablist", { name: "Agent sections" }));
      const actions = page.getByRole("button", { name: "Settings" });
      const a = await sizeOf(actions);
      // (D-408) A labelled button, not a 36 px icon: 36 px high, as wide as its words.
      expect(Math.round(a.height)).toBe(36);
      expect(a.width, "Settings keeps its label").toBeGreaterThan(80);
      // FIX-186: the track ends where the last tab ends.
      const lastTab = await sizeOf(page.getByRole("tablist", { name: "Agent sections" }).getByRole("tab").last());
      expect(Math.abs(switcher.x + switcher.width - (lastTab.x + lastTab.width)), "no empty track after the last tab").toBeLessThanOrEqual(4);
      expect(a.x, "actions at the right of the switcher").toBeGreaterThan(switcher.x + switcher.width);
      const rowMid = (b: { y: number; height: number }) => b.y + b.height / 2;
      expect(Math.abs(rowMid(a) - rowMid(switcher)), "switcher and actions share a row").toBeLessThanOrEqual(4);
      expect(switcher.y, "the switcher is inside the header").toBeGreaterThanOrEqual(h.y - 1);
      expect(switcher.y + switcher.height).toBeLessThanOrEqual(h.y + h.height + 1);
      // The card reaches the bottom of the window (a small margin) and the page does not scroll.
      const card = await sizeOf(chatCard(page));
      expect(800 - (card.y + card.height), "gap under the card").toBeLessThanOrEqual(40);
      expect(await fits(page), "the page scrolls").toBeLessThanOrEqual(0);
      // The expanded rail pushes the thread aside at 1280 px: the thread ends where the rail starts.
      const thread = await sizeOf(page.getByRole("region", { name: "Chat" }));
      const r = await sizeOf(rail(page).filter({ has: page.getByRole("heading", { name: "Status", level: 2 }) }));
      expect(thread.x + thread.width).toBeLessThanOrEqual(r.x + 1);
      await page.screenshot({ path: `${SHOTS}/header-1280-${scheme}.png` });
    });
  }
});

test.describe("1440×900", () => {
  test.use({ viewport: { width: 1440, height: 900 } });

  test("expanded by default when nothing is stored", async ({ page }) => {
    await open(page, "light");
    await expect(rail(page).getByRole("heading", { name: "Status", level: 2 })).toBeVisible();
    const box = await rail(page).boundingBox();
    expect(Math.round(box!.width)).toBe(288);
    expect(await fits(page), "the page scrolls").toBeLessThanOrEqual(0);
  });
});

test.describe("narrow panel", () => {
  test.use({ viewport: { width: 1000, height: 800 } });

  test("the expanded rail overlays the thread; Esc closes it", async ({ page }) => {
    await page.addInitScript((k) => window.localStorage.setItem(k, "expanded"), KEY);
    await open(page, "light");
    const panel =rail(page).filter({ has: page.getByRole("heading", { name: "Status", level: 2 }) });
    await expect(panel).toBeVisible();
    const thread = page.getByRole("region", { name: "Chat" });
    const t = await thread.boundingBox();
    const r = await panel.boundingBox();
    // The rail lies over the thread's right edge instead of narrowing it.
    expect(r!.x).toBeLessThan(t!.x + t!.width);
    await page.keyboard.press("Escape");
    await expect(panel).toHaveCount(0);
    await expect(rail(page).getByRole("button", { name: "Expand status panel" })).toBeFocused();
  });
});

test.describe("phone, 390 px", () => {
  test.use({ viewport: { width: 390, height: 800 } });

  test("no rail; the banner and the Status button open the sheet; no horizontal scroll", async ({ page }) => {
    await open(page, "light");
    await expect(page.getByRole("complementary", { name: "Status" })).toBeHidden();
    const banner = page.getByRole("button", { name: "2 approvals waiting" });
    await expect(banner).toBeVisible();
    expect((await banner.boundingBox())!.height).toBeGreaterThanOrEqual(44);
    expect(await overflow(page)).toBeLessThanOrEqual(0);
    await banner.click();
    const sheet = page.getByRole("dialog", { name: "Status" });
    await expect(sheet.getByRole("heading", { level: 3 })).toHaveText(["Needs you2", "Doing now", "Recent actions", "Computer", "About"]);
    await page.screenshot({ path: `${SHOTS}/sheet-390.png` });
    await page.keyboard.press("Escape");
    await expect(sheet).toHaveCount(0);
    await page.getByRole("button", { name: "Status", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Status" })).toBeVisible();
    expect(await overflow(page)).toBeLessThanOrEqual(0);
  });

  for (const scheme of ["light", "dark"] as const) {
    test(`compact header row, 44 px targets, no page scroll at 390×844 (${scheme})`, async ({ page }) => {
      await page.setViewportSize({ width: 390, height: 844 });
      await open(page, scheme);
      const header = pageHeader(page);
      const h = await sizeOf(header);
      // One row (back, avatar, name + status, actions) plus the section strip: well under two rows of 56.
      expect(h.height, "header height").toBeLessThanOrEqual(112);
      const back = await sizeOf(page.getByRole("link", { name: "Back to agents" }));
      const name = await sizeOf(header.getByRole("heading", { level: 1 }));
      const actions = await sizeOf(page.getByRole("button", { name: "Settings" }));
      expect(back.width).toBeGreaterThanOrEqual(44);
      expect(actions.width, "Settings keeps its label").toBeGreaterThan(80);
      expect(actions.height).toBeGreaterThanOrEqual(44);
      expect(Math.abs(back.y + back.height / 2 - (actions.y + actions.height / 2)), "back and actions share a row").toBeLessThanOrEqual(4);
      expect(name.y).toBeLessThan(back.y + back.height);
      const tabs = page.getByRole("tablist", { name: "Agent sections" }).getByRole("tab");
      for (let i = 0; i < (await tabs.count()); i += 1) expect((await sizeOf(tabs.nth(i))).height, `section tab ${i}`).toBeGreaterThanOrEqual(44);
      // FIX-186: the tabs share the row, no empty track after the last one.
      const strip = await sizeOf(page.getByRole("tablist", { name: "Agent sections" }));
      const last = await sizeOf(tabs.last());
      expect(Math.abs(strip.x + strip.width - (last.x + last.width)), "no empty track after the last tab").toBeLessThanOrEqual(4);
      expect(await fits(page), "the page scrolls").toBeLessThanOrEqual(0);
      expect(await overflow(page)).toBeLessThanOrEqual(0);
      await page.screenshot({ path: `${SHOTS}/phone-header-${scheme}.png` });
    });
  }
});
