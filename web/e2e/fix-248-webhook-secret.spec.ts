import { expect as baseExpect, test, type Page } from "@playwright/test";

import { serious } from "./support/axe";
import { overflow } from "./support/open";
import { openGallery, pickCard } from "./support/new-agent-templates";

// FIX-248: the one-time secret dialog at the end of the four-step page is not a bare code. It says what the secret
// is (what the webhook's sender must present so Aurora accepts the alert and wakes the agent), that it is shown only
// this once, where to paste it (from the webhook's verification), that the webhook starts Paused and where to turn it
// on. The Copy button gives feedback on every click (Copied for two seconds, then Copy again). The clipboard is read
// back with the browser's clipboard permissions. Themed, 1280 and 390 px, no horizontal scroll, axe clean.
const expect = baseExpect.configure({ timeout: 30_000 });

const SECRET = "whsec-grafana-248";

/** The create response with one Grafana-alert webhook, Bearer-verified and created paused (C-1111). */
const grafanaCreated = {
  agent: { id: "agent-12", name: "Rex", handle: "rex", kind: "hosted" },
  grants: [], notes: [], skills: [],
  webhooks: [{
    webhook: {
      id: "wh-70", name: "Grafana alert",
      verification: { kind: "bearer", header: "Authorization", format: "Bearer <secret>" },
      enabled: false,
    },
    url: "/v1/workspaces/ws1/webhooks/wh-70", secret: SECRET,
  }],
};

/** Overrides the shared create mock with the Grafana-alert webhook (later routes win). */
async function mockGrafanaCreate(page: Page) {
  await page.route(/\/v1\/workspaces\/[^/]+\/agents\/from-template$/, (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    return route.fulfill({ status: 201, contentType: "application/json", body: JSON.stringify(grafanaCreated) });
  });
}

/** Gallery → Use this template → the four steps → Create; the secret dialog is up. */
async function createRex(page: Page, scheme: "light" | "dark") {
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await openGallery(page, scheme);
  await mockGrafanaCreate(page);
  const dlg = page.getByRole("dialog", { name: "More templates" });
  await dlg.getByRole("button", { name: /Schedule manager/ }).click();
  await page.getByTestId("template-details").getByRole("button", { name: "Use this template" }).click();
  await expect(page).toHaveURL(/\/agents\/new\/schedule-manager$/);
  const view = page.getByTestId("new-agent-from-template");
  await view.getByLabel("Name").fill("Rex");
  await view.getByLabel("Handle (optional)").fill("rex");
  await view.getByLabel("Repository").fill("acme/town");
  await view.getByRole("button", { name: "Next" }).click();
  // Accounts: the existing GitHub account stands; Linear has none here, so it is connected through the
  // account-only journey (the same way the four-step journey spec does it); Notion is skipped.
  const linear = view.getByRole("region", { name: "It files cards." });
  await linear.getByRole("button", { name: "Connect Linear" }).click();
  const journey = page.getByTestId("ws-connect-journey");
  await pickCard(journey, /Connect another account/);
  await journey.getByRole("button", { name: "Next" }).click();
  await journey.getByLabel("Personal access token", { exact: true }).fill("glpat-not-a-real-token");
  await journey.getByRole("button", { name: "Connect", exact: true }).click();
  await expect(journey.getByText("Linear is connected. No agent can use it yet.")).toBeVisible();
  await journey.getByRole("button", { name: "Close", exact: true }).last().click();
  await pickCard(view.getByRole("region", { name: "It posts updates." }), /Skip Notion/);
  await view.getByRole("button", { name: "Next" }).click();
  await expect(view.getByRole("switch", { name: "Morning triage" })).toBeChecked();
  await view.getByRole("button", { name: "Next" }).click();
  await view.getByRole("button", { name: "Create" }).click();
  await expect(page.getByTestId("secret-key")).toHaveText(SECRET);
}

for (const width of [1280, 390]) {
  test.describe(`the webhook secret dialog (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light" as const, "dark" as const]) {
      test(`explains the secret, and Copy works on every click (${scheme})`, async ({ page }) => {
        const errors: string[] = [];
        page.on("pageerror", (e) => errors.push(e.message));
        page.on("console", (m) => {
          if (m.type() === "error" && !/^Failed to load resource/.test(m.text())) errors.push(m.text());
        });
        await createRex(page, scheme);

        // The heading names the webhook and the agent; the lead says what the secret is for.
        await expect(page.getByRole("heading", { name: "Secret for the webhook “Grafana alert” of Rex" })).toBeVisible();
        await expect(page.getByText("The sender of this webhook must present this secret with every alert, so Aurora knows the alert is genuine and wakes Rex.")).toBeVisible();
        // Why now, where it goes (Bearer token of the sender's contact point), and that it starts paused.
        await expect(page.getByText(/Copy it now: it is shown only this once and cannot be read again/)).toBeVisible();
        await expect(page.getByText(/Paste it into the sender as the Bearer token of its contact point/)).toBeVisible();
        await expect(page.getByText(/The webhook starts Paused/)).toBeVisible();
        await expect(page.getByRole("link", { name: "Rex's Automations page" })).toHaveAttribute("href", "/o/acme/w/main/agents/rex?tab=automations");
        expect(await overflow(page), "the dialog overflows horizontally").toBeLessThanOrEqual(0);
        expect(await page.getByRole("dialog").evaluate((el) => el.scrollWidth - el.clientWidth), "the secret dialog overflows").toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);

        // Every click copies and says so: Copied for about two seconds, then Copy again.
        const copy = page.getByRole("button", { name: "Copy", exact: true });
        await copy.click();
        await expect(page.getByRole("button", { name: "Copied" })).toBeVisible();
        expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(SECRET);
        await expect(page.getByRole("button", { name: "Copy again" })).toBeVisible({ timeout: 5_000 });
        await page.getByRole("button", { name: "Copy again" }).click();
        await expect(page.getByRole("button", { name: "Copied" })).toBeVisible();
        expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(SECRET);

        // A copy was made, so "I have saved it" closes at once and the agent's chat opens.
        await page.getByRole("button", { name: "I have saved it" }).click();
        await expect(page).toHaveURL(/\/agents\/rex$/);
        await expect(page.getByTestId("secret-key")).toHaveCount(0);
        expect(errors, "the journey threw").toEqual([]);
      });
    }
  });
}
