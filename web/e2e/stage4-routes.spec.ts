import { expect, test } from "@playwright/test";

import { mkdirSync } from "node:fs";

import { ACCESS_SCREENS, openAccess } from "./support/access";
import { ROUTES } from "./support/mock-api";
import { CONNECTOR_SCREENS, openConnectorScreen } from "./support/connectors";
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

// P4-MCON-3: the in-page screens of Admin → Connectors are themed and never scroll horizontally at
// 390 px; full-page screenshots of the add flow and the detail view for the coordinator.
const SHOTS = "/tmp/aurora-screens/p4-mcon-3";
for (const width of [1280, 390]) {
  test.describe(`connector screens (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      for (const s of CONNECTOR_SCREENS) {
        test(`${s}: themed, no horizontal page scroll (${scheme})`, async ({ page }) => {
          await openConnectorScreen(page, s, scheme);
          expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
          expect(await overflow(page), `${s} overflows horizontally`).toBeLessThanOrEqual(0);
          await page.screenshot({ path: `${SHOTS}/${s}-${scheme}-${width}.png`, fullPage: true });
        });
      }
    }
  });
}

// FIX-022: the agent Access tab (connection list) and the grant editor open, themed, no horizontal page scroll;
// full-page screenshots for the coordinator.
const ACCESS_SHOTS = "/tmp/aurora-screens/fix-022";
mkdirSync(ACCESS_SHOTS, { recursive: true });
for (const width of [1280, 390]) {
  test.describe(`agent Access tab (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      for (const s of ACCESS_SCREENS) {
        test(`${s}: themed, no horizontal page scroll (${scheme})`, async ({ page }) => {
          await openAccess(page, s, scheme);
          expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
          expect(await overflow(page), `${s} overflows horizontally`).toBeLessThanOrEqual(0);
          if (s !== "connections") {
            // The dialog scrolls inside itself; nothing in it may be wider than the dialog.
            const dlg = page.getByRole("dialog");
            expect(await dlg.evaluate((el) => el.scrollWidth - el.clientWidth), `${s} dialog overflows`).toBeLessThanOrEqual(0);
          }
          await page.screenshot({ path: `${ACCESS_SHOTS}/${s}-${scheme}-${width}.png`, fullPage: s === "connections" });
        });
      }
    }
  });
}

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
