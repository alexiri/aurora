import { expect, test, type Page } from "@playwright/test";

import type { Scheme } from "./support/open";
import { enc, mockWithTerminal, openTab, type MockPty } from "./support/terminal";

// FIX-098: the Terminal tab against the PRODUCTION build (`pnpm e2e:prod`: `next build` + `next
// start`, playwright.config.ts). `next dev` adds 'unsafe-inline' to style-src, so the dev suite can
// never see what the production CSP does to wterm's DOM renderer: it writes each run's colour and
// cell span (`--term-span-cols`) as inline style *attributes*, which `style-src 'self' 'nonce-…'`
// drops. D-176: the policy gains `style-src-attr 'unsafe-inline'`, nothing else.

test.use({ viewport: { width: 1280, height: 900 } });

/** The documented policy (docs/14 §Security headers, C-622, D-176), nonce elided. */
const EXPECTED_CSP =
  "^default-src 'self'; " +
  "img-src 'self' data:; " +
  "script-src 'self' 'nonce-[A-Za-z0-9+/=]+' 'wasm-unsafe-eval'; " +
  "style-src 'self' 'nonce-[A-Za-z0-9+/=]+'; " +
  "style-src-attr 'unsafe-inline'; " +
  "connect-src 'self'; " +
  "object-src 'none'; " +
  "base-uri 'self'; " +
  "form-action 'self'; " +
  "frame-ancestors 'none'$";

/** The palette's red (`--aurora-term-color-1`) per theme, as a computed colour. */
const RED: Record<Scheme, string> = { light: "rgb(185, 28, 28)", dark: "rgb(248, 113, 113)" };

/** Opens the pane with `output` painted; collects CSP violations the browser reports. */
async function paint(page: Page, output: string, scheme: Scheme = "light") {
  const violations: string[] = [];
  page.on("console", (m) => {
    if (/content security policy/i.test(m.text())) violations.push(m.text());
  });
  const pty: MockPty = { output: enc(output), resizes: [] };
  await mockWithTerminal(page, pty);
  await openTab(page, scheme);
  await expect(page.getByRole("group", { name: "Agent terminal" })).toContainText("ab", { timeout: 20_000 });
  return violations;
}

/** Row `n`'s bounding box and each child span's, relative to the pane's grid, plus the cell width. */
async function geometry(page: Page, row: number) {
  return page.locator(".term-pane").evaluate((pane, n) => {
    const cell = parseFloat(getComputedStyle(pane).getPropertyValue("--term-cell-width")) || 8;
    const r = pane.querySelectorAll(".term-row")[n]!;
    const origin = r.getBoundingClientRect().left;
    const spans = Array.from(r.children).map((c) => {
      const b = c.getBoundingClientRect();
      return { text: c.textContent ?? "", cls: c.className, left: b.left - origin, right: b.right - origin };
    });
    return { cell, spans };
  }, row);
}

test("the served header is the documented policy exactly: style-src-attr only, nothing looser (D-176, C-622)", async ({ request }) => {
  const res = await request.get("/o/acme/w/main/agents/daneel");
  expect(res.status()).toBe(200);
  const csp = res.headers()["content-security-policy"] ?? "";
  expect(csp).toMatch(new RegExp(EXPECTED_CSP));
  // The two loosenings the owner decided, spelled out: nothing else is unsafe.
  expect(csp.match(/'unsafe-[a-z]+'/g)).toEqual(["'unsafe-inline'"]);
  expect(csp.split("; ").find((d) => d.startsWith("style-src "))).not.toContain("unsafe-inline");
  expect(csp.split("; ").find((d) => d.startsWith("script-src "))).not.toMatch(/'unsafe-/);
  expect(res.headers()["x-content-type-options"]).toBe("nosniff");
});

test("only style attributes are loosened: an un-nonced <style> element is still blocked", async ({ page }) => {
  await page.goto("/o/acme/w/main/agents/daneel");
  const result = await page.evaluate(() => {
    const probe = document.createElement("div");
    probe.id = "csp-probe";
    probe.textContent = "x";
    document.body.append(probe);
    const attr = document.createElement("div");
    attr.setAttribute("style", "padding-left: 37px");
    document.body.append(attr);
    const style = document.createElement("style"); // no nonce
    style.textContent = "#csp-probe { padding-left: 53px }";
    document.head.append(style);
    return { attr: getComputedStyle(attr).paddingLeft, element: getComputedStyle(probe).paddingLeft };
  });
  expect(result.attr).toBe("37px");
  expect(result.element).toBe("0px");
});

for (const scheme of ["light", "dark"] as const) {
  test(`SGR red renders with the palette's red; a wide character keeps its two cells (${scheme})`, async ({ page }) => {
    const violations = await paint(page, `\x1b[31mred\x1b[0m 世 ab\r\n`, scheme);
    const red = page.locator(".term-row span", { hasText: /^red$/ });
    await expect(red).toBeVisible();
    expect(await red.evaluate((e) => getComputedStyle(e).color)).toBe(RED[scheme]);
    const { cell, spans } = await geometry(page, 0);
    const wide = spans.find((s) => s.cls.includes("term-wide"))!;
    expect(wide.text).toBe("世");
    expect(wide.right - wide.left).toBeGreaterThanOrEqual(2 * cell - 1);
    expect(wide.right - wide.left).toBeLessThanOrEqual(2 * cell + 1);
    // "red" (3 cells) then a space: the wide char starts at column 4.
    expect(Math.abs(wide.left - 4 * cell)).toBeLessThanOrEqual(1);
    expect(violations).toEqual([]);
  });
}

test("the cursor sits at column 8 after `sh-5.2$ `; differently styled runs of one row do not overlap", async ({ page }) => {
  await paint(page, "\x1b[1mab\x1b[0mcd\r\nsh-5.2$ ");
  const styled = await geometry(page, 0);
  const [ab, cd] = styled.spans;
  expect(ab!.text).toBe("ab");
  expect(cd!.text.startsWith("cd")).toBe(true);
  // `c` starts at column 2 and `ab` ends where it starts: no overlap, no gap.
  expect(Math.abs(cd!.left - 2 * styled.cell)).toBeLessThanOrEqual(1);
  expect(ab!.right).toBeLessThanOrEqual(cd!.left + 1);
  const prompt = await geometry(page, 1);
  const cursor = prompt.spans.find((s) => s.cls.includes("term-cursor"))!;
  expect(Math.abs(cursor.left - 8 * prompt.cell)).toBeLessThanOrEqual(1);
});
