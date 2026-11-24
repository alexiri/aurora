import { expect as baseExpect, test, type Page } from "@playwright/test";

import { serious } from "./support/axe";
import { agent, mockApi, now } from "./support/mock-api";
import { overflow, titled } from "./support/open";

// The first request of a route compiles it in `next dev`; on a loaded host that outlasts the default 5 s.
const expect = baseExpect.configure({ timeout: 30_000 });

// P6-TPL-5 (C-1118): **Save as template…** beside Export on the Manage page. The dialog reads the
// agent through the existing routes and downloads <id>-<version>.zip in the browser; it refuses
// Save while the outline's placeholder lines stand and asks a why per app.

const ownSkill = { id: "sk-own", name: "release-notes", description: "When asked what shipped", body: "# Notes", files: [], tier: "agent", source: null, enabled: true };
const libSkill = {
  id: "sk-lib", name: "incident-writeup", description: "Writes the incident summary.", body: "Library body that is never written.",
  files: [], tier: "agent", enabled: true, source: { kind: "library", detached: false, library_id: "lib1", update_pending: false },
};
const library = { id: "lib1", label: "Service desk skills", scope: "org", status: "active", url: "https://gitlab.example.org/agents/skills.git", ref: "v1.2", path: "skills" };

// Daneel: an own skill, a library skill, a memory, a schedule and a helper. The accounts list of
// mock-api already gives the workspace's two apps (GitHub read, Linear ask) to the agent.
const overrides: Record<string, unknown> = {
  [`/v1/agents/${agent.id}/memories`]: { items: [{ key: "likes-tea", content: "tea", updated_at: now }], next_cursor: null },
  [`/v1/agents/${agent.id}/skills`]: { items: [ownSkill, libSkill], next_cursor: null },
  [`/v1/agents/${agent.id}/schedules`]: { items: [{ name: "Morning", prompt: "Check.", cron: "0 8 * * 1-5", timezone: "UTC", enabled: true }], next_cursor: null },
  [`/v1/agents/${agent.id}/webhooks`]: { items: [], next_cursor: null },
  [`/v1/agents/${agent.id}/grants`]: { connections: [], secrets: [], converted_by_app: [] },
  [`/v1/agents/${agent.id}/helpers`]: { items: [{ path: "tidy.sh", size: 24, executable: true, sha256: "x" }] },
  [`/v1/agents/${agent.id}/skills/effective`]: { items: [], shadowed: [] },
  "/v1/skill-libraries/lib1": library,
};

async function open(page: Page, scheme: "light" | "dark") {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => {
    if (m.type() === "error" && !/^Failed to load resource/.test(m.text())) errors.push(m.text());
  });
  await mockApi(page, overrides);
  // Registered after mockApi, so it matches first: the helper's bytes (the list says it is executable).
  await page.route("**/helpers/content*", (route) => route.fulfill({ status: 200, contentType: "application/octet-stream", body: "#!/bin/sh\necho tidied\n" }));
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto("/o/acme/w/main/agents/daneel?tab=manage");
  await page.locator("h1, h2").first().waitFor();
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await titled(page);
  return errors;
}

for (const scheme of ["light", "dark"] as const) {
  for (const [width, label] of [[1280, "desktop"], [390, "phone"]] as const) {
    test.describe(`save as template (${scheme}, ${label})`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("refuses the placeholder, then downloads <id>-<version>.zip; axe, no scroll, no errors", async ({ page }) => {
        const errors = await open(page, scheme);
        await page.getByRole("button", { name: "Save as template…" }).click();
        const d = page.getByRole("dialog", { name: "Save Daneel as a template" });
        await expect(d).toBeVisible();
        // The groups line reads what the file would carry (the memory is unticked by default).
        await expect(d.getByTestId("template-summary-line")).toHaveText(
          "Instructions · 1 memory · 1 own skill · 1 library skill (references) · 1 helper file · 1 schedule",
        );
        // Save is refused while the outline's placeholder lines stand.
        await d.getByRole("button", { name: "Save", exact: true }).click();
        await expect(d.getByTestId("save-problem")).toContainText("Replace the placeholder lines");
        // Fill the summary, a why per app and the README; the memory stays unticked.
        await d.getByLabel("Summary").fill("Triage the team's pull requests.");
        for (const why of await d.getByLabel("Why it's needed").all()) await why.fill("Keeps the queue moving.");
        await d.getByLabel("What it's for").fill("## What Daneel does\n\nTriage pull requests and hand them to the right reviewer.\n");
        const [download] = await Promise.all([page.waitForEvent("download"), d.getByRole("button", { name: "Save", exact: true }).click()]);
        expect(download.suggestedFilename()).toBe("daneel-1.0.0.zip");
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
        expect(errors).toEqual([]);
      });
    });
  }
}
