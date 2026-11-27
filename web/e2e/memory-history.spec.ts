import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { id, mockApi } from "./support/mock-api";
import { AURORA, memory, mockMemories } from "./support/memory";
import { openThemed, overflow, type Scheme } from "./support/open";

// (P5-MEMORY-3, mem-4) A memory's History: open → select a version → read the diff → Restore → Undo; removing a
// version's text. Both themes, 1256 px and 390 px, axe (serious and critical) clean, no horizontal scroll.
// `MEMORY_SHOTS=<dir>` also writes screenshots to compare with the mockup. The API is mocked.

const URL = "/o/acme/w/main/agents/daneel?tab=memory&memory=aurora-migration";
const SHOTS = process.env.MEMORY_SHOTS;
const ago = (ms: number) => new Date(Date.now() - ms).toISOString();
const bob = { id: id(99), display_name: "Ada Lovelace" };

// The change mem-4 draws: one task ticked, one line reworded, one decision added.
const BEFORE = AURORA.replace("- [ ] Switch the old bot off", "- [ ] Switch the old bot off by 1 December").replace(
  "- Cut over on Monday mornings, never on a Friday.\n",
  "",
);
const CURRENT = AURORA.replace("- [ ] Move the on-call agent", "- [x] Move the on-call agent");

const revision = (n: number, content: string, over: Record<string, unknown>) => ({
  revision: n,
  author_kind: "user",
  author_label: "Ada Lovelace",
  provenance: "dashboard",
  author: { kind: "person", person: bob },
  via: "dashboard",
  redacted: false,
  content,
  created_at: ago(2 * 86_400_000),
  ...over,
});

const REVISIONS = [
  revision(3, CURRENT, { via: "api", author: { kind: "api_key", person: bob, api_key: { id: "k1", name: "laptop" } }, created_at: ago(120_000) }),
  revision(2, BEFORE, {
    via: "conversation",
    author_kind: "agent",
    author: { kind: "agent", agent: { id: id(11), name: "Daneel" }, conversation: { id: "c2", title: "Release prep" } },
    created_at: ago(86_400_000),
  }),
  revision(1, "# Aurora migration\n\n**Goal:** move the agents.", {}),
];

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
  const restored: number[] = [];
  const removed: number[] = [];
  const items = [memory({ key: "aurora-migration", title: "Aurora migration project", description: "Status, decisions and next steps", content: CURRENT, revision: 3 })];
  const revisions = REVISIONS.map((r) => ({ ...r }));
  await mockApi(page);
  await mockMemories(page, items);
  await page.route(/\/v1\/agents\/[^/]+\/memories\/aurora-migration\/(revisions(\/\d+\/redact)?|restore)$/, async (route) => {
    const path = new globalThis.URL(route.request().url()).pathname;
    const json = (status: number, body: unknown) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/revisions")) return json(200, { items: revisions, next_cursor: null });
    if (path.endsWith("/restore")) {
      restored.push((route.request().postDataJSON() as { revision: number }).revision);
      return json(200, items[0]);
    }
    const n = Number(path.split("/").at(-2));
    removed.push(n);
    Object.assign(revisions.find((r) => r.revision === n)!, { redacted: true, content: null });
    return route.fulfill({ status: 204 });
  });
  await openThemed(page, URL, scheme);
  return { errors, restored, removed };
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1256, 390]) {
    test.describe(`memory history, ${width} px, ${scheme}`, () => {
      test.use({ viewport: { width, height: 900 } });
      const phone = width < 768;

      test("open, select a version, read the diff, restore and undo", async ({ page }) => {
        const { errors, restored } = await open(page, scheme);
        if (phone) await page.getByRole("button", { name: "History", exact: true }).click();
        else {
          await page.getByRole("button", { name: "More actions" }).click();
          await page.getByRole("menuitem", { name: "History" }).click();
        }
        await expect(page).toHaveURL(/memory=aurora-migration&history=1/);
        const panel = page.getByRole("article", { name: "History" });
        await expect(page.getByRole("article", { name: "Aurora migration project" })).toHaveCount(0);
        await expect(panel.getByText("Aurora migration project · 3 versions")).toBeVisible();
        const versions = panel.getByRole("list", { name: "Versions" }).getByRole("button");
        await expect(versions).toHaveCount(3);
        await expect(versions.first()).toContainText("through an API key");
        await expect(versions.nth(1)).toContainText("in “Release prep”");
        await expect(panel).not.toContainText("@");
        await expect(panel.getByTestId("diff-view")).toBeVisible();
        await expect(panel.getByTestId("diff-summary")).toContainText("added");
        // Long lines wrap: the diff never scrolls sideways, and the folded rows' buttons stay inside its box.
        const box = panel.getByTestId("diff-box");
        expect(await box.evaluate((el) => el.scrollWidth - el.clientWidth), "the diff scrolls sideways").toBeLessThanOrEqual(0);
        const fold = box.getByTestId("diff-fold").getByRole("button");
        await expect(fold).toHaveCount(3);
        const edges = (await box.boundingBox())!;
        for (const button of await fold.all()) {
          const b = (await button.boundingBox())!;
          expect(b.x, "a fold button starts left of the box").toBeGreaterThanOrEqual(edges.x);
          expect(b.x + b.width, "a fold button ends right of the box").toBeLessThanOrEqual(edges.x + edges.width);
        }
        await expect(panel.getByRole("button", { name: "Restore this version" })).toHaveCount(0);
        if (phone) {
          await expect(page.getByRole("list", { name: "Memories" })).toHaveCount(0);
          await expect(page.getByRole("tablist", { name: "Settings pages" })).toBeHidden();
        } else {
          // The list stays beside the panel.
          const [l, a] = await Promise.all([page.getByRole("list", { name: "Memories" }).boundingBox(), panel.boundingBox()]);
          expect(a!.x).toBeGreaterThan(l!.x + l!.width);
        }
        await shot(page, `mem-4-${width}-${scheme}`);
        await clean(page, "history, current version");

        // The current version's text can't be removed, and the menu says why.
        await panel.getByRole("button", { name: "More actions" }).click();
        const disabled = page.getByRole("menuitem", { name: /Remove this version's text…/ });
        await expect(disabled).toHaveAttribute("aria-disabled", "true");
        await expect(disabled).toContainText("The current version can't be removed: change the memory first.");
        await shot(page, `mem-4-menu-${width}-${scheme}`);
        await clean(page, "history menu");
        await page.keyboard.press("Escape");

        await versions.nth(1).click();
        await expect(panel.getByText(/^Compared with the version before \(Ada Lovelace, /)).toBeVisible();
        await clean(page, "history, an older version");
        await panel.getByRole("button", { name: "Restore this version" }).click();
        await expect(page.getByRole("article", { name: "Aurora migration project" })).toBeVisible();
        const toast = page.getByTestId("undo-toast");
        await expect(toast).toContainText("Restored version 2");
        await toast.getByRole("button", { name: "Undo" }).click();
        await expect.poll(() => restored).toEqual([2, 3]);
        expect(errors).toEqual([]);
      });

      test("a version's text is removed after asking twice", async ({ page }) => {
        const { errors, removed } = await open(page, scheme);
        await page.goto(`${URL}&history=1`);
        const panel = page.getByRole("article", { name: "History" });
        const versions = panel.getByRole("list", { name: "Versions" }).getByRole("button");
        await versions.nth(1).click();
        await panel.getByRole("button", { name: "More actions" }).click();
        await page.getByRole("menuitem", { name: "Remove this version's text…" }).click();
        const dialog = page.getByRole("dialog", { name: "Remove the text of version 2?" });
        await expect(dialog).toContainText("This can't be undone.");
        await clean(page, "remove dialog");
        expect(removed).toEqual([]);
        await dialog.getByRole("button", { name: "Remove text" }).click();
        await expect(panel.getByText("This version's text was removed")).toBeVisible();
        await expect(versions.nth(1)).toContainText("its text was removed");
        await expect(panel.getByTestId("diff-view")).toHaveCount(0);
        expect(removed).toEqual([2]);
        await clean(page, "removed");
        await panel.page().getByRole("button", { name: "Back to the memory" }).click();
        await expect(page.getByRole("article", { name: "Aurora migration project" })).toBeVisible();
        expect(errors).toEqual([]);
      });
    });
  }
}
