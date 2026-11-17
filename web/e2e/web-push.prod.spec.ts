import { expect, test } from "@playwright/test";

import { mockApi } from "./support/mock-api";

// P4-011B-4 (C-835, SEC-102) against the production build, the only place the real Content-Security-Policy applies:
// the worker is served as JavaScript with `Cache-Control: no-cache` and the dashboard CSP, the page CSP allows workers
// and manifests from 'self' only (and the rest of it is unchanged), and the manifest is linked and served.

test("the push worker is served no-cache, as JavaScript, under the dashboard CSP", async ({ request }) => {
  const res = await request.get("/aurora-push-sw.js");
  expect(res.status()).toBe(200);
  expect(res.headers()["cache-control"]).toBe("no-cache");
  expect(res.headers()["content-type"]).toBe("text/javascript; charset=utf-8");
  expect(res.headers()["content-security-policy"]).toContain("worker-src 'self'");
  const body = await res.text();
  expect(body).not.toMatch(/addEventListener\(\s*["']fetch["']|importScripts|clients\.claim/);
});

test("the page CSP gains worker-src and manifest-src 'self' and nothing looser", async ({ page }) => {
  await mockApi(page);
  const res = await page.goto("/account");
  const csp = res!.headers()["content-security-policy"];
  expect(csp).toContain("worker-src 'self';");
  expect(csp).toContain("manifest-src 'self';");
  expect(csp).toContain("connect-src 'self';");
  expect(csp).toContain("default-src 'self'");
  expect(csp).not.toMatch(/(worker|manifest|connect|default)-src[^;]*(\*|https?:)/);
  expect(csp).not.toContain("'unsafe-eval'");
  await expect(page.locator('link[rel="manifest"]')).toHaveAttribute("href", "/manifest.webmanifest");
});

test("the manifest is the static C-835 document", async ({ request }) => {
  const res = await request.get("/manifest.webmanifest");
  expect(res.status()).toBe(200);
  expect(await res.json()).toEqual({
    name: "Aurora",
    short_name: "Aurora",
    start_url: "/",
    display: "standalone",
    icons: [
      { src: "/icon-192.png", sizes: "192x192", type: "image/png" },
      { src: "/icon-512.png", sizes: "512x512", type: "image/png" },
    ],
  });
});
