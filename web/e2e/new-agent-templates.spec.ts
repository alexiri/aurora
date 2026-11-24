import AxeBuilder from "@axe-core/playwright";
import { expect as baseExpect, test } from "@playwright/test";

import { LIBRARY_SHA, openGallery, pickCard } from "./support/new-agent-templates";
import { CANVAS, overflow } from "./support/open";

// The first request of a route compiles it in `next dev`; on a loaded host that outlasts the default 5 s.
const expect = baseExpect.configure({ timeout: 30_000 });

// P6-TPL-4 Part B1 (C-1116, D-458): **More templates…** in the New agent dialog opens the gallery of the
// workspace's templates; choosing a card opens its details first — the README rendered as inert data, what it
// needs, what it runs on its own, its helper files by name and the version — before **Use this template**. A
// card whose needs are not met says so and cannot be used. The gallery is themed in both themes, never scrolls
// horizontally at 390 px, and has no serious axe violations. The API is mocked.

for (const width of [1280, 390]) {
  test.describe(`template gallery (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      test(`gallery: themed, no horizontal page scroll, accessible (${scheme})`, async ({ page }) => {
        await openGallery(page, scheme);
        expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
        expect(await overflow(page), "gallery overflows horizontally").toBeLessThanOrEqual(0);
        const dlg = page.getByRole("dialog", { name: "More templates" });
        // Dialogs scroll inside themselves; nothing in them may be wider than the dialog.
        expect(await dlg.evaluate((el) => el.scrollWidth - el.clientWidth), "gallery dialog overflows").toBeLessThanOrEqual(0);
        const { violations } = await new AxeBuilder({ page }).analyze();
        const bad = violations
          .filter((v) => v.impact === "serious" || v.impact === "critical")
          .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
        expect(bad).toEqual([]);
      });
    }

    test("the card → details journey: what it needs, runs on its own, helper files, inert README", async ({ page }) => {
      await openGallery(page, "light");
      const dlg = page.getByRole("dialog", { name: "More templates" });
      // The cards: name, version, what the template runs on its own, and the not-ready sentences.
      await expect(dlg.getByText("Schedule manager")).toBeVisible();
      await expect(dlg.getByText("Version 1.0.0")).toBeVisible();
      await expect(dlg.getByText("Version 1.1.0")).toBeVisible();
      await expect(dlg.getByText("Schedule Morning triage")).toBeVisible();
      await expect(dlg.getByText("Webhook Alerts, created paused")).toBeVisible();
      await expect(dlg.getByText("Needs GitHub, which isn't available here.")).toBeVisible();
      // Choosing a card opens its details first (D-458).
      await dlg.getByRole("button", { name: /Schedule manager/ }).click();
      const details = page.getByTestId("template-details");
      await expect(details.getByRole("heading", { level: 3, name: "Schedule manager" }).first()).toBeVisible();
      const readme = page.getByTestId("template-readme");
      await expect(readme.getByText("<img src=x onerror=alert(1)>")).toBeVisible();
      expect(await readme.locator("img").count()).toBe(0);
      expect(await readme.locator("a[href^='javascript:']").count()).toBe(0);
      await expect(readme.getByText("remote picture")).toBeVisible();
      await expect(readme.getByText("docs")).toBeVisible();
      await expect(readme.getByRole("heading", { level: 3, name: "Schedule manager" })).toBeVisible();
      await expect(details.getByText("GitHub — required")).toBeVisible();
      await expect(details.getByText("Schedule Morning triage")).toBeVisible();
      await expect(details.getByText("triage.py")).toBeVisible();
      await expect(details.getByText("Version 1.0.0")).toBeVisible();
      // Back returns to the gallery (the dialog header's arrow is the first Back control).
      await page.getByRole("button", { name: "Back", exact: true }).first().click();
      await expect(page.getByRole("dialog", { name: "More templates" }).getByText("Schedule manager")).toBeVisible();
    });

    test("a card whose needs are not met opens details that say so and cannot be used (D-470)", async ({ page }) => {
      await openGallery(page, "light");
      const dlg = page.getByRole("dialog", { name: "More templates" });
      await dlg.getByRole("button", { name: /Incident scribe/ }).click();
      const details = page.getByTestId("template-details");
      // The details header is the first level-3 heading; the README's own `# …` heading is inside the readme.
      await expect(details.getByRole("heading", { level: 3, name: "Incident scribe" }).first()).toBeVisible();
      await expect(details.getByText("Needs GitHub, which isn't available here.")).toBeVisible();
      await expect(details.getByRole("button", { name: "Use this template" })).toBeDisabled();
    });
  });
}

const SHOTS = "/tmp/aurora-screens/p6-tpl-4";

for (const width of [1280, 390]) {
  test.describe(`the four-step page (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      // The whole journey (C-1116): gallery → details → About → Accounts (one existing account, one connected
      // through the account-only journey, one optional skipped) → Automations → Review → Create → the one-time
      // webhook secret → the new agent's chat. The create request is checked against the steps.
      test(`gallery → details → the four steps → Create lands in the new agent's chat (${scheme})`, async ({ page }) => {
        const errors: string[] = [];
        page.on("pageerror", (e) => errors.push(e.message));
        page.on("console", (m) => {
          if (m.type() === "error" && !/^Failed to load resource/.test(m.text())) errors.push(m.text());
        });
        const mock = await openGallery(page, scheme);
        const dlg = page.getByRole("dialog", { name: "More templates" });
        await dlg.getByRole("button", { name: /Schedule manager/ }).click();
        await page.getByTestId("template-details").getByRole("button", { name: "Use this template" }).click();
        await expect(page).toHaveURL(/\/agents\/new\/schedule-manager$/);
        const view = page.getByTestId("new-agent-from-template");

        // About: the agent's name, an optional handle and the template's input.
        await view.getByLabel("Name").fill("Rex");
        await view.getByLabel("Handle (optional)").fill("rex");
        await view.getByLabel("Repository").fill("acme/town");
        await page.screenshot({ path: `${SHOTS}/new-agent-about-${scheme}-${width}.png` });
        await view.getByRole("button", { name: "Next" }).click();

        // Accounts: the existing GitHub account is the recommended choice; Linear has none here, so it is
        // connected through the account-only journey; the optional Notion entry is skipped.
        const github = view.getByRole("region", { name: "It triages pull requests." });
        await expect(github.getByRole("radio", { name: /Use acme-bot/ })).toBeChecked();
        const linear = view.getByRole("region", { name: "It files cards." });
        // No Linear account exists here: the entry stands on "Connect a new account", and the button is the way in.
        await expect(linear.getByRole("radio", { name: /Connect a new Linear account/ })).toBeChecked();
        await linear.getByRole("button", { name: "Connect Linear" }).click();
        const journey = page.getByTestId("ws-connect-journey");
        // The workspace has a Linear account already ("Product team"): the journey asks which account first.
        await pickCard(journey, /Connect another account/);
        await journey.getByRole("button", { name: "Next" }).click();
        await journey.getByLabel("Personal access token", { exact: true }).fill("glpat-not-a-real-token");
        await page.screenshot({ path: `${SHOTS}/new-agent-connect-${scheme}-${width}.png` });
        await journey.getByRole("button", { name: "Connect", exact: true }).click();
        await expect(journey.getByText("Linear is connected. No agent can use it yet.")).toBeVisible();
        // The footer's Close (the dialog header's arrow-like ✕ has the same accessible name).
        await journey.getByRole("button", { name: "Close", exact: true }).last().click();
        await expect(journey).toBeHidden();
        // The refetched plan resolves the account the journey made; the entry uses it.
        await expect(linear.getByRole("radio", { name: /Use Linear · acme/ })).toBeChecked();
        await pickCard(view.getByRole("region", { name: "It posts updates." }), /Skip Notion/);
        await page.screenshot({ path: `${SHOTS}/new-agent-accounts-${scheme}-${width}.png` });
        await view.getByRole("button", { name: "Next" }).click();

        // Automations: the schedule runs on the template's default (on), in the workspace's zone; the webhook is
        // listed as created paused, with no switch of its own.
        await expect(view.getByRole("switch", { name: "Morning triage" })).toBeChecked();
        await expect(view.getByText("Every weekday at 09:00 · UTC")).toBeVisible();
        await expect(view.getByText(/Created paused\. Post alerts to the room\./)).toBeVisible();
        await page.screenshot({ path: `${SHOTS}/new-agent-automations-${scheme}-${width}.png` });
        await view.getByRole("button", { name: "Next" }).click();

        // Review: what Rex can do (C-986), the parts as inert data, the library skills at their commit.
        await expect(view.getByText("Rex can read acme-bot and Linear.")).toBeVisible();
        await expect(view.getByText("It asks you before changing anything in acme-bot.")).toBeVisible();
        await expect(view.getByText("It can reach any website.")).toBeVisible();
        await expect(view.getByTestId("ref-0").getByText("From Team skills (github.com/acme/skills) at abcdef1")).toBeVisible();
        await expect(view.getByText("Not added: needs the Reports skill, which isn't available here.")).toBeVisible();
        await page.screenshot({ path: `${SHOTS}/new-agent-review-${scheme}-${width}.png` });
        await view.getByRole("button", { name: "Create" }).click();

        // The new webhook's secret is shown exactly once, then the agent's chat opens.
        const secret = page.getByTestId("secret-key");
        await expect(secret).toHaveText("whsec-9f8e7d6c5b4a3210");
        expect(await secret.count()).toBe(1);
        await page.screenshot({ path: `${SHOTS}/new-agent-secret-${scheme}-${width}.png` });
        await page.getByRole("button", { name: "I have saved it" }).click();
        // Nothing was copied, so the dialog asks first; leave anyway.
        await page.getByRole("button", { name: "Close anyway" }).click();
        await expect(page).toHaveURL(/\/agents\/rex$/);
        await expect(page.getByRole("heading", { level: 1, name: "Rex" })).toBeVisible();
        await expect(page.getByTestId("secret-key")).toHaveCount(0);
        await expect(page.getByTestId("undo-toast")).toContainText("Rex is ready");

        // The create request is the four steps, exactly: the skipped entry sends nothing, unchanged access levels
        // are left out, the library skills carry their commit, the webhook is not in the schedules.
        expect(mock.writes).toEqual([
          {
            key: "POST /v1/workspaces/{ws}/agents/from-template",
            body: {
              template_id: "schedule-manager", version: "1.0.0", content_sha256: "c".repeat(64),
              name: "Rex", handle: "rex", inputs: { repo: "acme/town" },
              accounts: [
                { entry: 0, app_id: "github", connection_id: "00000000-0000-4000-8000-000000000040" },
                { entry: 1, app_id: "linear", connection_id: "00000000-0000-4000-8000-000000000042" },
              ],
              skill_refs: [
                { index: 0, library_id: "lib1", commit_sha: LIBRARY_SHA },
                { index: 1, library_id: "lib1", commit_sha: LIBRARY_SHA },
              ],
              automations: [{ kind: "schedule", name: "Morning triage", enabled: true }],
            },
          },
        ]);
        expect(errors, "the four-step journey threw").toEqual([]);
        expect(await overflow(page), "the agent's page overflows horizontally").toBeLessThanOrEqual(0);
      });
    }
  });
}
