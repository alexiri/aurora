import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { DEFAULT, MINE, openChat } from "./support/chat-mock";
import { overflow } from "./support/open";

// P5-CHAT-3 (D-228(b)-(d); docs/14 §Agent chat "Composer (phase 5)"): formatting row, shortcuts, Write/Preview,
// fence-aware Enter, emoji, files (drop, paste, files-only send), drafts. 1280 px and 390 px, both themes, axe with
// no rule disabled.

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map(
      (v) =>
        `${v.id}: ${v.nodes
          .map((n) => n.target.join(" "))
          .slice(0, 3)
          .join(" | ")}`,
    );
}

/** The mocked upload route: every POST makes the next attachment. */
async function mockUploads(page: Page) {
  let n = 0;
  await page.route(/\/v1\/conversations\/[^/]+\/attachments$/, async (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    n += 1;
    const name = /filename="([^"]+)"/.exec(route.request().postData() ?? "")?.[1] ?? `file${n}`;
    await route.fulfill({
      status: 201,
      contentType: "application/json",
      body: JSON.stringify({
        id: `00000000-0000-4000-8000-00000000a0${n}`,
        conversation_id: MINE.id,
        filename: name,
        size_bytes: 3,
        content_type: "text/plain",
        direction: "inbound",
        status: "pending",
        created_at: new Date().toISOString(),
      }),
    });
  });
}

async function openComposer(page: Page, scheme: "light" | "dark") {
  await page.addInitScript(() => window.localStorage.setItem("aurora:chat-formatting", "1"));
  await openChat(page, scheme, DEFAULT, `?c=${MINE.id}`);
  await mockUploads(page);
  const box = page.locator("#chat-input");
  await expect(box).toBeVisible();
  return box;
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`${scheme}, 1280 px`, () => {
    test.use({ viewport: { width: 1280, height: 900 } });

    test("format, Preview, send: the request carries the Markdown; the row is a roving toolbar", async ({ page }) => {
      const box = await openComposer(page, scheme);
      await box.fill("hello world");
      await box.evaluate((el: HTMLTextAreaElement) => el.setSelectionRange(0, 5));
      await page.getByRole("button", { name: "Bold (Ctrl+B)" }).click();
      await expect(box).toHaveValue("**hello** world");
      await page.keyboard.press("Control+e");
      await page.getByRole("tab", { name: "Preview" }).click();
      const panel = page.getByRole("tabpanel", { name: "Preview" });
      await expect(panel.locator("strong")).toHaveText("hello");
      await expect(page.getByRole("button", { name: "Bold (Ctrl+B)" })).toBeDisabled();
      expect(await serious(page)).toEqual([]);
      const sent = page.waitForRequest((r) => r.method() === "POST" && /\/messages$/.test(r.url()));
      // (the mocked conversation has a turn in flight, so the button reads "Follow up")
      await page.getByRole("button", { name: /^(Send|Follow up)$/ }).click();
      expect(((await sent).postDataJSON() as { text: string }).text).toBe("**`hello`** world");
      // Roving focus: one tab stop, the arrows move inside the row.
      await page.getByRole("tab", { name: "Write" }).click();
      await expect(box).toBeFocused(); // Write hands focus back to the text field
      const bold = page.getByRole("button", { name: "Bold (Ctrl+B)" });
      await bold.focus();
      await page.keyboard.press("ArrowRight");
      await expect(page.getByRole("button", { name: "Italic (Ctrl+I)" })).toBeFocused();
      await page.keyboard.press("End");
      await expect(page.getByRole("button", { name: "Emoji" })).toBeFocused();
    });

    test("undo after a toolbar action restores the text (Chromium)", async ({ page, browserName }) => {
      test.skip(browserName !== "chromium", "execCommand undo is checked in Chromium here; Firefox/WebKit are noted in the report");
      const box = await openComposer(page, scheme);
      await box.click();
      await page.keyboard.type("keep this");
      await page.keyboard.press("Control+a");
      await page.getByRole("button", { name: "Bold (Ctrl+B)" }).click();
      await expect(box).toHaveValue("**keep this**");
      await box.focus();
      await page.keyboard.press("Control+z");
      await expect(box).toHaveValue("keep this");
    });

    test("Enter sends; inside a fence it is a new line and Ctrl+Enter sends; Shift+Enter continues a list", async ({ page }) => {
      const box = await openComposer(page, scheme);
      await box.click();
      await page.keyboard.type("```ts");
      await page.keyboard.press("Enter");
      await page.keyboard.type("const a = 1;");
      await expect(page.getByTestId("composer-hint")).toContainText("Enter for a new line");
      await page.keyboard.press("Enter");
      await expect(box).toHaveValue("```ts\nconst a = 1;\n");
      const sent = page.waitForRequest((r) => r.method() === "POST" && /\/messages$/.test(r.url()));
      await page.keyboard.press("Control+Enter");
      expect(((await sent).postDataJSON() as { text: string }).text).toContain("const a = 1;");
      await expect(box).toHaveValue("");
      await page.keyboard.type("- one");
      await page.keyboard.press("Shift+Enter");
      await expect(box).toHaveValue("- one\n- ");
    });

    test("emoji: :shortcode suggestions by keyboard, and the picker from the row", async ({ page }) => {
      const box = await openComposer(page, scheme);
      await box.click();
      await page.keyboard.type("great :tad");
      const list = page.getByRole("listbox", { name: "Emoji suggestions" });
      await expect(list).toBeVisible();
      expect(await serious(page)).toEqual([]);
      await page.keyboard.press("Enter");
      await expect(box).toHaveValue("great 🎉");
      await page.getByRole("button", { name: "Emoji" }).click();
      await page.getByRole("button", { name: "rocket" }).click();
      await expect(box).toHaveValue("great 🎉🚀");
      await expect(box).toBeFocused();
    });

    test("files: drop two, paste an image, three chips, files-only send", async ({ page }) => {
      const box = await openComposer(page, scheme);
      const dt = await page.evaluateHandle(() => {
        const d = new DataTransfer();
        d.items.add(new File(["aaa"], "a.txt", { type: "text/plain" }));
        d.items.add(new File(["bbb"], "b.txt", { type: "text/plain" }));
        return d;
      });
      const pane = page.getByRole("region", { name: "Chat" }).or(page.locator('section[aria-label="Chat"]')).first();
      await pane.dispatchEvent("dragenter", { dataTransfer: dt });
      await expect(page.getByTestId("drop-overlay")).toContainText("Drop files to attach");
      expect(await serious(page)).toEqual([]);
      await pane.dispatchEvent("drop", { dataTransfer: dt });
      await expect(page.getByTestId("drop-overlay")).toHaveCount(0);
      // A real ClipboardEvent carrying the image the way a screenshot paste does (named `image.png`).
      await box.evaluate((el) => {
        const d = new DataTransfer();
        d.items.add(new File(["png"], "image.png", { type: "image/png" }));
        el.dispatchEvent(new ClipboardEvent("paste", { clipboardData: d, bubbles: true, cancelable: true }));
      });
      const chips = page.getByRole("list", { name: "Pending attachments" }).getByRole("listitem");
      await expect(chips).toHaveCount(3);
      await expect(chips.nth(2)).toContainText(/Pasted image \d{4}-\d{2}-\d{2} \d{2}\.\d{2}\.\d{2}\.png/);
      const sent = page.waitForRequest((r) => r.method() === "POST" && /\/messages$/.test(r.url()));
      await page.getByRole("button", { name: /^(Send|Follow up)$/ }).click();
      const body = (await sent).postDataJSON() as { text: string; attachment_ids: string[] };
      expect(body.text).toBe("");
      expect(body.attachment_ids).toHaveLength(3);
    });

    test("a reload keeps the draft", async ({ page }) => {
      const box = await openComposer(page, scheme);
      await box.fill("half-written thought");
      await expect.poll(() => page.evaluate(() => Object.keys(window.localStorage).some((k) => k.startsWith("aurora:chat-draft:")))).toBe(true);
      await page.reload();
      await expect(page.locator("#chat-input")).toHaveValue("half-written thought");
    });

    test("axe: formatting row, picker open", async ({ page }) => {
      await openComposer(page, scheme);
      await page.getByRole("button", { name: "Emoji" }).click();
      await expect(page.getByRole("button", { name: "rocket" })).toBeVisible();
      expect(await serious(page)).toEqual([]);
    });
  });

  test.describe(`${scheme}, 390 px`, () => {
    test.use({ viewport: { width: 390, height: 844 } });

    test("two rows, 44 px controls, the formatting strip scrolls inside itself, no page scroll, axe clean", async ({ page }) => {
      const box = await openComposer(page, scheme);
      await expect(box).toBeVisible();
      const sizes = await page.evaluate(() =>
        ["Attach files", "Formatting", "Follow up"].map((n) => {
          const el = document.querySelector<HTMLElement>(`[aria-label="${n}"]`);
          const r = (n === "Attach files" ? el?.closest("label") : el)?.getBoundingClientRect();
          return [n, Math.round(r?.width ?? 0), Math.round(r?.height ?? 0)];
        }),
      );
      for (const [, w, h] of sizes) {
        expect(w).toBeGreaterThanOrEqual(44);
        expect(h).toBeGreaterThanOrEqual(44);
      }
      const strip = page.getByTestId("formatting-row").locator("> div");
      expect(await strip.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
      expect(await overflow(page)).toBeFalsy();
      await box.fill("a :tad");
      await box.press("End");
      await expect(page.getByRole("listbox", { name: "Emoji suggestions" })).toBeVisible();
      expect(await overflow(page)).toBeFalsy();
      expect(await serious(page)).toEqual([]);
    });
  });
}
