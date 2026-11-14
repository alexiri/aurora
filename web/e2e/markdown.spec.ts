import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { MINE, openChat } from "./support/chat-mock";
import { MARKDOWN_FIXTURE } from "./support/markdown-fixture";
import { overflow } from "./support/open";

// FIX-137 (D-227, docs/14 › Agent chat): a long agent reply with every element, in both themes at 1280 px and
// 390 px — no page-level horizontal scroll (the wide table and the code block scroll inside their own boxes) and no
// serious or critical axe violation (no rule disabled, nothing excluded).

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`markdown, ${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });
      test("a reply with every element: tables and code scroll inside themselves, axe clean", async ({ page }) => {
        await openChat(page, scheme, MARKDOWN_FIXTURE, `?c=${MINE.id}`);
        const log = page.getByRole("log");
        await expect(log.getByRole("heading", { name: "Weekly report" })).toBeVisible();
        // The lazy extras (highlight, math, emoji) have applied.
        await expect(log.locator(".hljs-keyword").first()).toBeVisible({ timeout: 20_000 });
        await expect(log.locator(".katex").first()).toBeVisible({ timeout: 20_000 });
        await expect(log).toContainText("🎉");
        await expect(log.locator(".md-alert-note")).toBeVisible();
        await expect(log.locator(".md-alert-warning")).toBeVisible();
        // D-227: the other-origin image is a link, never loaded.
        await expect(log.locator("img[src^='https://evil.example']")).toHaveCount(0);
        await expect(log.getByRole("link", { name: /remote/ })).toHaveAttribute("href", "https://evil.example/leak.png");
        // The wide table scrolls in its own container.
        const box = log.locator(".md-table").first();
        const { scroll, client } = await box.evaluate((el) => ({ scroll: el.scrollWidth, client: el.clientWidth }));
        if (width === 390) expect(scroll, "the table needs its own scrolling at 390 px").toBeGreaterThan(client);
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
        const { violations } = await new AxeBuilder({ page }).analyze();
        expect(
          violations
            .filter((v) => v.impact === "serious" || v.impact === "critical")
            .map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).slice(0, 4).join(" | ")}`),
        ).toEqual([]);
        await page.screenshot({ path: `/tmp/aurora-screens/fix-137-${scheme}-${width}.png`, fullPage: false });
      });
    });
  }
}
