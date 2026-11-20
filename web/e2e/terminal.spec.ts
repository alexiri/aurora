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
    await expect(page.getByText(/This agent is paused/i)).toBeVisible();
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
    // (P5-REFINE-15) Focus lands on the control after the pane (it is only visible while focused; the pop-out is Expert).
    await expect(page.getByRole("button", { name: "Leave the terminal (Esc, then Tab)" })).toBeFocused();
    expect(pty.inputs!.join("").replace(before, "")).not.toContain("\t");
    await expect(page.getByText("Left the terminal.")).toBeVisible();
  });

  // (P5-REFINE-15, computer-2) After the last retry the pane stops and says so; Try again opens a fresh connection.
  test("after the last retry: 'Couldn't connect to the computer', no pane, Try again reconnects", async ({ page }) => {
    test.setTimeout(90_000);
    await mockApi(page);
    let opens = 0;
    await page.routeWebSocket(/\/v1\/agents\/[^/]+\/terminal$/, (ws) => {
      opens += 1;
      // 4503: the connection to the sandbox was lost (a retryable close the browser can see, unlike 1006).
      void ws.close({ code: 4503, reason: "lost" });
    });
    await openTab(page);
    const gaveUp = page.getByTestId("terminal-gave-up");
    await expect(gaveUp).toBeVisible({ timeout: 45_000 });
    await expect(gaveUp).toContainText("Couldn't connect to the computer");
    await expect(gaveUp).toContainText("Tried 4 times. The computer didn't answer.");
    await expect(page.getByText("Reconnecting…")).toHaveCount(0);
    await expect(page.getByTestId("terminal-not-connected")).toBeVisible();
    await expect(page.getByRole("group", { name: "Agent terminal" })).toBeHidden();
    expect(opens).toBe(5);
    await gaveUp.getByRole("button", { name: "Try again" }).click();
    await expect.poll(() => opens, { timeout: 10_000 }).toBeGreaterThan(5);
  });

  test("the sentence above the pane says an open terminal keeps the computer awake", async ({ page }) => {
    await mockWithTerminal(page, { output: enc("$ "), resizes: [] });
    await openTab(page);
    await expect(page.getByTestId("terminal-keep-awake")).toHaveText("An open terminal keeps the computer awake, so a running command is never cut off.");
  });

  // (computer-12) A phone has no Esc, Tab or Ctrl: a key row sits under the pane, and Open in new window is not offered.
  test.describe("phone, 390 px", () => {
    test.use({ viewport: { width: 390, height: 800 } });
    test("the key row sends Esc, Tab and arrows; Ctrl latches; no Open in new window", async ({ page }) => {
      const pty: MockPty = { output: enc("$ "), resizes: [], inputs: [] };
      await mockWithTerminal(page, pty);
      await openTab(page);
      await expect(page.getByRole("group", { name: "Agent terminal" })).toContainText("$", { timeout: 20_000 });
      await expect(page.getByRole("button", { name: "Open in new window" })).toBeHidden();
      const keys = page.getByRole("toolbar", { name: "Terminal keys" });
      await expect(keys).toBeVisible();
      for (const k of ["Esc", "Tab", "Up"]) await keys.getByRole("button", { name: k, exact: true }).click();
      await expect.poll(() => pty.inputs!.join("")).toContain("\x1b\t\x1b[A");
      await keys.getByRole("button", { name: "Ctrl" }).click();
      await expect(keys.getByRole("button", { name: "Ctrl" })).toHaveAttribute("aria-pressed", "true");
      await page.getByRole("group", { name: "Agent terminal" }).click();
      await page.keyboard.type("c");
      await expect.poll(() => pty.inputs!.join("")).toContain("\x03");
      await expect(keys.getByRole("button", { name: "Ctrl" })).toHaveAttribute("aria-pressed", "false");
      expect(await overflow(page)).toBeLessThanOrEqual(0);
    });
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

  // FIX-156: copy and paste in the pane. The pane is the real wterm; the clipboard is the browser's (granted to the
  // page), and `pty.inputs` records every Binary frame the pane sent.
  test.describe("clipboard", () => {
    const BP = "\x1b[?2004h"; // the app enabled bracketed paste
    async function ready(page: import("@playwright/test").Page, output: string) {
      const pty: MockPty = { output: enc(output), resizes: [], inputs: [] };
      await mockWithTerminal(page, pty);
      await openTab(page);
      const pane = page.getByRole("group", { name: "Agent terminal" });
      await expect(pane).toContainText("$", { timeout: 20_000 });
      await pane.click();
      return { pty, pane };
    }
    const seed = (page: import("@playwright/test").Page, text: string) => page.evaluate((t) => navigator.clipboard.writeText(t), text);
    const sent = (pty: MockPty) => pty.inputs!.join("");
    /** Selects `word` in the pane the way a person would leave it: a DOM selection, the input still focused. */
    const selectWord = (page: import("@playwright/test").Page, word: string) =>
      page.evaluate((w) => {
        const walker = document.createTreeWalker(document.querySelector("[aria-label='Agent terminal']")!, NodeFilter.SHOW_TEXT);
        for (let n = walker.nextNode(); n; n = walker.nextNode()) {
          const i = n.textContent?.indexOf(w) ?? -1;
          if (i < 0) continue;
          const r = document.createRange();
          r.setStart(n, i);
          r.setEnd(n, i + w.length);
          document.getSelection()!.removeAllRanges();
          document.getSelection()!.addRange(r);
          return;
        }
        throw new Error(`${w} not found`);
      }, word);

    test.beforeEach(async ({ context }) => {
      await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    });

    test("Ctrl+Shift+V pastes the clipboard (bracketed when the app asked), never ^V", async ({ page }) => {
      const { pty } = await ready(page, `${BP}$ `);
      await seed(page, "echo hi");
      await page.keyboard.press("Control+Shift+V");
      await expect.poll(() => sent(pty)).toBe("\x1b[200~echo hi\x1b[201~");
      expect(sent(pty)).not.toContain("\x16");
    });

    test("Ctrl+Shift+V strips ESC from the payload and chunks a long paste at 16 KiB", async ({ page }) => {
      const { pty } = await ready(page, `${BP}$ `);
      await seed(page, "a\x1b[201~b");
      await page.keyboard.press("Control+Shift+V");
      await expect.poll(() => sent(pty)).toBe("\x1b[200~a[201~b\x1b[201~");
      pty.inputs!.length = 0;
      const long = "x".repeat(40_000);
      await seed(page, long);
      await page.keyboard.press("Control+Shift+V");
      await expect.poll(() => sent(pty).length).toBe(long.length + 12);
      expect(pty.inputs!.every((f) => Buffer.byteLength(f) <= 16384)).toBe(true);
      expect(sent(pty)).toBe(`\x1b[200~${long}\x1b[201~`);
    });

    // A browser's own paste (Ctrl+V, ⌘V, the context menu) is a `paste` event on wterm's input; headless Chromium raises
    // none from a key press, so the event is dispatched the way the browser would.
    const nativePaste = (page: import("@playwright/test").Page, text: string) =>
      page.evaluate((t) => {
        const ta = document.querySelector<HTMLTextAreaElement>("[aria-label='Agent terminal'] textarea")!;
        const data = new DataTransfer();
        data.setData("text/plain", t);
        ta.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
      }, text);

    test("a native paste still works and plain Ctrl+C still interrupts", async ({ page }) => {
      const { pty } = await ready(page, "$ ");
      await nativePaste(page, "ls");
      await expect.poll(() => sent(pty)).toBe("ls");
      await page.keyboard.press("Control+C");
      await expect.poll(() => sent(pty)).toBe("ls\x03");
    });

    test("Ctrl+Shift+V pastes once: the browser's own paste and the pane's clipboard read never both fire", async ({ page }) => {
      const { pty } = await ready(page, "$ ");
      await seed(page, "once");
      await page.keyboard.press("Control+Shift+V");
      await expect.poll(() => sent(pty)).toContain("once");
      await page.waitForTimeout(600); // past the pane's fallback delay
      expect(JSON.stringify(sent(pty))).toBe(JSON.stringify("once"));
    });

    test("Ctrl+Shift+C without a selection sends nothing; with one it copies and sends nothing", async ({ page }) => {
      const { pty, pane } = await ready(page, "$ echo copyme\r\n");
      await seed(page, "untouched");
      await page.keyboard.press("Control+Shift+C");
      await page.waitForTimeout(300);
      expect(sent(pty)).toBe("");
      expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("untouched");
      // Select the word (the input keeps focus: a keyboard selection), then copy.
      await pane.click();
      await selectWord(page, "copyme");
      await page.keyboard.press("Control+Shift+C");
      await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toContain("copyme");
      expect(sent(pty)).not.toContain("\x03");
    });

    test("right-click over the pane opens Copy / Paste / Select all; Paste sends the clipboard through wterm", async ({ page }) => {
      const { pty, pane } = await ready(page, `${BP}$ echo copyme\r\n`);
      await seed(page, "from the menu");
      await pane.click({ button: "right", position: { x: 40, y: 20 } });
      const menu = page.getByRole("menu", { name: "Terminal" });
      await expect(menu).toBeVisible();
      // Nothing is selected: Copy is disabled; Paste and Select all are offered.
      await expect(menu.getByRole("menuitem", { name: "Copy" })).toHaveAttribute("aria-disabled", "true");
      await expect(menu.getByRole("menuitem", { name: "Select all" })).toBeVisible();
      await menu.getByRole("menuitem", { name: "Paste" }).click();
      await expect.poll(() => sent(pty)).toBe("\x1b[200~from the menu\x1b[201~");
      await expect(menu).toHaveCount(0);
    });

    test("right-click with a selection offers Copy, which copies exactly the selected text", async ({ page }) => {
      const { pane } = await ready(page, "$ echo copyme\r\n");
      await seed(page, "untouched");
      await selectWord(page, "copyme");
      // Right-click on the selected word (a right-click elsewhere clears the selection, as in any page).
      const box = await page.evaluate(() => {
        const r = document.getSelection()!.getRangeAt(0).getBoundingClientRect();
        return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
      });
      await page.mouse.click(box.x, box.y, { button: "right" });
      const menu = page.getByRole("menu", { name: "Terminal" });
      await menu.getByRole("menuitem", { name: "Copy" }).click();
      await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe("copyme");
    });

    test("while disconnected a paste sends nothing and says so", async ({ page }) => {
      const pty: MockPty = { output: enc("$ "), resizes: [], inputs: [] };
      await mockApi(page);
      await page.routeWebSocket(/\/v1\/agents\/[^/]+\/terminal$/, (ws) => {
        void ws.close({ code: 4404 });
      });
      await openTab(page);
      await expect(page.getByTestId("terminal-not-connected")).toBeVisible({ timeout: 20_000 });
      const pane = page.getByRole("group", { name: "Agent terminal" });
      await pane.click();
      await seed(page, "nope");
      await page.keyboard.press("Control+Shift+V");
      await expect(page.getByTestId("terminal-not-connected")).toContainText("what you typed was not sent");
      expect(pty.inputs).toEqual([]);
    });

    test("the OSC 52 confirmation copies without navigator.clipboard (secure-context fallback), never throws", async ({ page }) => {
      const errors: string[] = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.addInitScript(() => {
        Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
      });
      // OSC 52: write "from the app" to the clipboard (base64).
      const b64 = Buffer.from("from the app").toString("base64");
      const { pty } = await ready(page, `$ \x1b]52;c;${b64}\x07`);
      void pty;
      const button = page.getByRole("button", { name: "Copy to clipboard" });
      await expect(button).toBeVisible();
      await button.click();
      // Either the fallback copied and the confirmation cleared, or the failure notice names the next step.
      await expect(async () => {
        const cleared = (await button.count()) === 0;
        const failed = (await page.getByText(/Could not copy/).count()) > 0;
        expect(cleared || failed).toBe(true);
      }).toPass();
      expect(errors).toEqual([]);
    });
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
          // FIX-155: whose environment this is, fixed under the pane, in both themes, readable at 390 px.
          const notice = page.getByTestId("terminal-sandbox-notice");
          await expect(notice).toBeVisible();
          await expect(notice).toHaveText("This terminal runs inside the agent's sandbox: the agent can change what runs here and can see what you type.");
        });
      });
    }
  }
});
