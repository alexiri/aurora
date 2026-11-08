import { expect, type Page } from "@playwright/test";

import { mockApi } from "./mock-api";

export type Scheme = "light" | "dark";

// Canvas token values from globals.css (`--canvas`), as computed `rgb()` strings.
export const CANVAS: Record<Scheme, string> = { light: "rgb(246, 247, 249)", dark: "rgb(11, 16, 32)" };

/** Opens a route with the API mocked and the OS colour scheme emulated; resolves once the page is themed and hydrated. */
export async function openRoute(page: Page, path: string, scheme: Scheme) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  // A mock that does not fit the page would crash it; a crashed page proves nothing.
  expect(errors, `${path} threw`).toEqual([]);
}

export const overflow = (page: Page) =>
  page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
