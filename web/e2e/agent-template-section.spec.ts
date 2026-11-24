import AxeBuilder from "@axe-core/playwright";
import { expect as baseExpect, test, type Page } from "@playwright/test";

import { agent, mockApi, ws } from "./support/mock-api";
import { CANVAS, overflow } from "./support/open";

const expect = baseExpect.configure({ timeout: 30_000 });

// P6-TPL-4 B2 (C-1117, C-1143): the agent's Template section on Settings › Manage — "Made from
// <template> <version>", the helper files with View (path, size, Executable, the text as plain
// text) and Remove (one or all, with a confirm). Themed in both themes, usable at 390 px without
// horizontal scroll, no serious axe violations. The API is mocked; the apply journey is below.

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

async function open(page: Page, scheme: "light" | "dark") {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => {
    if (m.type() === "error" && !/^Failed to load resource/.test(m.text())) errors.push(m.text());
  });
  await mockApi(page, overrides);
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
    });
  }
}
