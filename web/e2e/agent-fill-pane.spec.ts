import { expect, test, type Page } from "@playwright/test";

import { openChat } from "./support/chat-mock";
import { Fs, mockFiles, openFiles, serious } from "./support/files";
import { agent, id, mockApi } from "./support/mock-api";
import { overflow, type Scheme } from "./support/open";
import { enc, mockWithTerminal, openTab } from "./support/terminal";

// docs/14 §Agent page: under the header and its tab rows the agent page's full-height panes (Terminal, Files, the Chat
// thread) take exactly the room that is left, so from 768 px the page never scrolls. Checked with Show more controls on
// and off: its line under the pane counts too.

/** `main`'s bottom padding from `md` (`md:py-8`). */
const PAGE_PADDING = 32;

async function moreControls(page: Page, on: boolean) {
  await page.route("**/v1/me/onboarding", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ welcome_seen: true, guide_dismissed: [], tour_completed_at: null, notices_seen: ["renames-2026-11"], more_controls: on }),
    }),
  );
}

/** The page is still, and the agent page ends exactly at the viewport's bottom less the page padding. */
async function expectFilled(page: Page) {
  const measure = () =>
    page.evaluate(() => {
      const root = document.querySelector("main header")!.parentElement!;
      return {
        scroll: document.scrollingElement!.scrollHeight - window.innerHeight,
        gap: Math.round(window.innerHeight - root.getBoundingClientRect().bottom),
      };
    });
  await expect.poll(measure, { message: "the page scrolls, or the pane leaves room unused" }).toEqual({ scroll: 0, gap: PAGE_PADDING });
  expect(await overflow(page)).toBeLessThanOrEqual(0);
}

const manyFiles = () => {
  const fs = new Fs();
  for (let i = 1; i <= 60; i += 1) fs.nodes.set(`/workspace/file-${String(i).padStart(2, "0")}.txt`, { kind: "file", content: "x\n", mtime: "2026-11-15T10:00:00Z" });
  return fs;
};

for (const [width, height, scheme] of [
  [1280, 800, "light"],
  [1280, 800, "dark"],
  [1256, 900, "light"],
] as [number, number, Scheme][]) {
  for (const more of [true, false]) {
    test.describe(`${width}×${height} ${scheme}, more controls ${more ? "on" : "off"}`, () => {
      test.use({ viewport: { width, height } });

      test("Terminal fills the room below the tab rows", async ({ page }) => {
        const lines = Array.from({ length: 200 }, (_, i) => `line ${i + 1}\r\n`).join("");
        await mockWithTerminal(page, { output: enc(lines), resizes: [] });
        await moreControls(page, more);
        await openTab(page, scheme);
        const pane = page.getByRole("group", { name: "Agent terminal" });
        await expect(pane).toContainText("line 200", { timeout: 20_000 });
        await expectFilled(page);
        expect((await pane.boundingBox())!.height).toBeGreaterThanOrEqual(288);
      });

      test("Files: a long listing scrolls inside its pane", async ({ page }) => {
        await mockFiles(page, manyFiles());
        await moreControls(page, more);
        await openFiles(page, scheme);
        const last = page.getByRole("button", { name: "file-60.txt", exact: true });
        await expect(last).toBeAttached();
        await expectFilled(page);
        await last.scrollIntoViewIfNeeded();
        await expect(last).toBeInViewport();
        await expectFilled(page);
      });

      test("Files: a short listing and the editor fill the same room", async ({ page }) => {
        await mockFiles(page, new Fs());
        await moreControls(page, more);
        await openFiles(page, scheme);
        await expect(page.getByRole("button", { name: "notes.md", exact: true })).toBeVisible();
        await expectFilled(page);
        await page.getByRole("button", { name: "notes.md", exact: true }).click();
        await expect(page.locator(".cm-editor")).toBeVisible();
        await expectFilled(page);
        expect((await page.locator(".cm-editor").boundingBox())!.height).toBeGreaterThanOrEqual(250);
      });

      test("the Chat thread fills the room below the tab row", async ({ page }) => {
        await page.addInitScript(() => window.localStorage.clear());
        await mockApi(page);
        await openChat(page, scheme);
        await moreControls(page, more);
        await page.reload();
        await page.locator('nav[aria-label="Conversations"]').waitFor();
        await page.waitForLoadState("networkidle");
        await expectFilled(page);
      });
    });
  }
}

// The Connect page of a bring-your-own agent: its code boxes scroll, so the keyboard must reach them.
for (const [width, scheme] of [
  [390, "light"],
  [390, "dark"],
  [1256, "light"],
  [1256, "dark"],
] as [number, Scheme][]) {
  test.describe(`Connect page, ${width} px (${scheme})`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("scrolling code boxes are focusable and named; axe is clean", async ({ page }) => {
      await mockApi(page);
      const long = `https://${"gateway-with-a-very-long-name.".repeat(6)}example.org:8443`;
      await page.route("**/v1/**", async (route) => {
        const p = new URL(route.request().url()).pathname;
        const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
        if (p.endsWith("/agents")) return json({ items: [{ ...agent, kind: "byo" }], next_cursor: null });
        if (p === `/v1/agents/${id(11)}/container-config` || p.endsWith("/container-config")) {
          return json({
            proxy_url: long,
            ca_path: "/etc/aurora/ca.pem",
            proxy_ca_path: "/etc/aurora/proxy-ca.pem",
            env: Object.fromEntries(Array.from({ length: 30 }, (_, i) => [`AURORA_SETTING_${i}`, long])),
            notes: [],
          });
        }
        return route.fallback();
      });
      await page.emulateMedia({ colorScheme: scheme });
      await page.goto("/o/acme/w/main/agents/daneel?tab=connect");
      await expect(page.getByTestId("container-config")).toBeVisible();
      await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
      await page.waitForLoadState("networkidle");
      expect(await serious(page)).toEqual([]);
      for (const name of ["Container config file", "aurora login", "aurora run"]) {
        await expect(page.getByRole("region", { name, exact: true })).toHaveAttribute("tabindex", "0");
      }
      expect(await overflow(page)).toBeLessThanOrEqual(0);
    });
  });
}
