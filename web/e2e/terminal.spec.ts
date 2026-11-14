import { expect, test } from "@playwright/test";

import { agent, mockApi } from "./support/mock-api";
import { CANVAS, overflow } from "./support/open";
import { enc, mockWithTerminal, openTab, type MockPty } from "./support/terminal";

// P4-TERM (docs/14 §Terminal tab, docs/18 §Web terminal): the agent Terminal tab against the
// shared mock. Tab presence follows the instance switch (`features.terminal`, C-622) and the
// hosted kind; the pane is the real wterm + Ghostty WASM in real Chromium (jsdom has neither
// WASM nor layout), fed by a mocked WebSocket that speaks the leg-1 protocol (C-619): binary
// frames are PTY bytes, `pty.resize` is the one text control, `pty_id: 0` names the leg's PTY.
// (SEC-87) The CSP carries `'wasm-unsafe-eval'` in script-src — asserted against the document
// the dev server actually serves; the pane cannot compile its WASM without it. The production
// policy (colours, cell spans, the exact header) is asserted by terminal.prod.spec.ts, which runs
// against `next build` + `next start` (FIX-098): `next dev` relaxes the CSP.

test.describe("agent Terminal tab", () => {
  test.use({ viewport: { width: 1280, height: 900 } });

  test("the served document's CSP carries 'wasm-unsafe-eval' in script-src (SEC-87)", async ({ request }) => {
    const res = await request.get("/o/acme/w/main/agents/daneel");
    expect(res.status()).toBe(200);
    // script-src alone: the nonce differs per request, so check the directives by name.
    const csp = res.headers()["content-security-policy"] ?? "";
    const scriptSrc = csp.split(";").find((d) => d.trim().startsWith("script-src"));
    expect(scriptSrc).toBeDefined();
    expect(scriptSrc).toContain("'wasm-unsafe-eval'");
  });

  test("tab exists only while the instance enabled the terminal; never for BYO", async ({ page }) => {
    await mockApi(page);
    await page.route("**/v1/**", async (route) => {
      const p = new URL(route.request().url()).pathname;
      const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
      if (p === "/v1/instance") {
        return json({
          version: "0.1.0", signup_mode: "closed", setup_errors: [],
          features: { hosted: true, previews: true, terminal: false },
          previews: { enabled: true, domain: "previews.example.org" },
          oidc_providers: [],
        });
      }
      if (p.endsWith("/agents")) {
        return json({ items: [{ ...agent, kind: "byo" }], next_cursor: null });
      }
      return route.fallback();
    });
    await page.goto("/o/acme/w/main/agents/daneel");
    // (D-200) A BYO agent has no Terminal page: not in any tablist of the page.
    await expect(page.getByRole("tablist", { name: "Agent sections" })).toBeVisible();
    await expect(page.getByRole("tab", { name: "Terminal" })).toHaveCount(0);
  });

  test("renders the pane, sends the layout resize and shows PTY output", async ({ page }) => {
    const pty: MockPty = { output: enc("daneel:/workspace$ "), resizes: [] };
    await mockWithTerminal(page, pty);
    await openTab(page);
    await expect(page.getByRole("tablist", { name: "Computer pages" }).getByRole("tab", { name: "Terminal" })).toBeVisible();
    // The pane (real wterm, DOM renderer) shows the PTY's greeting as selectable text.
    const pane = page.getByRole("group", { name: "Agent terminal" });
    await expect(pane).toBeVisible();
    await expect(pane).toContainText("daneel:/workspace$", { timeout: 20_000 });
    // The pane measured wider than the 80×24 seed, so the layout resize rode the socket with
    // `pty_id: 0` (the browser cannot know the runner's allocated id, C-619).
    await expect.poll(() => pty.resizes, { timeout: 20_000 }).not.toHaveLength(0);
    expect(pty.resizes[0]!.pty_id).toBe(0);
    expect(pty.resizes[0]!.cols).toBeGreaterThan(80);
  });

  test("a frozen agent shows the freeze notice, never a pane", async ({ page }) => {
    const pty: MockPty = { output: new Uint8Array(), resizes: [] };
    await mockWithTerminal(page, pty, { frozen: true });
    await openTab(page);
    await expect(page.getByText(/This agent is frozen/i)).toBeVisible();
    await expect(page.getByRole("group", { name: "Agent terminal" })).toHaveCount(0);
  });

  // FIX-138: a bounded pane that scrolls inside itself.
  for (const width of [1280, 390]) {
    test.describe(`scrolling pane, ${width} px`, () => {
      test.use({ viewport: { width, height: 800 } });
      test("200 lines of output keep the pane height and the page still; the last line is visible; scrolling up offers Jump to latest", async ({ page }) => {
        const lines = Array.from({ length: 200 }, (_, i) => `line ${i + 1}\r\n`).join("");
        const pty: MockPty = { output: enc(lines), resizes: [] };
        await mockWithTerminal(page, pty);
        await openTab(page);
        const pane = page.getByRole("group", { name: "Agent terminal" });
        await expect(pane).toContainText("line 200", { timeout: 20_000 });
        const box = await pane.boundingBox();
        // The pane keeps a bounded height (a fixed number of rows), and so does the page.
        expect(box!.height).toBeLessThan(800);
        expect(box!.height).toBeGreaterThan(200);
        // Desktop: the page itself does not scroll vertically; on a phone the pane is a fixed share of the viewport
        // and the page may scroll to reach it. Never sideways.
        if (width >= 768) {
          expect(await page.evaluate(() => document.scrollingElement!.scrollHeight - window.innerHeight), "the page scrolls").toBeLessThanOrEqual(0);
        }
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        // The last line is in view (the pane follows the cursor).
        const last = await pane.getByText("line 200", { exact: true }).boundingBox();
        expect(last!.y + last!.height).toBeLessThanOrEqual(box!.y + box!.height + 1);
        await expect(page.getByRole("button", { name: "Jump to latest" })).toHaveCount(0);
        // Scrolled up: it stays put and the control appears; the control brings the pane back.
        await pane.evaluate((el) => {
          el.scrollTop = 0;
          el.dispatchEvent(new Event("scroll"));
        });
        const jump = page.getByRole("button", { name: "Jump to latest" });
        await expect(jump).toBeVisible();
        await jump.click();
        await expect(jump).toHaveCount(0);
        expect((await pane.boundingBox())!.height).toBe(box!.height);
      });
    });
  }

  test("Tab reaches the shell; Esc then Tab leaves the terminal (no keyboard trap)", async ({ page }) => {
    const pty: MockPty = { output: enc("$ "), resizes: [], inputs: [] };
    await mockWithTerminal(page, pty);
    await openTab(page);
    const pane = page.getByRole("group", { name: "Agent terminal" });
    await expect(pane).toContainText("$", { timeout: 20_000 });
    await pane.click();
    await page.keyboard.type("ec");
    await page.keyboard.press("Tab");
    await expect.poll(() => pty.inputs!.join("")).toContain("ec\t");
    // The page did not take Tab: the terminal still has focus.
    await expect(pane).toHaveClass(/focused/);
    const before = pty.inputs!.join("");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Tab");
    await expect(page.getByRole("button", { name: "Open in new window" })).toBeFocused();
    expect(pty.inputs!.join("").replace(before, "")).not.toContain("\t");
    await expect(page.getByText("Left the terminal.")).toBeVisible();
  });

  test("Open in new window opens the standalone route without an opener", async ({ page }) => {
    await page.addInitScript(() => {
      (window as unknown as { __opened: unknown[] }).__opened = [];
      window.open = ((...args: unknown[]) => {
        (window as unknown as { __opened: unknown[] }).__opened.push(args);
        return null;
      }) as typeof window.open;
    });
    await mockWithTerminal(page, { output: new Uint8Array(), resizes: [] });
    await openTab(page);
    await page.getByRole("button", { name: "Open in new window" }).click();
    const opened = await page.evaluate(() => (window as unknown as { __opened: unknown[][] }).__opened);
    expect(opened).toEqual([["/o/acme/w/main/agents/daneel/terminal", "_blank", "noopener,noreferrer"]]);
  });

  test("the standalone window route: no app chrome, a titled window, a pane that fills and resizes to the window", async ({ page }) => {
    await page.setViewportSize({ width: 1600, height: 900 });
    const pty: MockPty = { output: enc("daneel:/workspace$ "), resizes: [] };
    await mockWithTerminal(page, pty);
    await page.goto("/o/acme/w/main/agents/daneel/terminal");
    await expect(page.getByRole("heading", { name: "Daneel — Terminal" })).toBeVisible();
    await expect(page.getByRole("complementary", { name: "Sidebar" })).toHaveCount(0);
    await expect(page).toHaveTitle("Daneel — Terminal");
    const pane = page.getByRole("group", { name: "Agent terminal" });
    await expect(pane).toContainText("daneel:/workspace$", { timeout: 20_000 });
    const box = await pane.boundingBox();
    expect(box!.width).toBeGreaterThan(1500);
    expect(box!.height).toBeGreaterThan(700);
    expect(await page.evaluate(() => document.scrollingElement!.scrollHeight - window.innerHeight), "the window scrolls").toBeLessThanOrEqual(0);
    await expect.poll(() => pty.resizes.some((r) => r.cols > 150 && r.rows > 30), { timeout: 20_000 }).toBe(true);
  });

  for (const scheme of ["light", "dark"] as const) {
    for (const width of [1280, 390]) {
      test.describe(`themed ${width} px (${scheme})`, () => {
        test.use({ viewport: { width, height: 800 } });
        test("themed canvas, no horizontal scroll, pane present", async ({ page }) => {
          const pty: MockPty = { output: enc("ready\r\n"), resizes: [] };
          await mockWithTerminal(page, pty);
          await openTab(page, scheme);
          expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
          expect(await overflow(page)).toBeLessThanOrEqual(0);
          await expect(page.getByRole("group", { name: "Agent terminal" })).toBeVisible();
        });
      });
    }
  }
});
