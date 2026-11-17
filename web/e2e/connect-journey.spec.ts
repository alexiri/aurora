import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";

import { openJourney } from "./support/journey";
import { overflow } from "./support/open";

// P5-UX-3 (C-914; docs/14 §Agents, accounts and access §3): "Give <Agent> <App>", the connect journey. From the agent's
// Access tab a token app is given with Read only in one go and the Access tab then lists the account with Read only; an
// app whose sign-in is not set up shows who can fix it and no form; from Apps & accounts in a one-agent workspace the
// agent is preselected; Another website or API ends with the agent able to use the website (one request, C-988).
// Both themes at 1280 px and 390 px, no horizontal scroll, axe with no rule disabled. Screenshots stay outside the repo.

const SHOTS = "/tmp/aurora-screens/p5-ux-3";
mkdirSync(SHOTS, { recursive: true });

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
}

async function check(page: Page, name: string, scheme: string, width: number) {
  expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
  await page.screenshot({ path: `${SHOTS}/${name}-${scheme}-${width}.png` });
  expect(await serious(page)).toEqual([]);
}

const AGENT_ACCESS = "/o/acme/w/main/agents/daneel?tab=access";
const ACCOUNTS = "/o/acme/w/main/apps?tab=accounts";

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("from the agent's Access tab: Give Daneel an app → GitHub → Read only → token → Connect → Done, and the tab lists it", async ({ page }) => {
        const saved = await openJourney(page, AGENT_ACCESS, scheme);
        await page.getByRole("button", { name: "Give Daneel an app" }).first().click();
        // The dialog's name follows the steps ("Give Daneel an app", then "Give Daneel GitHub"): hold on to the dialog itself.
        const dialog = page.getByTestId("ws-connect-journey");
        await expect(page.getByRole("dialog", { name: "Give Daneel an app" })).toBeVisible();
        await check(page, "app-step", scheme, width);
        await dialog.getByRole("button", { name: /^GitHub/ }).click();

        // Access first: Read only is preselected; the Account step is skipped (no GitHub account of this agent's to reuse).
        await expect(page.getByRole("dialog", { name: "Give Daneel GitHub" })).toBeVisible();
        const access = dialog.getByRole("radiogroup", { name: "What may Daneel do in GitHub?" });
        await expect(access.getByRole("radio", { name: /Read only/ })).toBeChecked();
        await check(page, "access-step", scheme, width);
        await dialog.getByRole("button", { name: "Next" }).click();

        // The token page as a button, then the permissions to tick for Read only.
        await expect(dialog.getByRole("link", { name: "Create a token on GitHub" })).toHaveAttribute("href", /github\.com\/settings/);
        await expect(dialog.getByText("For Read only, tick: Read your organizations")).toBeVisible();
        await dialog.getByLabel("Personal access token", { exact: true }).fill("not-a-real-token");
        await check(page, "sign-in-step", scheme, width);
        await dialog.getByRole("button", { name: "Connect", exact: true }).click();

        await expect(dialog.getByText("Daneel can now read GitHub.")).toBeVisible();
        expect(saved).toHaveLength(1);
        expect(saved[0]!.body).toMatchObject({ method_id: "token", scopes: ["read:org"], grant: { agent_ids: [expect.any(String)], level: "read" } });
        await check(page, "done-step", scheme, width);
        await expect(dialog.getByRole("link", { name: "Open chat" })).toBeVisible();
        await expect(dialog.getByRole("button", { name: "Give Daneel another app" })).toBeVisible();

        await dialog.getByRole("button", { name: "Done" }).click();
        await expect(dialog).toBeHidden();
        const row = page.getByRole("list", { name: "Accounts Daneel can use" }).getByRole("listitem", { name: "acme-prod" });
        await expect(row.getByRole("radio", { name: /Read only/ })).toBeChecked();
      });

      test("Ask before changes sends level ask and the write permissions", async ({ page }) => {
        const saved = await openJourney(page, "/o/acme/w/main/apps?tab=apps&connect=github", scheme);
        const dialog = page.getByRole("dialog", { name: "Give Daneel GitHub" });
        // From Apps & accounts the Agent step comes first; the workspace's one agent is already chosen.
        await dialog.getByRole("button", { name: "Give Daneel access" }).click();
        await dialog.getByText("Ask before changes", { exact: true }).click();
        await dialog.getByRole("button", { name: "Next" }).click();
        await dialog.getByLabel("Personal access token", { exact: true }).fill("not-a-real-token");
        await dialog.getByRole("button", { name: "Connect", exact: true }).click();
        await expect(dialog.getByText("Daneel can now read GitHub, and asks you before each change.")).toBeVisible();
        expect(saved[0]!.body).toMatchObject({ scopes: ["repo", "read:org"], grant: { level: "ask" } });
      });

      test("an app whose sign-in is not set up shows who can fix it and no form", async ({ page }) => {
        await openJourney(page, `${ACCOUNTS}&connect=`, scheme);
        const dialog = page.getByRole("dialog");
        await expect(dialog.getByRole("button", { name: "Sentry", exact: true })).toBeVisible();
        // The plan is read on focus, never all at once: Sentry moves to "Needs setup by an admin" once it is.
        await dialog.getByRole("button", { name: "Sentry", exact: true }).focus();
        const group = dialog.getByRole("region", { name: "Needs setup by an admin" });
        await expect(group.getByText("Sign-in is not set up.")).toBeVisible();
        await expect(group.getByText(/Ada Lovelace/)).toBeVisible();
        await expect(dialog.getByRole("textbox")).toHaveCount(0);
        await check(page, "needs-setup", scheme, width);
      });

      test("from Apps & accounts in a one-agent workspace the agent is preselected: Give Daneel access", async ({ page }) => {
        await openJourney(page, ACCOUNTS, scheme);
        await page.getByRole("button", { name: /^Add/ }).first().click();
        await page.getByRole("menuitem", { name: "Connect an app or website" }).click();
        const dialog = page.getByRole("dialog");
        await expect(dialog).toBeVisible();
        await dialog.getByRole("button", { name: /^GitHub/ }).click();
        // The Agent step: the workspace has one agent, so it is already chosen and the button says so.
        await expect(dialog.getByRole("checkbox", { name: /Daneel/ })).toBeChecked();
        await check(page, "agent-step", scheme, width);
        await dialog.getByRole("button", { name: "Give Daneel access" }).click();
        await expect(dialog.getByRole("radio", { name: /Read only/ })).toBeChecked();
      });

      test("Another website or API ends with the agent able to use the website, in one request", async ({ page }) => {
        const saved = await openJourney(page, `${ACCOUNTS}&connect=`, scheme);
        const dialog = page.getByRole("dialog");
        await dialog.getByRole("button", { name: "Another website or API" }).click();
        await dialog.getByRole("button", { name: "Give Daneel access" }).click();
        const form = page.getByRole("dialog", { name: "Give Daneel a website" });
        await form.getByLabel("Name", { exact: true }).fill("Weather");
        await form.getByLabel("Website", { exact: true }).fill("api.weather.example");
        await form.getByLabel("Token", { exact: true }).fill("not-a-real-token");
        await check(page, "website", scheme, width);
        await form.getByRole("button", { name: "Connect", exact: true }).click();
        await expect(form.getByRole("status")).toContainText("Daneel");
        expect(saved).toHaveLength(1);
        expect(saved[0]!.path).toMatch(/\/secrets$/);
        expect(JSON.stringify(saved[0]!.body)).toContain("api.weather.example");
        expect(JSON.stringify(saved[0]!.body)).toContain("agent");
      });
    });
  }
}
