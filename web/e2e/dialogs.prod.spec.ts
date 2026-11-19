import { expect, test } from "@playwright/test";

import { mockApi } from "./support/mock-api";

// P5-POLISH-1 (D-393, F3): under the production CSP (`style-src 'self' 'nonce-…'`) opening and closing a menu and a dialog
// logs no violation. The `<style>` that `react-remove-scroll-bar` injects for a modal must carry the page's nonce.

for (const scheme of ["light", "dark"] as const) {
  test(`${scheme} theme: menus and dialogs open and close with no CSP violation`, async ({ page }) => {
    const bad: string[] = [];
    page.on("console", (m) => {
      // "Failed to load resource" is a request the mock leaves unanswered; a CSP block is reported as "Refused to …".
      if (/^Failed to load resource/.test(m.text())) return;
      if (m.type() === "error" || /content security policy|violates|refused to/i.test(m.text())) bad.push(m.text());
    });
    await page.addInitScript(() => {
      document.addEventListener("securitypolicyviolation", (e) => {
        console.error(`securitypolicyviolation ${e.violatedDirective} ${e.blockedURI}`);
      });
    });
    await mockApi(page);
    await page.emulateMedia({ colorScheme: scheme });
    await page.goto("/o/acme/w/main/agents/daneel");
    await page.getByRole("button", { name: "Agent actions" }).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await page.getByRole("menuitem", { name: /Delete agent/ }).click();
    const dialog = page.getByRole("dialog", { name: "Delete this agent" });
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Close" }).click();
    await expect(dialog).toBeHidden();
    expect(bad).toEqual([]);
  });
}
