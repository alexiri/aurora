import { expect, test } from "@playwright/test";

import { ROUTES } from "./support/mock-api";
import { CANVAS, openRoute, overflow } from "./support/open";

// P2-UI stage 4 (docs/18 §Dashboard visual design): on every dashboard route, in both themes,
// the page takes the theme's canvas colour and (at 390 px) has no horizontal page scroll. The
// sidebar background spans the viewport on long pages. The API is mocked with the real built-in
// packages (e2e/support/mock-api.ts).

test.describe("theme and narrow shell, every route (390 px)", () => {
  test.use({ viewport: { width: 390, height: 800 } });

  for (const scheme of ["light", "dark"] as const) {
    for (const r of ROUTES) {
      test(`${r.name}: themed, no horizontal page scroll (${scheme})`, async ({ page }) => {
        await openRoute(page, r.path, scheme);
        expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor), `${r.path} canvas`).toBe(CANVAS[scheme]);
        expect(await overflow(page), `${r.path} overflows horizontally`).toBeLessThanOrEqual(0);
      });
    }
  }
});

test.describe("sidebar background", () => {
  test.use({ viewport: { width: 1280, height: 720 } });

  // P2-UI-3b's full-page screenshots showed the sidebar background ending partway down. Check it with
  // real scrolling: the sticky sidebar must keep covering the whole viewport on a page taller than it.
  for (const scheme of ["light", "dark"] as const) {
    test(`covers the viewport height while a long page scrolls (${scheme})`, async ({ page }) => {
      await openRoute(page, "/o/acme/storage", scheme);
      await page.evaluate(() => {
        const filler = document.createElement("div");
        filler.className = "h-[3000px]";
        document.querySelector("main")!.appendChild(filler);
      });
      for (const y of [0, 900, 2500]) {
        await page.evaluate((top) => window.scrollTo(0, top), y);
        const box = await page.getByRole("complementary", { name: "Sidebar" }).evaluate((el) => {
          const r = el.getBoundingClientRect();
          return { top: r.top, bottom: r.bottom, vh: window.innerHeight, bg: getComputedStyle(el).backgroundColor };
        });
        expect(box.top, `scrollY ${y}`).toBeCloseTo(0, 0);
        expect(box.bottom, `scrollY ${y}`).toBeGreaterThanOrEqual(box.vh - 1);
        expect(box.bg).not.toBe("rgba(0, 0, 0, 0)");
      }
    });
  }
});
