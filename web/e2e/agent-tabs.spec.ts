import { expect, test } from "@playwright/test";

import { mockWithTerminal } from "./support/terminal";
import { CANVAS, overflow, type Scheme } from "./support/open";

// FIX-129 + P5-AGENT-2 (D-200, docs/14 §Agent page (phase 5)): the agent page is five sections with a segmented row of
// pages inside a section that has several; since P5-REFINE-11 (D-408) Chat · Automations · Computer are tabs and the
// Settings pages (Instructions, Apps & access, Memory, Skills, Tests, Other agents) open from the Settings button. Every page id of
// `?tab=` still opens its page, at 390, 768, 1280 and 1920 px, both themes: the page does not scroll
// horizontally. The API is mocked.

/** Page id → the section that shows it (docs/14: the section is derived from the page). */
const PAGES: [string, string][] = [
  ["chat", "Chat"],
  ["instructions", "Instructions"],
  ["memory", "Memory"],
  ["skills", "Skills"],
  ["automations", "Automations"],
  // (P5-REFINE-14) The old ids open the one Automations page, filtered to their kind.
  ["schedules", "Automations"],
  ["webhooks", "Automations"],
  ["peers", "Other agents"],
  ["channels", "Automations"],
  ["computer", "Computer"],
  ["previews", "Computer"],
  ["terminal", "Computer"],
  ["access", "Apps & access"],
];

/** Pages that are Settings pages (D-408): the Settings button, a sub-nav, no section tab selected. */
const SETTINGS_PAGES = new Set(["Instructions", "Memory", "Skills", "Tests", "Other agents", "Apps & access"]);

/** The shared mock plus the terminal socket, and the Access tab's grants (empty) the shared mock does not serve. */
async function mockAgent(page: import("@playwright/test").Page) {
  await mockWithTerminal(page, { output: new Uint8Array(), resizes: [] });
  await page.route("**/v1/agents/*/grants", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ secrets: [], connections: [] }) }),
  );
  await page.route("**/v1/agents/*/effective", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ credentials: [], tools: [] }) }),
  );
  // (P5-POLISH-6) The Channels page shows its sections (and the progress switch) only when the organization has a chat
  // integration: one the agent can be attached to.
  await page.route("**/v1/agents/*/presence-options", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        items: [{ integration_id: "i1", connector_id: "mattermost", connector_name: "Mattermost", tenant_label: "chat.example.com", modes: ["shared"], presence_setup: {}, attached: false }],
      }),
    }),
  );
}

const tablistOverflow = (page: import("@playwright/test").Page) =>
  page.getByRole("tablist").evaluateAll((els) => Math.max(0, ...els.map((el) => el.scrollWidth - el.clientWidth)));

for (const scheme of ["light", "dark"] as const satisfies readonly Scheme[]) {
  for (const width of [390, 768, 1280, 1920]) {
    test.describe(`agent page sections (${width} px, ${scheme})`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("every page: its section is selected, no horizontal scroll on the page or a tablist", async ({ page }) => {
        const errors: string[] = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await mockAgent(page);
        await page.emulateMedia({ colorScheme: scheme });
        for (const [tab, section] of PAGES) {
          await page.goto(`/o/acme/w/main/agents/daneel?tab=${tab}`);
          await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
          const sections = page.getByRole("tablist", { name: "Agent sections" });
          if (SETTINGS_PAGES.has(section)) {
            // (D-408) Settings pages open from the Settings button: no section tab is selected and the page carries its
            // own heading and a sub-nav tab.
            await expect(page.getByRole("heading", { level: 2, name: section, exact: true }), `${tab}: its page heading`).toBeVisible();
            await expect(sections.getByRole("tab", { selected: true }), `${tab}: no section tab is selected`).toHaveCount(0);
          } else {
            await expect(sections.getByRole("tab", { selected: true }), `${tab}: its section is selected`).toHaveText(section);
          }
          await page.waitForLoadState("networkidle");
          expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
          expect(await overflow(page), `${tab} @${width}: the page scrolls sideways`).toBeLessThanOrEqual(0);
          expect(await tablistOverflow(page), `${tab} @${width}: a tablist scrolls sideways`).toBeLessThanOrEqual(0);
          // (P4-NARR, C-852) The Channels page carries the progress switch, off by default.
          if (tab === "channels") {
            await expect(page.getByLabel("Show progress in chat")).not.toBeChecked();
          }
        }
        expect(errors).toEqual([]);
      });
    });
  }
}

test.describe("agent page width", () => {
  test.use({ viewport: { width: 1280, height: 900 } });

  test("the content is wider than the old 896 px cap at 1280 px", async ({ page }) => {
    await mockAgent(page);
    await page.goto("/o/acme/w/main/agents/daneel?tab=skills");
    // (D-408) A settings page is a 13rem sub-nav beside the page body: the content as a whole (sub-nav + body) is
    // what the old 896 px cap limited, and the body alone is no longer the whole width.
    const nav = page.getByRole("tablist", { name: "Settings pages" });
    const panel = page.getByRole("tabpanel").first();
    await expect(nav).toBeVisible();
    await expect(panel).toBeVisible();
    await page.waitForLoadState("networkidle");
    const width = await page.evaluate(
      ([a, b]) => b - a,
      await Promise.all([
        nav.evaluate((el) => el.getBoundingClientRect().left),
        panel.evaluate((el) => el.getBoundingClientRect().right),
      ]),
    );
    expect(width).toBeGreaterThan(896);
  });

  test("a hosted agent opens on Chat; switching section opens its first page and keeps the page id in the URL", async ({ page }) => {
    await mockAgent(page);
    await page.goto("/o/acme/w/main/agents/daneel");
    const sections = page.getByRole("tablist", { name: "Agent sections" });
    // (D-408) Three section tabs and a labelled Settings button.
    await expect(sections.getByRole("tab")).toHaveText(["Chat", "Automations", "Computer"]);
    await expect(sections.getByRole("tab", { name: "Chat" })).toHaveAttribute("aria-selected", "true");
    await sections.getByRole("tab", { name: "Automations" }).click();
    // (P5-REFINE-14) One page, no sub-tabs: its own heading and a filter by kind.
    await expect(page).toHaveURL(/tab=automations/);
    await expect(page.getByRole("tablist", { name: "Automations pages" })).toHaveCount(0);
    await expect(page.getByRole("heading", { level: 2, name: "Automations", exact: true })).toBeVisible();
    await expect(page.getByRole("radiogroup", { name: "Show" }).or(page.getByRole("heading", { name: "No automations yet" }))).toBeVisible();
    await page.getByRole("button", { name: "Settings" }).click();
    await expect(page).toHaveURL(/tab=instructions/);
    await page.getByRole("tablist", { name: "Settings pages" }).getByRole("tab", { name: "Other agents" }).click();
    await expect(page).toHaveURL(/tab=peers/);
  });

  // (P5-REFINE-15, D-408) The Computer opens on Files, with the one line of state above every sub-page; Details is last.
  test("the Computer opens on Files with a state line; the sub-nav ends with Details; old ?tab= ids still open", async ({ page }) => {
    await mockAgent(page);
    await page.goto("/o/acme/w/main/agents/daneel");
    await page.getByRole("tablist", { name: "Agent sections" }).getByRole("tab", { name: "Computer" }).click();
    await expect(page).toHaveURL(/tab=files/);
    const pages = page.getByRole("tablist", { name: "Computer pages" }).getByRole("tab");
    await expect(pages.first()).toHaveText("Files");
    await expect(pages.last()).toHaveText("Details");
    await expect(page.getByTestId("computer-state")).toBeVisible();
    await page.getByRole("tablist", { name: "Computer pages" }).getByRole("tab", { name: "Details" }).click();
    await expect(page).toHaveURL(/tab=computer/);
    await expect(page.getByTestId("computer-state")).toBeVisible();
    await expect(page.getByTestId("computer-facts")).toBeVisible();
    await page.goto("/o/acme/w/main/agents/daneel?tab=terminal");
    await expect(page.getByRole("tab", { name: "Terminal", selected: true })).toBeVisible();
  });
});
