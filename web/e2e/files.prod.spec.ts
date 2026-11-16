import { expect, test } from "@playwright/test";

import { Fs, HTML_WITH_SCRIPT, mockFiles, openFiles } from "./support/files";

// P5-FILES-3 (D-316(g), C-905) against the PRODUCTION build (`pnpm e2e:prod`), the only place the real Content-Security-
// Policy applies: CodeMirror's `<style>` elements must carry the page's nonce (`EditorView.cspNonce`), so the editor is
// styled with no violation and no loosened policy; an HTML file is shown as text and never runs.

test.use({ viewport: { width: 1280, height: 900 } });

for (const scheme of ["light", "dark"] as const) {
  test(`${scheme} theme: the editor renders styled under the production CSP with no violation`, async ({ page }) => {
    const violations: string[] = [];
    page.on("console", (m) => {
      if (/content security policy/i.test(m.text())) violations.push(m.text());
    });
    const fs = new Fs();
    fs.nodes.set("/workspace/page.html", { kind: "file", content: HTML_WITH_SCRIPT, mtime: "2026-11-15T10:00:00Z" });
    await mockFiles(page, fs);
    await openFiles(page, scheme);
    await page.getByRole("link", { name: "src", exact: true }).click();
    await page.getByRole("button", { name: "main.rs", exact: true }).click();
    await expect(page.locator(".cm-content")).toContainText("fn main()", { timeout: 30_000 });
    // The theme's rules applied (a style element with the nonce reached the page, and the gutter has its token colour).
    expect(await page.evaluate(() => [...document.querySelectorAll("style")].some((s) => s.nonce !== ""))).toBe(true);
    const gutter = await page.locator(".cm-gutters").evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(gutter).not.toBe("rgba(0, 0, 0, 0)");
    await page.getByRole("button", { name: "Back to folder" }).click();
    await page.getByRole("link", { name: "workspace" }).click();
    await page.getByRole("button", { name: "page.html", exact: true }).click();
    await expect(page.locator(".cm-content")).toContainText("<script>window.__html_ran = true</script>");
    expect(await page.evaluate(() => (window as unknown as Record<string, unknown>).__html_ran)).toBeUndefined();
    expect(violations).toEqual([]);
  });
}
