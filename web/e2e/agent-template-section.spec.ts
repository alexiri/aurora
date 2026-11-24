import AxeBuilder from "@axe-core/playwright";
import { expect as baseExpect, test, type Page } from "@playwright/test";

import { agent, mockApi, ws } from "./support/mock-api";
import { CANVAS, overflow } from "./support/open";

const expect = baseExpect.configure({ timeout: 30_000 });

// Surface token values from globals.css (`--surface`), as computed `rgb()` strings — the dialog's background.
const SURFACE: Record<"light" | "dark", string> = { light: "rgb(255, 255, 255)", dark: "rgb(18, 24, 41)" };

// P6-TPL-4 B2 (C-1117, C-1143): the agent's Template section on Settings › Manage — "Made from
// <template> <version>", the helper files with View (path, size, Executable, the text as plain
// text) and Remove (one or all, with a confirm), and the apply journey (C-1112): Review changes
// with the README fold, the parts' states and checkboxes, then Apply of exactly the ticked parts,
// bound to the shown hash. Themed in both themes (the dialog included), usable at 390 px without
// horizontal scroll, no serious axe violations. The API is mocked.

const SHA = "a".repeat(64);

const helpers = {
  items: [
    { path: "tidy.sh", size: 24, executable: true, sha256: SHA },
    { path: "notes.md", size: 120, executable: false, sha256: "b".repeat(64) },
  ],
};

const overrides: Record<string, unknown> = {
  // The agent is made from a template, so the section shows.
  [`/v1/workspaces/${ws.id}/agents`]: {
    items: [{ ...agent, template: { template_id: "schedule-manager", name: "Schedule manager", version: "1.0.0" } }],
  },
  [`/v1/agents/${agent.id}/template`]: {
    template: { template_id: "schedule-manager", name: "Schedule manager", version: "1.0.0" },
    available: true,
    notes: [],
    update: null,
  },
  [`/v1/agents/${agent.id}/helpers`]: helpers,
};

// (C-1112) The update offer the apply journey works through: the instructions were edited here, the
// helper files changed upstream, and a skill is new.
const OFFER_SHA = "c".repeat(64);
const update = {
  version: "1.1.0",
  content_sha256: OFFER_SHA,
  readme: "# New in 1.1.0\n\nQuieter, and it greets.",
  changelog: null,
  inputs_needed: [],
  apps_added: [],
  parts: [
    { kind: "instructions", key: "", state: "edited_here", current: "Triage the team's pull requests.", proposed: "Triage the team's pull requests quietly.", skill_ref: null },
    { kind: "helpers", key: "", state: "unchanged_here", current: "tidy.sh, notes.md", proposed: "tidy.sh, notes.md, greet.md", skill_ref: null },
    { kind: "skill", key: "greet", state: "new", current: null, proposed: "Say hello once a day.", skill_ref: null },
  ],
};

const helpersAfter = {
  items: [
    { path: "tidy.sh", size: 24, executable: true, sha256: SHA },
    { path: "notes.md", size: 120, executable: false, sha256: "b".repeat(64) },
    { path: "greet.md", size: 32, executable: false, sha256: "c".repeat(64) },
  ],
};

async function open(page: Page, scheme: "light" | "dark", extra: Record<string, unknown> = {}) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => {
    if (m.type() === "error" && !/^Failed to load resource/.test(m.text())) errors.push(m.text());
  });
  await mockApi(page, { ...overrides, ...extra });
  // Registered after mockApi, so it matches first.
  await page.route(`**/v1/agents/${agent.id}/helpers/content*`, (route) => {
    const path = new URL(route.request().url()).searchParams.get("path") ?? "";
    return route.fulfill({ status: 200, contentType: "application/octet-stream", body: path === "tidy.sh" ? "#!/bin/sh\necho tidied\n" : "# notes\n" });
  });
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(`/o/acme/w/main/agents/daneel?tab=manage`);
  await page.locator("h1, h2").first().waitFor();
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  return errors;
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`template section (${scheme}, ${width} px)`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("shows the section; the helper dialog is themed, inert and accessible", async ({ page }) => {
        const errors = await open(page, scheme);
        const section = page.getByTestId("agent-template-section");
        await expect(section).toBeVisible();
        await expect(section.getByTestId("template-made-from")).toHaveText("Made from Schedule manager 1.0.0");
        await expect(section.getByTestId("helper-count")).toHaveText("2 helper files in .aurora/helpers");
        // View: the files as data; the text arrives as plain text (SEC-146).
        await section.getByRole("button", { name: "View" }).click();
        const dlg = page.getByRole("dialog", { name: "Helper files" });
        await expect(dlg.getByTestId("helper-tidy.sh").locator("summary")).toContainText("24 bytes · Executable");
        await dlg.getByTestId("helper-tidy.sh").locator("summary").click();
        await expect(dlg.getByLabel("tidy.sh (executable) as text")).toHaveText("#!/bin/sh\necho tidied\n");
        // Dialogs scroll inside themselves; nothing in them may be wider than the dialog.
        expect(await dlg.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(0);
        const { violations } = await new AxeBuilder({ page }).analyze();
        const bad = violations
          .filter((v) => v.impact === "serious" || v.impact === "critical")
          .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
        expect(bad).toEqual([]);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
        expect(errors).toEqual([]);
      });

      test("removes one helper file after its confirm, then all of them", async ({ page }) => {
        const errors = await open(page, scheme);
        const section = page.getByTestId("agent-template-section");
        await section.getByRole("button", { name: "View" }).click();
        const dlg = page.getByRole("dialog", { name: "Helper files" });
        await dlg.getByTestId("helper-tidy.sh").locator("..").getByRole("button", { name: "Remove" }).click();
        const one = page.getByRole("dialog", { name: "Remove tidy.sh?" });
        await expect(one).toContainText("If a template update offers it again, applying it brings it back.");
        await one.getByRole("button", { name: "Remove file" }).click();
        await expect(page.getByRole("dialog", { name: "Remove tidy.sh?" })).toHaveCount(0);
        // All of them: the section's Remove, with the confirm that says what an update does.
        await dlg.getByRole("button", { name: "Close" }).last().click();
        await section.getByRole("button", { name: "Remove" }).click();
        const all = page.getByRole("dialog", { name: "Remove all helper files?" });
        await expect(all).toContainText("The 2 files in .aurora/helpers are removed from Daneel.");
        await all.getByRole("button", { name: "Remove all" }).click();
        await expect(all).toHaveCount(0);
        expect(errors).toEqual([]);
      });

      test("applies part of an update, the helper files included (C-1112, C-1117)", async ({ page }) => {
        let applied: unknown = null;
        const errors = await open(page, scheme, {
          [`/v1/agents/${agent.id}/template`]: {
            template: { template_id: "schedule-manager", name: "Schedule manager", version: "1.0.0" },
            available: true,
            notes: [],
            update,
          },
        });
        // Registered after mockApi, so they match first. The apply answers with the new status; the
        // helper list reflects what the applied version brought.
        await page.route(`**/v1/agents/${agent.id}/template/apply`, (route) => {
          applied = route.request().postDataJSON();
          return route.fulfill({
            status: 200,
            contentType: "application/json",
            body: JSON.stringify({ template: { template_id: "schedule-manager", name: "Schedule manager", version: "1.1.0" }, available: true, notes: [], update: null }),
          });
        });
        await page.route(`**/v1/agents/${agent.id}/helpers`, (route) =>
          route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(applied ? helpersAfter : helpers) }),
        );
        const section = page.getByTestId("agent-template-section");
        await expect(section.getByTestId("template-update")).toHaveText("Version 1.1.0 is available. Daneel keeps working as it is until you apply it.");
        await section.getByRole("button", { name: "Review changes" }).click();
        const dlg = page.getByRole("dialog", { name: "Review changes" });
        // The README is a fold — shown, never applied — and the parts' texts are inert (SEC-146).
        const readme = dlg.getByTestId("offer-readme");
        await readme.locator("summary").click();
        await expect(readme).toContainText("Quieter, and it greets.");
        await expect(dlg.getByTestId("offer-part-0").getByTestId("part-state")).toHaveText("You changed it since. Leave it unticked to keep yours.");
        // The checkboxes: ticked for `unchanged_here` and `new`, not for `edited_here` (C-1117).
        await expect(dlg.getByTestId("offer-part-0").getByRole("checkbox")).not.toBeChecked();
        await expect(dlg.getByTestId("offer-part-1").getByRole("checkbox")).toBeChecked();
        await expect(dlg.getByTestId("offer-part-2").getByRole("checkbox")).toBeChecked();
        // The dialog is themed and scrolls inside itself, at 390 px too.
        expect(await dlg.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(0);
        expect(await dlg.evaluate((el) => getComputedStyle(el).backgroundColor)).toBe(SURFACE[scheme]);
        await dlg.getByRole("button", { name: "Apply" }).click();
        // Exactly the ticked parts, bound to the shown hash (C-1112).
        expect(applied).toEqual({
          version: "1.1.0",
          content_sha256: OFFER_SHA,
          parts: [
            { kind: "helpers", key: "" },
            { kind: "skill", key: "greet" },
          ],
          skill_refs: [],
          inputs: {},
        });
        // The section reads the answer: the new provenance and the helper files the version brought.
        await expect(section.getByTestId("template-made-from")).toHaveText("Made from Schedule manager 1.1.0");
        await expect(section.getByTestId("helper-count")).toHaveText("3 helper files in .aurora/helpers");
        await expect(dlg).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(errors).toEqual([]);
      });
    });
  }
}
