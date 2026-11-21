import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { mockApi, now } from "./support/mock-api";
import { overflow, titled, type Scheme } from "./support/open";

// FIX-171 (D-373, C-1027): a row's Details shows what it recorded; the e-mail's link lands on the filtered log.

const LONG = "https://llm.internal.example.test/" + "very-long-path-segment-".repeat(12);
const row = {
  id: "e0000000-0000-4000-8000-000000000001", org_id: null, workspace_id: null, actor_kind: "instance_admin", actor_id: "a0000000-0000-4000-8000-000000000002",
  actor_label: "root@example.test", action: "instance_settings.update", target_kind: "instance_settings", target_id: "llm", outcome: "success",
  source: "dashboard", ip: "10.1.2.3", critical: true, created_at: now,
  metadata: {
    section: "llm", generation: 4,
    change: [
      { key: "llm.endpoints", entry_id: "eu", before: { url: "https://old.example.test", model: "m1" }, after: { url: LONG, model: "m1" } },
      { key: "llm.api_key", before: "(secret)", after: "(secret changed)" },
    ],
  },
};

async function open(page: Page, scheme: Scheme, seen: string[]) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  await page.route("**/v1/admin/audit?**", (route) => {
    seen.push(new URL(route.request().url()).search);
    return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ items: [row], next_cursor: null }) });
  });
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto("/admin/audit?action=instance_settings.");
  await page.locator("h1, h2").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await titled(page);
  return errors;
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("the old audit link lands on Data › Audit log with the filter filled in; Details shows before and after", async ({ page }) => {
        const seen: string[] = [];
        const errors = await open(page, scheme, seen);
        await expect(page).toHaveURL(/\/admin\/data\?(?=.*tab=audit)(?=.*action=instance_settings\.)/);
        // (P5-REFINE-20, admin-10) One "All actions" select in words; the area from the link shows by its name.
        await expect(page.getByLabel("Action")).toHaveValue("instance_settings.");
        await expect(page.getByLabel("Action").locator("option:checked")).toHaveText("Instance settings");
        expect(new URLSearchParams(seen[0]).get("action")).toBe("instance_settings.");

        // (P5-REFINE-18 #11) The entry reads as a sentence; Details opens the raw event under Technical details.
        await expect(page.getByTestId("audit-sentence")).toContainText("changed");
        await page.getByRole("button", { name: "Details" }).click();
        await page.getByText("Technical details", { exact: true }).click();
        const detail = page.getByTestId("audit-detail");
        await expect(detail).toContainText("llm.endpoints › eu");
        await expect(detail).toContainText("https://old.example.test");
        await expect(detail).toContainText("(secret changed)");
        await expect(detail).toContainText("10.1.2.3");
        expect(await overflow(page), "horizontal page scroll with a long value open").toBeLessThanOrEqual(0);
        const { violations } = await new AxeBuilder({ page }).analyze();
        expect(violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id)).toEqual([]);
        expect(errors).toEqual([]);
      });
    });
  }
}
