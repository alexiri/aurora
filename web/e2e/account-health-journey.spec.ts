import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { DEFAULT, MINE, openChat } from "./support/chat-mock";
import { openJourney } from "./support/journey";
import { id, now, probed } from "./support/mock-api";
import { overflow, openRoute } from "./support/open";

// P5-UX-14 (D-335, D-350, C-943, C-945; docs/14 §Agents, accounts and access §7), where an account's health shows besides its
// row: the warning on the connect journey's Done step (a method with no probe is tested once after connecting, a method
// with a probe is not), the banner above the message box of the agent's conversations, and the workspace Overview's block.
// Both themes at 1280 px and 390 px, no horizontal scroll, axe (serious and critical) clean.

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
}

async function check(page: Page) {
  expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
  expect(await serious(page)).toEqual([]);
}

const AGENT_ACCESS = "/o/acme/w/main/agents/daneel?tab=access";
const NEW_ACCOUNT_TEST = `/v1/accounts/connection/${id(500)}/test`;

/** Give Daneel GitHub from the Access tab with a token, up to the Done step. */
async function connectGitHub(page: Page) {
  await page.getByRole("button", { name: "Give Daneel an app" }).first().click();
  const dialog = page.getByTestId("ws-connect-journey");
  await dialog.getByRole("button", { name: /^GitHub/ }).click();
  await dialog.getByRole("button", { name: "Next" }).click();
  await dialog.getByLabel("Personal access token", { exact: true }).fill("not-a-real-token");
  await dialog.getByRole("button", { name: "Connect", exact: true }).click();
  await expect(dialog.getByText("Daneel can now read GitHub.")).toBeVisible();
  return dialog;
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("a method with no probe is tested once after connecting, and a failure is a warning: the account is kept", async ({ page }) => {
        await openJourney(page, AGENT_ACCESS, scheme);
        let tests = 0;
        await page.route(`**${NEW_ACCOUNT_TEST}`, (route) => {
          tests += 1;
          return route.fulfill({
            status: 200, contentType: "application/json",
            body: JSON.stringify({ result: "unreachable", detail: { host: "db.platform.example.org" }, checked_at: now }),
          });
        });
        const dialog = await connectGitHub(page);
        await expect(dialog.getByRole("alert")).toHaveText("Saved, but GitHub could not be reached (db.platform.example.org). Check the address or test again.");
        expect(tests).toBe(1);
        // The account is kept: the success sentence and the next steps stay.
        await expect(dialog.getByRole("link", { name: "Open chat" })).toBeVisible();
        await check(page);
      });

      test("a method with a probe was checked while connecting: no second test, no warning", async ({ page }) => {
        await openJourney(page, AGENT_ACCESS, scheme);
        probed.add("token");
        let tests = 0;
        await page.route(`**${NEW_ACCOUNT_TEST}`, (route) => {
          tests += 1;
          return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ result: "unreachable", checked_at: now }) });
        });
        const dialog = await connectGitHub(page);
        await expect(dialog.getByRole("link", { name: "Open chat" })).toBeVisible();
        await expect(dialog.getByRole("alert")).toHaveCount(0);
        expect(tests).toBe(0);
      });

      test("a conversation shows a banner above the message box for an account whose sign-in expired, with Reconnect", async ({ page }) => {
        await openChat(page, scheme, DEFAULT, `?c=${MINE.id}`);
        const banner = page.getByRole("list", { name: "Accounts that need attention" });
        await expect(banner).toContainText("Daneel can't use Linear: the sign-in expired.");
        await expect(banner.getByRole("link", { name: "Reconnect" })).toHaveAttribute("href", /\/agents\/[^?]+\?tab=access&reconnect=/);
        // Above the message box, not below it.
        const above = await page.evaluate(() => {
          const b = document.querySelector('[aria-label="Accounts that need attention"]');
          const i = document.querySelector("#chat-input");
          return !!b && !!i && !!(b.compareDocumentPosition(i) & Node.DOCUMENT_POSITION_FOLLOWING);
        });
        expect(above).toBe(true);
        await check(page);
      });

      test("the workspace Overview's Needs you card lists the accounts that need attention with the row's line and fix", async ({ page }) => {
        await openRoute(page, "/o/acme/w/main", scheme);
        // (P5-UX-7) The account block lives in the Overview's "Needs you" card, beside the approvals.
        const block = page.getByRole("region", { name: /^Needs you/ });
        await expect(block).toContainText("Product team");
        await expect(block).toContainText("The sign-in to Linear expired");
        await expect(block.getByRole("link", { name: "Reconnect" })).toHaveAttribute("href", /\/apps\?tab=accounts&reconnect=/);
        await check(page);
      });
    });
  }
}
