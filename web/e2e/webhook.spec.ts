import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { overflow } from "./support/open";
import { openWebhook, SECRET_CANARY, SIGNING, URL_CANARY } from "./support/webhook";

// P5-WEBHOOK-4 (C-913) against the mocked API (P5-WEBHOOK-2 is not merged). Both themes at 1280 and 390 px, no horizontal
// scroll, axe with no serious violation. The dev-stack specs of the brief wait for P5-WEBHOOK-2: see webhook-stack.spec.ts.

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).slice(0, 4).join(" | ")}`);
}
async function check(page: Page) {
  expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
  expect(await serious(page)).toEqual([]);
}

const ACCOUNTS = "/o/acme/w/main/apps?tab=accounts";

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("connect with the suggested preset, preview, signing shown once, and the address is gone from the page", async ({ page }) => {
        const { sent } = await openWebhook(page, "/o/acme/w/main/apps?tab=apps&connect=webhook", scheme);
        const dialog = page.getByRole("dialog");
        await dialog.getByRole("button", { name: "Give Daneel access" }).click();
        await dialog.getByRole("button", { name: "Next" }).click();
        await dialog.getByLabel("Name", { exact: true }).fill("ops-alerts");
        await dialog.getByLabel("Address to send to", { exact: true }).fill(URL_CANARY);
        await check(page);
        // (P5-REFINE-13) Address, Message and Done are the journey's own steps: Back and Next are its footer, no inner wizard.
        await expect(dialog.getByRole("list", { name: "Steps" })).toContainText("Address");
        await dialog.getByRole("button", { name: "Next" }).click();
        await expect(dialog.getByText("This address looks like Slack.")).toBeVisible();
        await dialog.getByRole("button", { name: "Use the Slack preset" }).click();
        await dialog.getByRole("button", { name: "Preview" }).click();
        await expect(dialog.getByTestId("preview-body")).toContainText("Deploy 42 failed");
        await check(page);
        // The extras (secret values, headers, signing, limits) are in a fold on the Message step.
        await dialog.getByText("Secret values, headers, signing and limits").click();
        await dialog.getByLabel("Make one").check({ force: true });
        await check(page);
        await dialog.getByRole("button", { name: "Connect", exact: true }).click();
        const once = dialog.getByTestId("signing-secret-once");
        await expect(once).toContainText(SIGNING);
        await expect(once).toContainText("This will not be shown again.");
        await check(page);
        const connect = sent.find((s) => s.path.endsWith("/connect"))!;
        expect(connect.body).toMatchObject({ fields: { name: "ops-alerts", url: URL_CANARY }, webhook: { preset: "slack", signing: true } });
        // Never copied: closing asks first (SecretOnce), then "Close anyway" drops the secret.
        await once.getByRole("button", { name: "I have saved it" }).click();
        await expect(once).toContainText("You haven't copied it. Close anyway?");
        await once.getByRole("button", { name: "Close anyway" }).click();
        const html = await page.content();
        expect(html).not.toContain("CANARY");
        expect(html).not.toContain(SIGNING);
      });

      test("Done offers Send a test; a 405 is one plain sentence and the receiver's page is only in Details, cut short", async ({ page }) => {
        const { sent } = await openWebhook(page, "/o/acme/w/main/apps?tab=apps&connect=webhook", scheme, { testStatus: 405 });
        const dialog = page.getByRole("dialog");
        await dialog.getByRole("button", { name: "Give Daneel access" }).click();
        await dialog.getByRole("button", { name: "Next" }).click();
        await dialog.getByLabel("Address to send to", { exact: true }).fill("https://hooks.slack.com/services/T0/B0/x");
        await dialog.getByLabel("Name", { exact: true }).fill("Ops alerts");
        await dialog.getByRole("button", { name: "Next" }).click();
        await dialog.getByRole("button", { name: "Use the Slack preset" }).click();
        await dialog.getByRole("button", { name: "Connect", exact: true }).click();
        await expect(dialog.getByText("Not sure the address works?")).toBeVisible();
        await dialog.getByRole("button", { name: "Send a test" }).click();
        await expect(dialog.getByText("The test did not go through")).toBeVisible();
        await expect(dialog.getByText("The address doesn't accept POST. Check the URL and try again.")).toBeVisible();
        // The receiver's page is text under Details, never rendered, and only its first 200 characters.
        await expect(dialog.getByTestId("test-body")).toBeHidden();
        await dialog.getByText("Details", { exact: true }).click();
        await expect(dialog.getByTestId("test-body")).toContainText("(first 200 characters)");
        expect(await page.evaluate(() => (window as unknown as { hacked?: number }).hacked)).toBeUndefined();
        await check(page);
        expect(sent.filter((s) => s.path.endsWith("/test"))).toHaveLength(1);
        await expect(dialog.getByRole("button", { name: "Edit address" })).toBeVisible();
      });

      test("a preview refusal marks the template; connecting from an example makes the {{text}} slot", async ({ page }) => {
        await openWebhook(page, "/o/acme/w/main/apps?tab=apps&connect=webhook", scheme);
        const dialog = page.getByRole("dialog");
        await dialog.getByRole("button", { name: "Give Daneel access" }).click();
        await dialog.getByRole("button", { name: "Next" }).click();
        await dialog.getByLabel("Name", { exact: true }).fill("ops-alerts");
        await dialog.getByLabel("Address to send to", { exact: true }).fill("https://example.org/hook");
        await dialog.getByRole("button", { name: "Next" }).click();
        await dialog.getByText("Start from an example").click();
        await dialog.getByLabel("The example body").fill('{"username": "Aurora", "text": "Hello"}');
        await dialog.getByRole("button", { name: "Show the example" }).click();
        await dialog.getByRole("button", { name: "Agent fills in text" }).click();
        await expect(dialog.getByTestId("example-template")).toContainText('"text": {{text}}');
        await check(page);
        await dialog.getByRole("button", { name: "Use this template" }).click();
        await expect(dialog.getByLabel("Name", { exact: true }).and(page.locator("#slot-0-name"))).toHaveValue("text");
        await check(page);
      });

      test("the card shows the effective limit; Edit changes a slot; Send test asks first", async ({ page }) => {
        const { sent, errors } = await openWebhook(page, ACCOUNTS, scheme, { withAccount: true });
        const row = page.getByRole("listitem", { name: "ops-alerts" });
        await expect(row.getByTestId("webhook-limit")).toHaveText("30 a minute, 500 a day (your organization's limit)");
        await expect(row.getByText("hooks.slack.com")).toBeVisible();
        await check(page);

        await row.getByRole("button", { name: "Edit ops-alerts" }).click();
        const edit = page.getByRole("dialog", { name: "Edit ops-alerts" });
        await edit.locator("#slot-0-desc").fill("The alert text");
        await check(page);
        await edit.getByRole("button", { name: "Next" }).click();
        await expect(edit.getByTestId("wh-effective")).toContainText("30 a minute and 500 a day");
        await edit.getByRole("button", { name: "Save changes" }).click();
        await expect(edit).toBeHidden();
        expect(errors, "page errors after saving").toEqual([]);
        const put = sent.find((s) => s.method === "PUT")!;
        expect((put.body as { slots: { description: string }[] }).slots[0]!.description).toBe("The alert text");

        await row.getByRole("button", { name: "Send test to ops-alerts" }).click();
        const test = page.getByRole("dialog", { name: "Send test: ops-alerts" });
        await test.getByRole("button", { name: "Dry run" }).click();
        await expect(test.getByText("Dry run: nothing was sent.")).toBeVisible();
        await test.getByRole("button", { name: "Send", exact: true }).click();
        const confirm = page.getByRole("dialog", { name: /Send a test to ops-alerts/ });
        await expect(confirm).toContainText("This really calls the webhook");
        expect(sent.filter((s) => s.path.endsWith("/test") && !s.body?.dry_run)).toHaveLength(0);
        await confirm.getByRole("button", { name: "Send", exact: true }).click();
        await expect(test.getByText("The test went through.")).toBeVisible();
        await check(page);
        void SECRET_CANARY;
      });

      test("the organization's Apps tab has the switch and limits; Admin › Advanced shows the settings section", async ({ page }) => {
        const { sent } = await openWebhook(page, "/o/acme/apps?tab=apps", scheme);
        await page.getByLabel(/Sends per minute/).fill("10");
        await page.getByRole("button", { name: "Save", exact: true }).click();
        await expect(page.getByText("Saved", { exact: true })).toBeVisible();
        expect(sent.find((s) => s.method === "PATCH")!.body).toMatchObject({ settings: { outgoing_webhooks_per_minute_max: 10 } });
        await check(page);
        await openWebhook(page, "/admin/apps?tab=advanced", scheme);
        await expect(page.getByRole("heading", { name: "Outgoing webhooks" }).first()).toBeVisible();
        await check(page);
      });
    });
  }
}
