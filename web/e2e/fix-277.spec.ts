import { expect, test, type Locator, type Page } from "@playwright/test";

import { id, mockApi } from "./support/mock-api";
import { memory, mockMemories } from "./support/memory";
import { openThemed, overflow } from "./support/open";

// FIX-277: two rendering details only a real layout can show. A list that mixes ordinary and checklist items keeps the
// ordinary items' bullets; and in History's wrapped diff a continued line starts under the line's text, not under its
// +/- sign, and a long word starts on the sign's line. The API is mocked.

const MIXED = ["## Risks", "", "- The service account still needs read access to the dashboards.", "- [ ] Ask for the access", "- [x] Write it down"].join("\n");
const LONG_WORDS = "Switch the old bot off by 1 December, after the last weekly report has gone out and Carol has confirmed the rota.";
const LONG_ID = `platform/${"very-long-identifier-without-a-break-".repeat(4)}end`;
const BEFORE = [MIXED, "", "Old line."].join("\n");
const CURRENT = [MIXED, "", LONG_WORDS, LONG_ID].join("\n");

const revision = (n: number, content: string) => ({
  revision: n,
  author_kind: "user",
  author_label: "Ada Lovelace",
  provenance: "dashboard",
  author: { kind: "person", person: { id: id(99), display_name: "Ada Lovelace" } },
  via: "dashboard",
  redacted: false,
  content,
  created_at: new Date(Date.now() - 86_400_000 * (3 - n)).toISOString(),
});

async function open(page: Page, query = "") {
  await mockApi(page);
  await mockMemories(page, [memory({ key: "risks", title: "Risks", content: CURRENT, revision: 2 })]);
  await page.route(/\/v1\/agents\/[^/]+\/memories\/risks\/revisions$/, (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ items: [revision(2, CURRENT), revision(1, BEFORE)], next_cursor: null }) }),
  );
  await openThemed(page, `/o/acme/w/main/agents/daneel?tab=memory&memory=risks${query}`, "light");
}

for (const width of [1256, 390]) {
  test.describe(`${width} px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("a mixed list keeps the ordinary item's bullet; checklist items have the box and none", async ({ page }) => {
      await open(page);
      const article = page.getByRole("article", { name: "Risks" });
      const plain = article.getByRole("listitem").filter({ hasText: "service account" });
      const task = article.getByRole("listitem").filter({ hasText: "Ask for the access" });
      await expect(plain).toBeVisible();
      const bullet = (item: Locator) => item.evaluate((el) => getComputedStyle(el).listStyleType);
      expect(await bullet(plain)).toBe("disc");
      expect(await bullet(task)).toBe("none");
      // The ordinary item is indented like a list item (its bullet has room), and the box stays left of that text.
      const textLeft = (item: Locator) => item.evaluate((el) => el.getBoundingClientRect().left - el.parentElement!.getBoundingClientRect().left);
      expect(await textLeft(plain)).toBeGreaterThanOrEqual(16);
      expect(await textLeft(task)).toBeLessThanOrEqual(6);
      expect(await overflow(page)).toBe(0);
    });

    test("a wrapped diff line continues under its text, and a long word starts on the sign's line", async ({ page }) => {
      await open(page, "&history=1");
      const panel = page.getByRole("article", { name: "History" });
      await expect(panel.getByTestId("diff-view")).toBeVisible();
      for (const start of ["Switch the old bot off by 1 December, after", "platform/very-long-identifier"]) {
        const cell = panel.locator('tr[data-diff="add"] td:last-child').filter({ hasText: start });
        // The left edge and top of every line box the cell draws: the sign's, and the text's.
        const lines = await cell.evaluate((el) => {
          const rects: { left: number; top: number; text: string }[] = [];
          const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
          for (let node = walker.nextNode(); node; node = walker.nextNode()) {
            const value = node.textContent ?? "";
            for (let i = 0; i < value.length; i++) {
              if (/\s/.test(value[i]!)) continue;
              const range = document.createRange();
              range.setStart(node, i);
              range.setEnd(node, i + 1);
              const r = range.getClientRects()[0];
              if (r) rects.push({ left: r.left, top: Math.round(r.top), text: value[i]! });
            }
          }
          const [sign, ...rest] = rects;
          const byLine = new Map<number, number>();
          for (const r of rest) byLine.set(r.top, Math.min(byLine.get(r.top) ?? Infinity, r.left));
          return { sign: sign!, tops: [...byLine.keys()], lefts: [...byLine.values()] };
        });
        expect(lines.sign.text).toBe("+");
        if (width === 390) expect(lines.tops.length, `${start}: wraps`).toBeGreaterThan(1);
        expect(lines.tops[0], `${start}: starts on the sign's line`).toBe(lines.sign.top);
        for (const left of lines.lefts) expect(Math.abs(left - lines.lefts[0]!), `${start}: continuations align with the text`).toBeLessThan(1);
        expect(lines.lefts[0]!).toBeGreaterThan(lines.sign.left + 5);
      }
      const box = panel.getByTestId("diff-box");
      expect(await box.evaluate((el) => el.scrollWidth - el.clientWidth), "the diff scrolls sideways").toBeLessThanOrEqual(0);
      expect(await overflow(page)).toBe(0);
    });
  });
}
