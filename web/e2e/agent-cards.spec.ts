import { expect, test, type Page } from "@playwright/test";

import { agent, id, mockApi } from "./support/mock-api";
import { openRoute, overflow } from "./support/open";

// P5-LAYOUT-3 (D-440): agents as cards in the shared CardGrid on Overview and on the Agents page: four to a row at
// 1920 px, three at 1280, one at 768 and 390 (the grid's container is narrower than `@2xl` beside the sidebar), a single agent at the start of the row, no horizontal scroll.

const four = [
  agent,
  { ...agent, id: id(12), handle: "rex", name: "Rex", is_default: false },
  { ...agent, id: id(13), handle: "marvin", name: "Marvin", is_default: false },
  { ...agent, id: id(14), handle: "bishop", name: "Bishop", is_default: false },
];

async function withAgents(page: Page, items: unknown[]) {
  // Registered after `mockApi`: Playwright runs the latest matching route first.
  await page.route(/\/v1\/workspaces\/[^/]+\/agents$/, (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ items, next_cursor: null }) }),
  );
}

async function perRow(page: Page): Promise<number> {
  const cards = page.getByRole("list", { name: "Agents" }).getByRole("listitem");
  const tops = await cards.evaluateAll((els) => els.map((el) => Math.round(el.getBoundingClientRect().top)));
  return Math.max(...tops.map((t) => tops.filter((u) => u === t).length));
}

const PAGES = [
  ["Overview", "/o/acme/w/main"],
  ["Agents", "/o/acme/w/main/agents"],
] as const;

for (const scheme of ["light", "dark"] as const) {
  for (const [width, columns] of [[1920, 4], [1280, 3], [768, 1], [390, 1]] as const) {
    // (768 px: the sidebar leaves the grid ~450 px, below `@2xl`, so one column; the grid follows its container.)
    for (const [label, path] of PAGES) {
      test.describe(`${label}, ${scheme} theme, ${width} px`, () => {
        test.use({ viewport: { width, height: width === 1920 ? 1080 : 900 } });

        test(`four agents show ${columns} per row, each with its name link and Chat, and nothing scrolls sideways`, async ({ page }) => {
          await mockApi(page);
          await withAgents(page, four);
          await page.emulateMedia({ colorScheme: scheme });
          await page.goto(path);
          const list = page.getByRole("list", { name: "Agents" });
          await expect(list.getByRole("listitem")).toHaveCount(4);
          await expect(list.getByRole("link", { name: "Rex", exact: true })).toBeVisible();
          await expect(list.getByRole("link", { name: "Chat with Rex" })).toBeVisible();
          expect(await perRow(page)).toBe(columns);
          expect(await overflow(page)).toBeLessThanOrEqual(0);
        });

        test("one agent is one card at the start of the row, not stretched", async ({ page }) => {
          await openRoute(page, path, scheme);
          const card = page.getByRole("list", { name: "Agents" }).getByRole("listitem");
          await expect(card).toHaveCount(1);
          const box = (await card.boundingBox())!;
          const list = (await page.getByRole("list", { name: "Agents" }).boundingBox())!;
          expect(Math.abs(box.x - list.x)).toBeLessThan(2);
          if (columns > 1) expect(box.width).toBeLessThan(list.width * 0.6);
          expect(await overflow(page)).toBeLessThanOrEqual(0);
        });
      });
    }
  }
}
