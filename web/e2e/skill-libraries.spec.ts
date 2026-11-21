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

const PAGES = ["libraries", "instance-libraries"];

for (const width of [1280, 768, 390]) {
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

test.describe("library catalogue paging", () => {
  test.use({ viewport: { width: 1280, height: 900 } });
  test("Load more lists the skills of the next page (FIX-100)", async ({ page }) => {
    await openLibraryScreen(page, "add-list", "light");
    const list = page.getByRole("list", { name: "Library skills" });
    await expect(list.getByRole("button", { name: /^triage-issues/ })).toHaveCount(0);
    await page.getByRole("button", { name: "Load more" }).click();
    await expect(list.getByRole("button", { name: /^triage-issues/ })).toBeVisible();
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

    test("organization: add a GitHub page address → check → add → the row appears", async ({ page }) => {
      const mock = await openLibraryScreen(page, "libraries", "light");
      await expect(page.getByRole("heading", { name: "Your organization's libraries" })).toBeVisible();
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
      const table = page.getByRole("table", { name: "Your organization's libraries" });
      await expect(table.getByText("github.com/acme/skills")).toBeVisible();
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
      const pick = page.getByLabel("Library", { exact: true });
      await expect(pick.locator("optgroup")).toHaveCount(2);
      await expect(pick.locator("optgroup").nth(0)).toHaveAttribute("label", "Your organization");
      await expect(pick.locator("optgroup").nth(1)).toHaveAttribute("label", "Shared with every organization");
      await pick.selectOption({ label: "Company skills" });
      await page.getByRole("button", { name: /^incident-notes/ }).click();
      await page.getByLabel("Preview of what will be stored").waitFor();
      await page.getByRole("dialog").getByRole("button", { name: "Add", exact: true }).click();
      await expect(page.getByRole("dialog", { name: "Add from a library" })).toHaveCount(0);
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

    test("instance: Defaults › Skill libraries › Manage → add → the row and Organizations using it", async ({ page }) => {
      await openLibraryScreen(page, "libraries", "light");
      await page.goto("/admin/internet-access?tab=defaults");
      await page.getByRole("heading", { name: "Skill libraries", level: 3 }).waitFor();
      await expect(page.getByText("1 shared with every organization")).toBeVisible();
      await page.getByRole("link", { name: "Manage skill libraries" }).click();
      await expect(page).toHaveURL(/\/admin\/skill-libraries$/);
      const table = page.getByRole("table", { name: "Skill libraries of this instance" });
      await expect(table.getByRole("columnheader", { name: "Organizations using it" })).toBeVisible();
      await page.getByRole("button", { name: "Add a library" }).first().click();
      const dlg = page.getByRole("dialog", { name: "Add a library" });
      await dlg.getByLabel("Repository address", { exact: true }).fill("https://github.com/acme/skills/tree/main/skills");
      await dlg.getByRole("button", { name: "Check", exact: true }).click();
      await dlg.getByTestId("library-found").waitFor();
      await dlg.getByRole("button", { name: "Add library" }).click();
      await expect(dlg).toHaveCount(0);
      await expect(table.getByText("github.com/acme/skills")).toBeVisible();
      expect(await overflow(page)).toBeLessThanOrEqual(0);
    });
  });
}
