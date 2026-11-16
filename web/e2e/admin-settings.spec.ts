import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { openRoute, overflow } from "./support/open";

// SET-1 (D-236): Admin › Settings and the generic section form in both themes at 1280 and 390 px:
// no serious or critical axe violation, no horizontal scroll, locked settings read-only.

async function axeClean(page: import("@playwright/test").Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("the settings index lists the sections and the locked key", async ({ page }) => {
        await openRoute(page, "/admin/settings", scheme);
        await expect(page.getByRole("heading", { name: "Settings", level: 1 })).toBeVisible();
        await expect(page.getByRole("link", { name: "Governance", exact: true })).toBeVisible();
        await expect(page.getByText(/1 setting is configured in Helm and cannot be changed here/)).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("a section shows a locked setting read-only and the editable ones", async ({ page }) => {
        await openRoute(page, "/admin/settings/governance", scheme);
        await expect(page.getByRole("heading", { name: "Governance", level: 1 })).toBeVisible();
        await expect(page.getByTestId("governance.egress.minimum_posture-locked-value")).toHaveText("restricted");
        await expect(page.getByText("config.governance.egress.minimum_posture").first()).toBeVisible();
        await expect(page.locator("[id='governance.egress.minimum_posture-c']")).toHaveCount(0);
        await expect(page.locator("[id='governance.llm.require_key_origin-c']")).toBeChecked();
        // SET-2: the page lays the registry's keys out in four sections; the keyed-list "Managed by Helm"
        // marker is covered on the LLM endpoints page below.
        await expect(page.getByRole("group", { name: "Agent tokens" })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      // SET-3: providers, the preference order locked by Helm, a Helm endpoint beside a dashboard endpoint.
      test("the LLM endpoints page shows providers, a Helm endpoint and a dashboard endpoint", async ({ page }) => {
        await openRoute(page, "/admin/settings/llm", scheme);
        await expect(page.getByRole("heading", { name: "LLM endpoints", level: 1 })).toBeVisible();
        await expect(page.getByRole("checkbox", { name: "LiteLLM" })).toBeChecked();
        await expect(page.getByRole("checkbox", { name: "OpenAI", exact: true })).not.toBeChecked();
        await expect(page.getByTestId("llm.provider_preference-locked-value")).toContainText("1. LiteLLM");
        await expect(page.getByText("Managed by Helm")).toBeVisible();
        await expect(page.getByRole("button", { name: "Edit team" })).toBeVisible();
        await expect(page.getByRole("button", { name: "Edit central" })).toHaveCount(0);
        await page.getByRole("button", { name: "Edit team" }).click();
        const dialog = page.getByRole("dialog");
        await expect(dialog.getByRole("button", { name: "Test connection" })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      // SET-4: the Helm provider beside a dashboard provider, the provider dialog and the Email page.
      test("the sign-in page lists providers and opens the provider dialog", async ({ page }) => {
        await openRoute(page, "/admin/settings/sign-in", scheme);
        await expect(page.getByRole("heading", { name: "Sign-in", level: 1 })).toBeVisible();
        await expect(page.getByText("Managed by Helm")).toBeVisible();
        await expect(page.getByRole("button", { name: "Edit corp" })).toBeVisible();
        await expect(page.getByRole("button", { name: "Edit helm" })).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
        await page.getByRole("button", { name: "Edit corp" }).click();
        const dialog = page.getByRole("dialog");
        await expect(dialog.getByRole("button", { name: "Test discovery" })).toBeVisible();
        await expect(dialog.getByLabel("Issuer")).toHaveValue("https://corp.example.test/realms/main");
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      // SET-5: the operational pages — the runners' sizes read-only, a terminal the deployment lacks, rate classes.
      test("the agents page shows a locked deadline and the runners' sizes", async ({ page }) => {
        await openRoute(page, "/admin/settings/agents", scheme);
        await expect(page.getByRole("heading", { name: "Agents & sandboxes", level: 1 })).toBeVisible();
        await expect(page.getByTestId("hosted.idle_stop_secs-locked-value")).toHaveText("1800");
        await expect(page.getByRole("region", { name: "Runners and sandbox sizes" })).toContainText("4096");
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("the terminal page explains a missing deployment wiring", async ({ page }) => {
        await openRoute(page, "/admin/settings/terminal", scheme);
        await expect(page.getByRole("heading", { name: "Terminal", level: 1 })).toBeVisible();
        await expect(page.getByText(/does not provide the terminal/)).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("the limits page lists a Helm rate class and warns about private destinations", async ({ page }) => {
        await openRoute(page, "/admin/settings/limits", scheme);
        await expect(page.getByRole("heading", { name: "Limits", level: 1 })).toBeVisible();
        await expect(page.getByText("Managed by Helm")).toBeVisible();
        await expect(page.getByText(/Private destinations\./)).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("the email page offers a test e-mail", async ({ page }) => {
        await openRoute(page, "/admin/settings/email", scheme);
        await expect(page.getByRole("heading", { name: "Email", level: 1 })).toBeVisible();
        await expect(page.getByRole("button", { name: "Send test e-mail" })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });
    });
  }
}
