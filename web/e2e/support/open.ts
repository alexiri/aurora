import { expect, type Page } from "@playwright/test";

import { mockApi } from "./mock-api";

export type Scheme = "light" | "dark";

// Canvas token values from globals.css (`--canvas`), as computed `rgb()` strings.
export const CANVAS: Record<Scheme, string> = { light: "rgb(246, 247, 249)", dark: "rgb(11, 16, 32)" };

/**
 * Resolves once the document has a title. After a client-side navigation (a redirect, a link) the old route's `<title>` is
 * gone and the new route's metadata streams in a few ms later; in between `document.title` is "" and axe's
 * `document-title` rule fails. The title is the real condition to wait for, not time.
 */
export async function titled(page: Page) {
  await expect.poll(() => page.title(), { message: "document.title is set" }).not.toBe("");
}

/** Opens `path` (mocked by the spec) with the OS colour scheme emulated; resolves once the page is themed and titled. */
export async function openThemed(page: Page, path: string, scheme: Scheme) {
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await titled(page);
}

/** Opens a route with the API mocked and the OS colour scheme emulated; resolves once the page is themed and hydrated. */
export async function openRoute(page: Page, path: string, scheme: Scheme) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  // (P5-POLISH-1) A console error (a CSP violation included) fails every spec that opens a route. "Failed to load
  // resource" is a request the mock leaves unanswered, not a page error.
  page.on("console", (m) => {
    if (m.type() === "error" && !/^Failed to load resource/.test(m.text())) errors.push(m.text());
  });
  await mockApi(page);
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  // `/` shows a placeholder and then redirects client-side; the page to check is where it lands, not the placeholder
  // (the first heading below is already on `/`, so without this the redirect could land during the checks).
  if (path === "/") await page.waitForURL((u) => u.pathname !== "/");
  await page.locator("h1, h2").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await titled(page);
  // A mock that does not fit the page would crash it; a crashed page proves nothing.
  expect(errors, `${path} threw`).toEqual([]);
}

export const overflow = (page: Page) =>
  page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
