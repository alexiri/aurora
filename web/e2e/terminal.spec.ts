import { expect, test, type Page, type WebSocketRoute } from "@playwright/test";

import { agent, mockApi, org } from "./support/mock-api";
import { CANVAS, overflow, type Scheme } from "./support/open";

// P4-TERM (docs/14 §Terminal tab, docs/18 §Web terminal): the agent Terminal tab against the
// shared mock. Tab presence follows the instance switch (`features.terminal`, C-622) and the
// hosted kind; the pane is the real wterm + Ghostty WASM in real Chromium (jsdom has neither
// WASM nor layout), fed by a mocked WebSocket that speaks the leg-1 protocol (C-619): binary
// frames are PTY bytes, `pty.resize` is the one text control, `pty_id: 0` names the leg's PTY.
// (SEC-87) The CSP carries `'wasm-unsafe-eval'` in script-src — asserted against the document
// the dev server actually serves; the pane cannot compile its WASM without it.

interface MockPty {
  /** Bytes the PTY "printed" — sent to the pane on open. */
  output: Uint8Array;
  /** The `pty.resize` controls the pane sent, in order. */
  resizes: { pty_id: number; cols: number; rows: number }[];
}

/** The leg-1 server half: replays the greeting, records the pane's resize controls. */
function servePty(ws: WebSocketRoute, pty: MockPty) {
  ws.onMessage((message) => {
    if (typeof message === "string") {
      const control = JSON.parse(message) as { kind?: string; pty_id?: number; cols?: number; rows?: number };
      if (control.kind === "pty.resize") {
        pty.resizes.push({ pty_id: control.pty_id ?? -1, cols: control.cols ?? 0, rows: control.rows ?? 0 });
      }
      return;
    }
    // Binary keystrokes are accepted and dropped (the mock shell does not echo).
  });
  if (pty.output.length > 0) ws.send(Buffer.from(pty.output));
}

/** The shared mock plus a leg-1 terminal WebSocket at `/v1/agents/{id}/terminal`. */
async function mockWithTerminal(page: Page, pty: MockPty, over: { frozen?: boolean } = {}) {
  await mockApi(page);
  await page.route("**/v1/**", async (route) => {
    const p = new URL(route.request().url()).pathname;
    const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
    if (over.frozen && p.endsWith("/agents")) {
      return json({
        items: [{ ...agent, frozen: { at: "2026-01-02T00:00:00Z", authority: "workspace_owner", by_label: "ada@example.test", level: "agent", reason: "Runaway loop" } }],
        next_cursor: null,
      });
    }
    if (p === `/v1/orgs/${org.id}`) return json(org);
    return route.fallback();
  });
  await page.routeWebSocket(/\/v1\/agents\/[^/]+\/terminal$/, (ws) => servePty(ws, pty));
}

async function openTab(page: Page, scheme: Scheme = "light", query = "?tab=terminal") {
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(`/o/acme/w/main/agents/daneel${query}`);
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await page.waitForLoadState("networkidle");
}

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
    const tabs = page.getByRole("tablist", { name: "Agent" });
    await expect(tabs.getByRole("tab", { name: "Terminal" })).toHaveCount(0);
  });

  test("renders the pane, sends the layout resize and shows PTY output", async ({ page }) => {
    const pty: MockPty = { output: new TextEncoder().encode("daneel:/workspace$ "), resizes: [] };
    await mockWithTerminal(page, pty);
    await openTab(page);
    await expect(page.getByRole("tablist", { name: "Agent" }).getByRole("tab", { name: "Terminal" })).toBeVisible();
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

  test("a frozen agent shows the pause notice, never a pane", async ({ page }) => {
    const pty: MockPty = { output: new Uint8Array(), resizes: [] };
    await mockWithTerminal(page, pty, { frozen: true });
    await openTab(page);
    await expect(page.getByText(/paused/i)).toBeVisible();
    await expect(page.getByRole("group", { name: "Agent terminal" })).toHaveCount(0);
  });

  for (const scheme of ["light", "dark"] as const) {
    for (const width of [1280, 390]) {
      test.describe(`themed ${width} px (${scheme})`, () => {
        test.use({ viewport: { width, height: 800 } });
        test("themed canvas, no horizontal scroll, pane present", async ({ page }) => {
          const pty: MockPty = { output: new TextEncoder().encode("ready\r\n"), resizes: [] };
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
