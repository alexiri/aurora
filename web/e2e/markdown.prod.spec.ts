import { expect, test } from "@playwright/test";

import { MINE, openChat } from "./support/chat-mock";
import { MARKDOWN_FIXTURE } from "./support/markdown-fixture";

// FIX-137 against the PRODUCTION build (`pnpm e2e:prod`), the only place the real Content-Security-Policy
// applies: KaTeX (its stylesheet and fonts come from 'self'; its sizing is inline style *attributes*, allowed by
// style-src-attr) and the highlighter must render without a single CSP violation, with no loosened policy.

test.use({ viewport: { width: 1280, height: 900 } });

test("math, highlighted code, alerts and the table render under the production CSP with no violation", async ({ page }) => {
  const violations: string[] = [];
  page.on("console", (m) => {
    if (/content security policy/i.test(m.text())) violations.push(m.text());
  });
  await openChat(page, "light", MARKDOWN_FIXTURE, `?c=${MINE.id}`);
  const log = page.getByRole("log");
  await expect(log.locator(".katex").first()).toBeVisible({ timeout: 30_000 });
  await expect(log.locator(".hljs-keyword").first()).toBeVisible({ timeout: 30_000 });
  await expect(log.locator(".md-alert-note")).toBeVisible();
  // KaTeX's stylesheet applied (its glyph boxes have a real size), not just its markup.
  const w = await log.locator(".katex .katex-html").first().evaluate((el) => el.getBoundingClientRect().width);
  expect(w).toBeGreaterThan(20);
  expect(violations).toEqual([]);
});
