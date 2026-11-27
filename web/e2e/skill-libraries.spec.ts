import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { openLibraryScreen, LIBRARY_SCREENS } from "./support/skill-libraries";
import { id } from "./support/mock-api";
import { CANVAS, overflow } from "./support/open";

// P4-GSL, P5-SKLIB-3 (docs/14 §Skills, §Skill libraries; docs/18 §Git skill libraries, Dashboard): the organization's
// libraries page (two sections), the two-step Add dialog, Edit, the instance's Skill libraries page, Add from library
// (two groups, list and full preview) and the update review are themed in both themes, never scroll horizontally at
// 390 px, have no serious axe violations, and render hostile repository text (markup, a javascript: link) as inert
// data. The API is mocked.

const PAGES = ["skills", "ws-skills", "libraries", "instance-libraries"];

for (const width of [1256, 768, 390]) {
  test.describe(`skill library screens (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      for (const s of LIBRARY_SCREENS) {
        test(`${s}: themed, no horizontal page scroll, accessible (${scheme})`, async ({ page }) => {
          await openLibraryScreen(page, s, scheme);
          expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
          expect(await overflow(page), `${s} overflows horizontally`).toBeLessThanOrEqual(0);
          if (!PAGES.includes(s)) {
            // Dialogs scroll inside themselves; nothing in them may be wider than the dialog.
            const dlg = page.getByRole("dialog");
            expect(await dlg.evaluate((el) => el.scrollWidth - el.clientWidth), `${s} dialog overflows`).toBeLessThanOrEqual(0);
          }
          const { violations } = await new AxeBuilder({ page }).analyze();
          const bad = violations
            .filter((v) => v.impact === "serious" || v.impact === "critical")
            .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
          expect(bad).toEqual([]);
        });
      }
    }
  });
}

test.describe("update review on a phone", () => {
  test.use({ viewport: { width: 390, height: 900 } });
  // (FIX-279) The diffs wrap, the 120-character word included: no box scrolls sideways and cuts its buttons.
  for (const scheme of ["light", "dark"] as const) {
    test(`no diff scrolls sideways (${scheme})`, async ({ page }) => {
      await openLibraryScreen(page, "review", scheme);
      const boxes = page.getByRole("dialog").getByTestId("diff-box");
      await expect(boxes).toHaveCount(3);
      expect(Math.max(...(await boxes.evaluateAll((els) => els.map((el) => el.scrollWidth - el.clientWidth))))).toBeLessThanOrEqual(0);
    });
  }
});

test.describe("library catalogue paging", () => {
  test.use({ viewport: { width: 1280, height: 900 } });
  test("Load more lists the skills of the next page (FIX-100)", async ({ page }) => {
    await openLibraryScreen(page, "add-list", "light");
    const list = page.getByRole("list", { name: "Library skills" });
    await expect(list.getByRole("button", { name: /^Triage issues/ })).toHaveCount(0);
    await page.getByRole("button", { name: "Load more" }).click();
    await expect(list.getByRole("button", { name: /^Triage issues/ })).toBeVisible();
    await expect(page.getByRole("button", { name: "Load more" })).toHaveCount(0);
  });
});

test.describe("repository text is data", () => {
  test.use({ viewport: { width: 1280, height: 900 } });
  test("a markup or link payload in a description or diff is inert", async ({ page }) => {
    await openLibraryScreen(page, "review", "light");
    const dlg = page.getByRole("dialog");
    // the payload is visible as text...
    await expect(dlg.getByText("<img src=x onerror=alert(1)>", { exact: false }).first()).toBeVisible();
    // ...and never became an element or a link
    expect(await dlg.locator("img").count()).toBe(0);
    expect(await dlg.locator("a[href^='javascript:']").count()).toBe(0);
    expect(await page.locator("a[href^='javascript:']").count()).toBe(0);
    // the whole diff is shown: old and new lines of the body and the file
    await expect(dlg.getByText("Check the tests first.")).toBeVisible();
    await expect(dlg.getByText("- docs", { exact: false })).toBeVisible();
  });
});

// P5-SKLIB-3 (acceptance 2): the journeys of both levels, at 1280 and 390 px.
for (const width of [1280, 390]) {
  test.describe(`skill library journeys (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });

    // (D-506) skill-6: the page's heading and tabs, the header's Add skill on the Skills tab only, and cards with every
    // action on screen (no table, no sideways scroll).
    test("organization: the tabs Skills and Libraries; every card action is on screen", async ({ page }) => {
      await openLibraryScreen(page, "skills", "light");
      await expect(page.getByRole("heading", { name: "Skills", level: 1 })).toBeVisible();
      await expect(page.getByText("What every agent in Acme Labs can do, and where it comes from.")).toBeVisible();
      await expect(page.getByRole("tab")).toHaveText(["Skills", "Libraries"]);
      await page.getByRole("button", { name: "Add skill" }).click();
      await page.getByRole("menuitem", { name: "Write a new skill" }).click();
      await expect(page.getByLabel("When should agents use it?")).toBeVisible();
      await page.getByRole("tab", { name: "Libraries" }).click();
      await expect(page).toHaveURL(/\/o\/acme\/skills\?tab=libraries$/);
      await expect(page.getByRole("button", { name: "Add skill" })).toHaveCount(0);
      await expect(page.getByRole("table")).toHaveCount(0);
      const cards = page.getByRole("list", { name: "Your organization's libraries" });
      for (const name of ["Check now Team skills", "Edit Team skills", "More actions for Team skills", "Check now Private tools"]) {
        // Scrolled to (down the page only: nothing scrolls sideways, checked below), the whole button is on screen.
        await cards.getByRole("button", { name }).scrollIntoViewIfNeeded();
        await expect(cards.getByRole("button", { name })).toBeInViewport({ ratio: 1 });
        expect(await cards.getByRole("button", { name }).evaluate((el) => el.getBoundingClientRect().right <= document.documentElement.clientWidth)).toBe(true);
      }
      await expect(cards.getByRole("link", { name: /Update available/ })).toBeVisible();
      // (P5-SKILLS-1, C-1176) The card says how many skills the library offers.
      await expect(cards.getByText("17 skills · 2 used by Acme Labs")).toBeVisible();
      // One card per row at 1256 and 390 px: the two cards share a left edge.
      const boxes = await Promise.all((await cards.locator(":scope > li").all()).map((li) => li.boundingBox()));
      expect(new Set(boxes.map((b) => Math.round(b!.x))).size).toBe(1);
      await cards.getByRole("button", { name: "More actions for Team skills" }).click();
      await expect(page.getByRole("menuitem")).toHaveText(["Browse skills", "Pause", "Delete…"]);
      const { violations } = await new AxeBuilder({ page }).include("main").analyze();
      expect(violations.filter((v) => v.impact === "serious" || v.impact === "critical")).toEqual([]);
      expect(await overflow(page)).toBeLessThanOrEqual(0);
    });

    test("organization: add a GitHub page address → check → add → the card appears", async ({ page }) => {
      const mock = await openLibraryScreen(page, "libraries", "light");
      await expect(page.getByRole("list", { name: "Your organization's libraries" })).toBeVisible();
      await expect(page.getByRole("heading", { name: "Shared with every organization" })).toBeVisible();
      await page.getByRole("button", { name: "Add a library" }).first().click();
      const dlg = page.getByRole("dialog", { name: "Add a library" });
      await dlg.getByLabel("Repository address", { exact: true }).fill("https://github.com/acme/skills/tree/main/skills");
      await dlg.getByRole("button", { name: "Check", exact: true }).click();
      const found = dlg.getByTestId("library-found");
      await expect(found).toContainText("github.com/acme/skills");
      await expect(found).toContainText("main");
      await expect(dlg.getByText("Branch and folder were taken from the address.")).toBeVisible();
      await expect(dlg.getByText("2 skills can be added")).toBeVisible();
      await expect(dlg.getByLabel("Label", { exact: true })).toHaveValue("skills");
      await dlg.getByRole("button", { name: "Add library" }).click();
      await expect(dlg).toHaveCount(0);
      const cards = page.getByRole("list", { name: "Your organization's libraries" });
      await expect(cards.getByText("github.com/acme/skills")).toBeVisible();
      expect(mock.writes.find((w) => w.key.endsWith("/skill-libraries"))?.body).toEqual({ label: "skills", url: "https://github.com/acme/skills.git", ref: "main", path: "skills" });
    });

    test("organization: edit → add a token → saved", async ({ page }) => {
      const mock = await openLibraryScreen(page, "edit", "light");
      const dlg = page.getByRole("dialog", { name: "Edit Team skills" });
      await expect(dlg.getByText("No token.")).toBeVisible();
      await dlg.getByRole("button", { name: "Add a token" }).click();
      await dlg.getByLabel("New access token", { exact: true }).fill("a-read-only-token");
      await dlg.getByRole("button", { name: "Save" }).click();
      await expect(dlg).toHaveCount(0);
      expect(mock.writes.find((w) => w.key.startsWith("PATCH"))?.body).toEqual({ token: "a-read-only-token" });
      // re-open: the token is now saved (never shown back)
      await page.getByRole("button", { name: "Edit Team skills" }).click();
      await expect(page.getByRole("dialog", { name: "Edit Team skills" }).getByText(/A token is saved/)).toBeVisible();
    });

    test("organization: Add from library lists both groups and adds from the instance library", async ({ page }) => {
      const mock = await openLibraryScreen(page, "add-list", "light");
      // (P5-SKILLS-4) the libraries on the left, the shared ones under their heading; on a phone a select with both groups
      if (width >= 768) {
        const nav = page.getByRole("navigation", { name: "Libraries" });
        await nav.getByRole("list", { name: "Shared with every organization" }).getByRole("button", { name: /^Company skills/ }).click();
      } else {
        const pick = page.getByLabel("Library", { exact: true });
        await expect(pick.locator("optgroup")).toHaveCount(2);
        await expect(pick.locator("optgroup").nth(0)).toHaveAttribute("label", "Your organization");
        await expect(pick.locator("optgroup").nth(1)).toHaveAttribute("label", "Shared with every organization");
        await pick.selectOption({ label: "Company skills" });
      }
      await page.getByRole("button", { name: /^Incident notes/ }).click();
      await page.getByLabel("Preview of what will be stored").waitFor();
      await page.getByRole("dialog").getByRole("button", { name: "Add to Acme Labs" }).click();
      await expect(page.getByRole("dialog", { name: "Add a skill from a library" })).toHaveCount(0);
      expect(mock.writes.find((w) => w.key.endsWith("/skills"))?.body).toMatchObject({ from_library: { library_id: id(701), path: "incident-notes" } });
    });

    test("organization: an update to a skill from the instance library → Approve with the full diff → applied", async ({ page }) => {
      const mock = await openLibraryScreen(page, "libraries", "light");
      await page.goto("/o/acme/skills");
      await page.getByRole("button", { name: /^Triage issues/ }).first().click();
      await page.getByRole("button", { name: "Review update" }).click();
      const dlg = page.getByRole("dialog");
      await expect(dlg.getByText("Check the tests first.")).toBeVisible();
      await dlg.getByRole("button", { name: "Approve" }).click();
      await expect(dlg).toHaveCount(0);
      const approve = mock.writes.find((w) => w.key.endsWith(`/library/updates/${id(721)}/approve`));
      expect(approve?.body).toEqual({ content_sha256: "b".repeat(64) });
    });

    test("instance: Administration › Skills holds the libraries and the settings; Defaults no longer does", async ({ page }) => {
      await openLibraryScreen(page, "libraries", "light");
      await page.goto("/admin/internet-access?tab=defaults");
      await page.getByRole("heading", { name: "Defaults for new agents" }).waitFor();
      await expect(page.getByRole("heading", { name: "Library settings" })).toHaveCount(0);
      // the old address lands on the new page
      await page.goto("/admin/skill-libraries");
      await expect(page).toHaveURL(/\/admin\/skills$/);
      await expect(page.getByRole("heading", { name: "Skills", level: 1 })).toBeVisible();
      await expect(page.getByRole("heading", { name: "Library settings", level: 3 })).toBeVisible();
      const cards = page.getByRole("list", { name: "Skill libraries of this instance" });
      await expect(cards.getByText("5 skills · used by 2 organizations")).toBeVisible();
      await page.getByRole("button", { name: "Add a library" }).first().click();
      const dlg = page.getByRole("dialog", { name: "Add a library" });
      await dlg.getByLabel("Repository address", { exact: true }).fill("https://github.com/acme/skills/tree/main/skills");
      await dlg.getByRole("button", { name: "Check", exact: true }).click();
      await dlg.getByTestId("library-found").waitFor();
      await dlg.getByRole("button", { name: "Add library" }).click();
      await expect(dlg).toHaveCount(0);
      await expect(cards.getByText("github.com/acme/skills")).toBeVisible();
      expect(await overflow(page)).toBeLessThanOrEqual(0);
    });
  });
}
