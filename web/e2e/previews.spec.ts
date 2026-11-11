import { expect, test, type Page } from "@playwright/test";

import { agent, mockApi, org } from "./support/mock-api";
import { CANVAS, overflow, type Scheme } from "./support/open";

// P4-PRV-1 (docs/14 §Previews, docs/18 §Port previews): the agent Previews tab and the org
// settings switch, against the shared mock. Tab presence follows the switches: instance
// `features.previews` AND the org's `previews_enabled`. Both themes, no horizontal scroll at 390 px.

/** The shared mock with the org switch off (or a given role): later routes win over it. */
async function mockWith(page: Page, over: { orgSettings?: Record<string, unknown> }) {
  await mockApi(page);
  await page.route("**/v1/**", async (route) => {
    const p = new URL(route.request().url()).pathname;
    const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
    if (over.orgSettings && p === `/v1/orgs/${org.id}`) return json({ ...org, settings: over.orgSettings });
    return route.fallback();
  });
}

async function openTab(page: Page, scheme: Scheme = "light") {
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto("/o/acme/w/main/agents/daneel?tab=previews");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await page.waitForLoadState("networkidle");
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`agent Previews tab, themed ${width} px (${scheme})`, () => {
      test.use({ viewport: { width, height: 800 } });
      test("tab open, themed canvas, no horizontal scroll", async ({ page }) => {
        await mockApi(page);
        await openTab(page, scheme);
        expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
      });
    });
  }
}

test.describe("agent Previews tab", () => {
  test.use({ viewport: { width: 1280, height: 900 } });

  test("lists the shares with status and actions, opens the add dialog with port chips", async ({ page }) => {
    await mockApi(page);
    await openTab(page);
    const table = page.getByRole("table", { name: "Preview shares" });
    await expect(table).toBeVisible();
    // The URL is monospace text with Copy and Open beside it; Open opens a new tab safely.
    const open = table.getByRole("link", { name: "Open" });
    await expect(open).toHaveAttribute("rel", "noopener noreferrer");
    await expect(open).toHaveAttribute("target", "_blank");
    // The status is a word, never colour alone; the running command opens the last-start dialog.
    await expect(table.getByText("Listening")).toBeVisible();
    await table.getByRole("button", { name: "npm run dev" }).click();
    const lastStart = page.getByRole("dialog", { name: "Port 3000 — last start" });
    await expect(lastStart).toContainText("ready on 3000");
    await lastStart.getByRole("button", { name: "Close" }).click();

    await page.getByRole("button", { name: "Add share" }).click();
    const dlg = page.getByRole("dialog", { name: "Share a port" });
    await expect(dlg).toBeVisible();
    // The currently listening ports are offered as chips (from /sandbox); one fills the port field.
    await dlg.getByRole("button", { name: "3000", exact: true }).click();
    await expect(dlg.getByLabel("Port")).toHaveValue("3000");
    await dlg.getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByRole("dialog", { name: "Share a port" })).toHaveCount(0);
  });

  test("without the org switch there is no tab, and the Computer port list has no Share action", async ({ page }) => {
    await mockWith(page, { orgSettings: {} });
    await page.goto("/o/acme/w/main/agents/daneel");
    const tabs = page.getByRole("tablist", { name: "Agent" });
    await expect(tabs.getByRole("tab", { name: "Previews" })).toHaveCount(0);
    await tabs.getByRole("tab", { name: "Computer" }).click();
    const ports = page.getByRole("table", { name: "Listening ports" });
    await expect(ports).toBeVisible();
    await expect(ports.getByRole("button", { name: /Share/ })).toHaveCount(0);
    // The port is still marked shared in words (the badge, not the column header).
    await expect(ports.getByRole("cell", { name: "Shared" }).getByText("Shared")).toBeVisible();
  });

  test("a BYO agent has no Previews tab even with previews on", async ({ page }) => {
    await mockApi(page);
    await page.route("**/v1/**", async (route) => {
      const p = new URL(route.request().url()).pathname;
      if (p.endsWith("/agents")) {
        return route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ items: [{ ...agent, kind: "byo" }], next_cursor: null }),
        });
      }
      return route.fallback();
    });
    await page.goto("/o/acme/w/main/agents/daneel");
    await expect(page.getByRole("tablist", { name: "Agent" }).getByRole("tab", { name: "Previews" })).toHaveCount(0);
  });
});

test.describe("org settings Previews switch", () => {
  test.use({ viewport: { width: 1280, height: 900 } });

  test("the switch card shows the domain and turning it off asks for confirmation", async ({ page }) => {
    await mockApi(page);
    await page.goto("/o/acme/settings");
    const card = page.getByRole("region", { name: "Port previews" });
    await expect(card).toBeVisible();
    // The domain is read-only.
    await expect(card.getByLabel("Preview domain")).toHaveValue("previews.example.org");
    await expect(card.getByLabel("Preview domain")).toHaveAttribute("aria-readonly", "true");
    // Off asks for the confirmation dialog; Confirm patches the org, nothing sent before.
    let patched: unknown = null;
    await page.route("**/v1/**", async (route) => {
      const req = route.request();
      if (req.method() === "PATCH" && new URL(req.url()).pathname === `/v1/orgs/${org.id}`) {
        patched = req.postDataJSON();
        return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ ...org, settings: { previews_enabled: false } }) });
      }
      return route.fallback();
    });
    // Unchecking asks for the confirmation dialog first; the checkbox only changes after it.
    await card.getByRole("checkbox", { name: "Allow port previews in this organization" }).click();
    const confirm = page.getByRole("dialog", { name: "Turn port previews off?" });
    await expect(confirm).toContainText("Existing links stop working");
    await confirm.getByRole("button", { name: "Turn off" }).click();
    await expect.poll(() => patched).toEqual({ settings: { previews_enabled: false } });
  });
});
