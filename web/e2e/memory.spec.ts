import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { mockApi } from "./support/mock-api";
import { mockMemories } from "./support/memory";
import { openThemed, overflow, type Scheme } from "./support/open";

// (P5-MEMORY-2) The Memory page (mockups mem-1, mem-2, mem-3, mem-6): list → read → edit → save, search "rotation", a
// new memory, and on a phone the list and the memory as two screens. Both themes; axe (serious and critical) clean; no
// horizontal scroll. `MEMORY_SHOTS=<dir>` also writes screenshots to compare with the mockups. The API is mocked.

const URL = "/o/acme/w/main/agents/daneel?tab=memory";
const SHOTS = process.env.MEMORY_SHOTS;

async function shot(page: Page, name: string) {
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/${name}.png`, fullPage: true });
}

async function clean(page: Page, what: string) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  const bad = violations.filter((v) => v.impact === "serious" || v.impact === "critical");
  expect(bad.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`), `axe: ${what}`).toEqual([]);
  expect(await overflow(page), `${what}: the page scrolls sideways`).toBeLessThanOrEqual(0);
}

async function open(page: Page, scheme: Scheme) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  await mockMemories(page);
  await openThemed(page, URL, scheme);
  return errors;
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1256, 1280]) {
    test.describe(`memory, ${width} px, ${scheme}`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("list, reading view, edit and save", async ({ page }) => {
        const errors = await open(page, scheme);
        const list = page.getByRole("list", { name: "Memories" });
        await expect(list.getByRole("button")).toHaveCount(12);
        await expect(page.getByRole("radio", { name: "All 12" })).toBeChecked();
        await expect(page.getByRole("radio", { name: "By Daneel 1" })).toBeVisible();
        await expect(page.getByRole("radio", { name: "Locked 2" })).toBeVisible();
        await expect(list.getByText("Learned by Daneel in “Release prep” · today")).toBeVisible();
        await expect(page.getByText(/Select a memory/)).toHaveCount(0);
        const article = page.getByRole("article", { name: "Aurora migration project" });
        await expect(article.getByRole("checkbox", { name: "Pilot workspace set up" })).toBeChecked();
        await expect(article.getByTestId("memory-meta")).toContainText("Changed twice · Daneel last used it in “Weekly report” yesterday");
        // Two columns: the memory sits beside the list.
        const [l, a] = await Promise.all([list.boundingBox(), article.boundingBox()]);
        expect(a!.x).toBeGreaterThan(l!.x + l!.width);
        await shot(page, `mem-1-${width}-${scheme}`);
        await clean(page, "reading view");

        await article.getByRole("button", { name: "Edit" }).click();
        const form = page.getByRole("form", { name: "Edit Aurora migration project" });
        const text = form.getByRole("textbox", { name: "What should Daneel remember?" });
        expect((await text.boundingBox())!.height).toBeGreaterThanOrEqual(398);
        await text.fill("# Aurora migration\n\nDone.");
        await expect(page.getByRole("region", { name: "Unsaved changes" })).toBeVisible();
        await shot(page, `mem-2-edit-${width}-${scheme}`);
        await clean(page, "edit");
        // Leaving by the Settings tab's second row asks first.
        await page.getByRole("tablist", { name: "Settings pages" }).getByRole("tab", { name: "Skills" }).click();
        await page.getByRole("dialog", { name: "Leave without saving?" }).getByRole("button", { name: "Keep editing" }).click();
        await form.getByRole("tab", { name: "Preview" }).click();
        await expect(form.getByRole("heading", { name: "Aurora migration" })).toBeVisible();
        await page.getByRole("button", { name: "Save", exact: true }).click();
        await expect(page.getByTestId("save-bar-saved")).toBeVisible();
        await expect(article).toContainText("Done.");
        // The deep link holds the memory over a reload.
        await list.getByRole("button", { name: /^On-call rotation/ }).click();
        await expect(page).toHaveURL(/tab=memory&memory=on-call-rotation/);
        await page.reload();
        await expect(page.getByRole("article", { name: "On-call rotation" })).toBeVisible();
        expect(errors).toEqual([]);
      });

      test("search shows the match", async ({ page }) => {
        const errors = await open(page, scheme);
        await page.getByRole("searchbox", { name: "Search memories" }).fill("rotation");
        await expect(page.getByText("1 memory matches “rotation”")).toBeVisible();
        await expect(page.getByRole("list", { name: "Search results" }).locator("mark").first()).toBeVisible();
        const article = page.getByRole("article", { name: "On-call rotation" });
        await expect(article.getByTestId("match-line")).toContainText("Match 1 of 1");
        await expect(article.locator(".md-match-line")).toBeInViewport();
        expect(await page.locator("main").innerText()).not.toContain("<b>");
        await shot(page, `mem-3-${width}-${scheme}`);
        await clean(page, "search");
        await page.getByRole("button", { name: "Clear", exact: true }).click();
        await expect(page.getByRole("article", { name: "Aurora migration project" })).toBeVisible();
        expect(errors).toEqual([]);
      });

      test("a new memory has a key made from its title", async ({ page }) => {
        const errors = await open(page, scheme);
        await page.getByRole("button", { name: "New memory" }).click();
        await page.getByRole("textbox", { name: "Title" }).fill("Where the runbooks live");
        await expect(page.getByRole("textbox", { name: "Key" })).toHaveValue("where-the-runbooks-live");
        await expect(page.getByText("New memory · not saved yet")).toBeVisible();
        await shot(page, `mem-2-new-${width}-${scheme}`);
        await clean(page, "new memory");
        await page.getByRole("textbox", { name: "What should Daneel remember?" }).fill("In the wiki.");
        await page.getByRole("button", { name: "Save", exact: true }).click();
        await expect(page.getByRole("article", { name: "Where the runbooks live" })).toBeVisible();
        expect(errors).toEqual([]);
      });
    });
  }

  test.describe(`memory, 390 px, ${scheme}`, () => {
    test.use({ viewport: { width: 390, height: 800 } });

    test("the list and the memory are two screens", async ({ page }) => {
      const errors = await open(page, scheme);
      const list = page.getByRole("list", { name: "Memories" });
      await expect(list).toBeVisible();
      await expect(page.getByRole("article")).toHaveCount(0);
      await expect(page.getByRole("button", { name: "New memory" })).toHaveText("New", { useInnerText: true });
      await shot(page, `mem-6-list-390-${scheme}`);
      await clean(page, "phone list");

      await list.getByRole("button", { name: /^Aurora migration project/ }).click();
      const article = page.getByRole("article", { name: "Aurora migration project" });
      await expect(article).toBeInViewport();
      await expect(list).toHaveCount(0);
      // The title row and the second row step aside on the memory's own screen.
      await expect(page.getByRole("tablist", { name: "Settings pages" })).toBeHidden();
      for (const name of ["Memory", "Edit", "More actions", "Lock", "History"]) {
        await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
      }
      await shot(page, `mem-6-memory-390-${scheme}`);
      await clean(page, "phone memory");

      await page.getByRole("button", { name: "Edit" }).click();
      await expect(page.getByRole("form", { name: "Edit Aurora migration project" })).toBeVisible();
      await page.getByRole("textbox", { name: "Title" }).fill("Aurora move");
      await expect(page.getByRole("region", { name: "Unsaved changes" })).toBeVisible();
      await shot(page, `mem-6-edit-390-${scheme}`);
      await clean(page, "phone edit");
      await page.getByRole("button", { name: "Discard" }).click();
      await page.getByRole("button", { name: "Memory", exact: true }).click();
      await expect(list).toBeVisible();
      await expect(page.getByRole("tablist", { name: "Settings pages" })).toBeVisible();
      expect(errors).toEqual([]);
    });
  });
}
