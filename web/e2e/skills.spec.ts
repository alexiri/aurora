import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { mockApi, org, user, ws } from "./support/mock-api";
import { openThemed, overflow, type Scheme } from "./support/open";
import { mockSkills } from "./support/skills";

// (P5-SKILLS-3) The Skills pages (mockups skill-1 … skill-4, skill-6's list): the agent's one list, the panel, an
// inherited skill turned off, a library skill, the editor and the Add file dialog, the workspace's and organization's
// lists, and on a phone the list and the skill as two screens. Both themes; axe (serious and critical) clean; no
// horizontal scroll. `SKILLS_SHOTS=<dir>` also writes screenshots to compare with the mockups. The API is mocked.

const AGENT = "/o/acme/w/main/agents/daneel?tab=skills";
const SHOTS = process.env.SKILLS_SHOTS;

async function shot(page: Page, name: string) {
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/${name}.png`, fullPage: true });
}

async function clean(page: Page, what: string) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  const bad = violations.filter((v) => v.impact === "serious" || v.impact === "critical");
  expect(bad.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`), `axe: ${what}`).toEqual([]);
  expect(await overflow(page), `${what}: the page scrolls sideways`).toBeLessThanOrEqual(0);
}

async function open(page: Page, scheme: Scheme, path = AGENT) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  const mock = await mockSkills(page);
  await openThemed(page, path, scheme);
  return { errors, mock };
}

/** The shared mock's `/v1/me` names no organization role; an admin may add a library. */
async function asOrgAdmin(page: Page) {
  await page.route("**/v1/me", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ user, memberships: [{ org, role: "owner", status: "active", workspaces: [{ ...ws, personal: false }] }], default_org_id: org.id }),
    }),
  );
}

const skills = (page: Page) => page.getByRole("list", { name: "Skills", exact: true });
const row = (page: Page, name: string) => page.getByTestId(`skill-row-${name}`);
const openRow = (page: Page, name: string, label: string) => row(page, name).getByRole("button", { name: label, exact: true }).click();

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1256, 1280]) {
    test.describe(`skills, ${width} px, ${scheme}`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("one list, Add skill, the panel, edit and save", async ({ page }) => {
        const { errors, mock } = await open(page, scheme);
        await expect(skills(page).getByRole("listitem")).toHaveCount(12);
        await expect(page.getByRole("radio", { name: "All 12" })).toBeChecked();
        await expect(page.getByRole("radio", { name: "Daneel’s own 6" })).toBeVisible();
        await expect(page.getByRole("radio", { name: "From the workspace 2" })).toBeVisible();
        await expect(page.getByRole("radio", { name: "From Acme Labs 4" })).toBeVisible();
        await expect(page.getByRole("radio", { name: "Off 1" })).toBeVisible();
        await expect(row(page, "release-notes")).toContainText("Daneel’s version replaces the workspace’s");
        await shot(page, `skill-1-${width}-${scheme}`);
        await clean(page, "list");
        await page.getByRole("button", { name: "Add skill" }).click();
        await expect(page.getByRole("menuitem")).toHaveText(["Write a new skill", "Add from a library", "Upload a skill folder"]);
        await shot(page, `skill-1-menu-${width}-${scheme}`);
        await page.keyboard.press("Escape");

        await openRow(page, "triage-alerts", "Triage alerts");
        const panel = page.getByRole("article", { name: "Triage alerts" });
        await expect(panel.getByRole("link", { name: "Try it in chat" })).toBeVisible();
        await expect(panel.getByTestId("skill-usage")).toHaveText("Used 3 times this week · last in “Alert storm 26 Nov”");
        // The panel is beside the list.
        const [l, a] = await Promise.all([skills(page).boundingBox(), panel.boundingBox()]);
        expect(a!.x).toBeGreaterThan(l!.x + l!.width);
        await panel.getByRole("button", { name: "scripts/firing.sh" }).click();
        await expect(panel.getByRole("region", { name: "File scripts/firing.sh" })).toContainText("List firing alerts as JSON");
        await expect(page).toHaveURL(/tab=skills&skill=/);
        await shot(page, `skill-2-${width}-${scheme}`);
        await clean(page, "panel");
        // The link holds the skill over a reload.
        await page.reload();
        await expect(page.getByRole("article", { name: "Triage alerts" })).toBeVisible();

        await openRow(page, "summarize-pull-requests", "Summarize pull requests");
        await page.getByRole("article", { name: "Summarize pull requests" }).getByRole("button", { name: "Edit" }).click();
        const form = page.getByRole("form", { name: "Edit Summarize pull requests" });
        await expect(form.getByText("summarize-pull-requests")).toBeVisible();
        const when = form.getByLabel("When should Daneel use it?");
        expect((await when.boundingBox())!.height).toBeLessThanOrEqual(44);
        await when.fill("Summarize a pull request for a reviewer in five bullets");
        await expect(page.getByRole("region", { name: "Unsaved changes" })).toBeVisible();
        await shot(page, `skill-4-${width}-${scheme}`);
        await clean(page, "editor");
        await form.getByRole("button", { name: /Files · 2/ }).click();
        await form.getByRole("button", { name: "Open template.md" }).click();
        await expect(form.getByRole("textbox", { name: "Content of template.md" })).toHaveValue(/## Summary/);
        await shot(page, `skill-4-files-${width}-${scheme}`);
        await clean(page, "files");
        await form.getByRole("button", { name: "Add file" }).click();
        const dialog = page.getByRole("dialog", { name: "Add a file" });
        await dialog.getByLabel("File name").fill("scripts/pr-diff.sh");
        await shot(page, `skill-4-add-file-${width}-${scheme}`);
        await clean(page, "add file");
        await dialog.getByRole("button", { name: "Cancel" }).click();
        await expect(dialog).toHaveCount(0);
        // Cancelling the dialog leaves the form as it was.
        await expect(when).toHaveValue("Summarize a pull request for a reviewer in five bullets");
        await page.getByRole("button", { name: "Save", exact: true }).click();
        await expect(page.getByTestId("save-bar-saved")).toBeVisible();
        await expect(page.getByRole("article", { name: "Summarize pull requests" })).toContainText("Use it to summarize a pull request for a reviewer");
        expect(mock.writes.map((w) => w.key)).toEqual([expect.stringMatching(/^PATCH \/v1\/skills\//)]);
        expect(errors).toEqual([]);
      });

      test("an inherited skill is turned off for this agent; read-only skills say so", async ({ page }) => {
        const { errors, mock } = await open(page, scheme);
        await row(page, "onboarding-new-hire").getByRole("switch", { name: "Turn off for Daneel: Onboarding new hire" }).click();
        await expect(page.getByRole("radio", { name: "Off 2" })).toBeVisible();
        await expect(row(page, "onboarding-new-hire").getByRole("switch")).not.toBeChecked();
        expect(mock.writes.map((w) => w.key)).toEqual([expect.stringMatching(/^PUT \/v1\/agents\/[^/]+\/skills\/[^/]+\/off$/)]);

        await openRow(page, "onboarding-new-hire", "Onboarding new hire");
        const panel = page.getByRole("article", { name: "Onboarding new hire" });
        await expect(panel.getByTestId("skill-lock")).toHaveText("Managed by Acme Labs’s admins. You can turn it off for Daneel.");
        await expect(panel.getByRole("textbox")).toHaveCount(0);
        // …and on again from the panel.
        await panel.getByRole("switch").click();
        await expect(panel.getByRole("switch")).toBeChecked();
        await expect(panel.getByRole("link", { name: "Try it in chat" })).toBeVisible();
        await expect(panel.getByTestId("skill-usage")).toHaveText("Used by 14 agents in Acme Labs · Daneel used it 0 times this week");
        await shot(page, `skill-3-${width}-${scheme}`);
        await clean(page, "read-only");

        await openRow(page, "pdf", "pdf");
        const pdf = page.getByRole("article", { name: "pdf" });
        await expect(pdf.getByTestId("skill-lock")).toContainText("From Anthropic skills · pinned to e4bb5d9.");
        await expect(pdf.getByTestId("update-available")).toContainText("3 files changed upstream.");
        await expect(pdf.getByRole("button", { name: "Save" })).toHaveCount(0);
        await shot(page, `skill-3-library-${width}-${scheme}`);
        await clean(page, "library skill");

        await openRow(page, "release-notes", "Release notes");
        await openRow(page, "release-notes", "Release notes");
        await row(page, "release-notes").getByRole("button", { name: "Compare Release notes" }).click();
        await expect(page.getByRole("dialog", { name: "Compare Release notes" })).toContainText("SKILL.md");
        await clean(page, "compare");
        expect(errors).toEqual([]);
      });

      // (P5-SKILLS-4, skill-5)
      test("add a skill from a library: only what can be added, a preview, then Add", async ({ page }) => {
        const { errors, mock } = await open(page, scheme);
        await asOrgAdmin(page);
        await page.reload();
        await page.getByRole("button", { name: "Add skill" }).click();
        await page.getByRole("menuitem", { name: "Add from a library" }).click();
        const dialog = page.getByRole("dialog", { name: "Add a skill from a library" });
        const libraries = dialog.getByRole("navigation", { name: "Libraries" });
        await expect(libraries.getByRole("button", { name: /^Anthropic skills\s*17 skills$/ })).toHaveAttribute("aria-current", "true");
        await expect(libraries.getByRole("button", { name: "Add a library" })).toBeVisible();
        const list = dialog.getByRole("list", { name: "Library skills" });
        await expect(list.getByRole("listitem")).toHaveCount(7);
        await expect(list.getByRole("button", { name: /^pdf.*Added$/ })).toBeVisible();
        await dialog.getByRole("button", { name: /^Brand guidelines/ }).click();
        const preview = dialog.getByRole("group", { name: "Preview of what will be stored" });
        await expect(preview.getByRole("heading", { name: "Typography" })).toBeVisible();
        await expect(preview.getByRole("button", { name: /^LICENSE\.txt\s*0\.3 KB$/ })).toBeVisible();
        await expect(preview).toContainText("Pinned to e4bb5d9 when added.");
        await expect(preview).toContainText("image: a pixel");
        expect(await preview.locator("img").count()).toBe(0);
        // The three columns stand side by side, inside the dialog.
        const [nav, mid, right] = await Promise.all([libraries.boundingBox(), list.boundingBox(), preview.boundingBox()]);
        expect(mid!.x).toBeGreaterThanOrEqual(nav!.x + nav!.width - 1);
        expect(right!.x).toBeGreaterThan(mid!.x + mid!.width);
        await shot(page, `skill-5-${width}-${scheme}`);
        await clean(page, "add from a library");
        // The line is pinned at the foot of the column, on screen without scrolling.
        await expect(dialog.getByRole("button", { name: /8 can’t be added/ })).toBeInViewport({ ratio: 1 });
        await dialog.getByRole("button", { name: /8 can’t be added/ }).click();
        const reasons = dialog.getByRole("list", { name: "Skills that can’t be added" });
        await expect(reasons).toContainText("A skill named release-notes is already on Daneel.");
        await expect(reasons.getByRole("listitem").first()).toBeInViewport();
        await shot(page, `skill-5-why-${width}-${scheme}`);
        await clean(page, "why");
        await dialog.getByRole("button", { name: "Add to Daneel" }).click();
        await expect(dialog).toHaveCount(0);
        await expect(page.getByRole("article", { name: "Brand guidelines" })).toBeVisible();
        expect(JSON.parse(mock.writes.at(-1)!.body)).toMatchObject({ from_library: { path: "brand-guidelines" } });
        expect(await page.getByRole("article", { name: "Brand guidelines" }).locator("img").count()).toBe(0);
        expect(errors).toEqual([]);
      });

      for (const level of [
        { name: "workspace", path: "/o/acme/w/main/skills", column: /^For /, count: 3 },
        { name: "organization", path: "/o/acme/skills", column: /^For Acme Labs$/, count: 4 },
      ]) {
        test(`the ${level.name} page is the same list and panel`, async ({ page }) => {
          const { errors } = await open(page, scheme, level.path);
          await expect(skills(page).getByRole("listitem")).toHaveCount(level.count);
          await expect(page.getByText(level.column)).toBeVisible();
          await expect(page.getByRole("radio", { name: new RegExp(`^All ${level.count}$`) })).toBeChecked();
          await expect(page.getByRole("button", { name: "Add skill" })).toBeVisible();
          await expect(skills(page).getByRole("listitem").first()).toContainText(/Used by \d+ agents?/);
          await shot(page, `skill-6-${level.name}-${width}-${scheme}`);
          await clean(page, `${level.name} list`);
          await skills(page).getByRole("listitem").first().getByRole("button").first().click();
          await expect(page.getByRole("article").getByTestId("skill-usage")).toContainText(/Used by \d+ agents?/);
          await expect(page.getByRole("article").getByRole("link", { name: "Try it in chat" })).toHaveCount(0);
          await shot(page, `skill-6-${level.name}-panel-${width}-${scheme}`);
          await clean(page, `${level.name} panel`);
          expect(errors).toEqual([]);
        });
      }
    });
  }

  // 768 px too: the rail and the settings sub-nav leave the list about 300 px there.
  for (const width of [390, 768]) test.describe(`skills, ${width} px, ${scheme}`, () => {
    test.use({ viewport: { width, height: 800 } });

    test("the list and the skill are two screens", async ({ page }) => {
      const { errors } = await open(page, scheme);
      await expect(skills(page)).toBeVisible();
      await expect(page.getByRole("article")).toHaveCount(0);
      // Every name has room to be read and pressed.
      for (const name of await skills(page).locator("li button:first-of-type span").all()) expect((await name.boundingBox())!.width).toBeGreaterThan(20);
      await shot(page, `skill-1-${width}-${scheme}`);
      await clean(page, "phone list");
      await openRow(page, "triage-alerts", "Triage alerts");
      const panel = page.getByRole("article", { name: "Triage alerts" });
      await expect(panel).toBeInViewport();
      await expect(skills(page)).toHaveCount(0);
      await expect(page.getByRole("tablist", { name: "Settings pages" })).toBeHidden();
      await panel.getByRole("button", { name: "scripts/firing.sh" }).click();
      await expect(panel.getByRole("region", { name: "File scripts/firing.sh" })).toBeVisible();
      await shot(page, `skill-2-${width}-${scheme}`);
      await clean(page, "phone panel");
      await panel.getByRole("button", { name: "Edit" }).click();
      await expect(page.getByRole("form", { name: "Edit Triage alerts" })).toBeVisible();
      await page.getByRole("button", { name: /Files · 2/ }).click();
      await shot(page, `skill-4-${width}-${scheme}`);
      await clean(page, "phone editor");
      await page.getByRole("button", { name: "Done" }).click();
      await page.getByRole("button", { name: "Skills", exact: true }).click();
      await expect(skills(page)).toBeVisible();
      await expect(page.getByRole("tablist", { name: "Settings pages" })).toBeVisible();
      expect(errors).toEqual([]);
    });

    // (FIX-279) The two texts wrap: nothing to scroll sideways to, and no button cut off.
    test("Compare wraps its diffs", async ({ page }) => {
      const { errors } = await open(page, scheme);
      await row(page, "release-notes").getByRole("button", { name: "Compare Release notes" }).click();
      const dialog = page.getByRole("dialog", { name: "Compare Release notes" });
      const boxes = dialog.getByTestId("diff-box");
      await expect(boxes).toHaveCount(2);
      expect(Math.max(...(await boxes.evaluateAll((els) => els.map((el) => el.scrollWidth - el.clientWidth))))).toBeLessThanOrEqual(0);
      await shot(page, `skill-1-compare-${width}-${scheme}`);
      await clean(page, "compare");
      expect(errors).toEqual([]);
    });

    test("add a skill from a library on a narrow screen", async ({ page }) => {
      const { errors, mock } = await open(page, scheme);
      await page.getByRole("button", { name: "Add skill" }).click();
      await page.getByRole("menuitem", { name: "Add from a library" }).click();
      const dialog = page.getByRole("dialog", { name: "Add a skill from a library" });
      const list = dialog.getByRole("list", { name: "Library skills" });
      await expect(list.getByRole("listitem")).toHaveCount(7);
      // Below 768 px the libraries are a select and the skill its own screen; from 768 px the three columns.
      const phone = width < 768;
      await expect(dialog.getByLabel("Library", { exact: true })).toHaveCount(phone ? 1 : 0);
      const dialogOverflow = () => dialog.evaluate((el) => el.scrollWidth - el.clientWidth);
      expect(await dialogOverflow()).toBeLessThanOrEqual(0);
      await shot(page, `skill-5-list-${width}-${scheme}`);
      await clean(page, "library list");
      await dialog.getByRole("button", { name: /^Brand guidelines/ }).click();
      await expect(dialog.getByRole("group", { name: "Preview of what will be stored" })).toBeVisible();
      await expect(list).toHaveCount(phone ? 0 : 1);
      expect(await dialogOverflow()).toBeLessThanOrEqual(0);
      await expect(dialog.getByRole("button", { name: "Add to Daneel" })).toBeInViewport({ ratio: 1 });
      await shot(page, `skill-5-preview-${width}-${scheme}`);
      await clean(page, "library preview");
      if (phone) {
        await dialog.getByRole("button", { name: "Skills", exact: true }).click();
        await expect(list).toBeVisible();
        await dialog.getByRole("button", { name: /^Brand guidelines/ }).click();
      }
      await dialog.getByRole("button", { name: "Add to Daneel" }).click();
      await expect(dialog).toHaveCount(0);
      await expect(page.getByRole("article", { name: "Brand guidelines" })).toBeVisible();
      expect(mock.writes).toHaveLength(1);
      expect(errors).toEqual([]);
    });

    for (const path of ["/o/acme/w/main/skills", "/o/acme/skills"]) {
      test(`${path} fits a phone`, async ({ page }) => {
        const { errors } = await open(page, scheme, path);
        await expect(skills(page)).toBeVisible();
        await shot(page, `skill-6-${path.includes("/w/") ? "workspace" : "organization"}-${width}-${scheme}`);
        await clean(page, "phone level list");
        await skills(page).getByRole("listitem").first().getByRole("button").first().click();
        await expect(page.getByRole("article")).toBeInViewport();
        await clean(page, "phone level panel");
        expect(errors).toEqual([]);
      });
    }
  });
}
