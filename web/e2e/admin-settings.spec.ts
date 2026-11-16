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
        await expect(page.getByRole("heading", { name: "All settings", level: 1 })).toBeVisible();
        await expect(page.getByRole("link", { name: "Instance rules", exact: true })).toBeVisible();
        await page.getByRole("searchbox", { name: "Search settings" }).fill("no such setting");
        await expect(page.getByRole("link", { name: "Instance rules", exact: true })).toHaveCount(0);
        await page.getByRole("searchbox", { name: "Search settings" }).fill("");
        await expect(page.getByText(/1 setting is configured in Helm and cannot be changed here/)).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("a section shows a locked setting read-only and the editable ones", async ({ page }) => {
        // The section lives on AI models › Rules; the old address redirects there (C-960, C-971).
        await openRoute(page, "/admin/settings/governance", scheme);
        await expect(page).toHaveURL(/\/admin\/ai-models\?tab=rules$/);
        await expect(page.getByRole("heading", { name: "AI models", level: 1 })).toBeVisible();
        await expect(page.locator("[id='governance.llm.require_key_origin-c']")).toBeChecked();
        // The egress and BYO keys of the same section are not on this page (C-971: three topics).
        await expect(page.locator("[id='governance.egress.minimum_posture-c']")).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
        // Administration › Internet access holds the egress keys: the Helm-locked one reads as text.
        await openRoute(page, "/admin/internet-access", scheme);
        await expect(page.getByRole("heading", { name: "Internet access", level: 1 })).toBeVisible();
        // The locked value reads in words, never the stored id (D-341).
        await expect(page.getByTestId("governance.egress.minimum_posture-locked-value")).toHaveText("Approved websites only");
        await expect(page.getByText("config.governance.egress.minimum_posture").first()).toBeVisible();
        await expect(page.locator("[id='governance.egress.minimum_posture-c']")).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
        // Administration › Agents › BYO holds the BYO agents and agent tokens groups.
        await openRoute(page, "/admin/agents?tab=byo", scheme);
        await expect(page.getByRole("group", { name: "Agent tokens" })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      // P5-IA-5 (C-972): the BYO choice reads in words, as choice cards with a sentence each, never `owners_only`.
      test("the BYO choice reads in words", async ({ page }) => {
        await openRoute(page, "/admin/agents?tab=byo", scheme);
        const group = page.getByRole("radiogroup", { name: "Who may connect their own agent" });
        await expect(group.getByRole("radio")).toHaveCount(3);
        await expect(group.getByText("Only workspace owners may connect their own agent.")).toBeVisible();
        await expect(group.getByRole("radio", { name: /Workspace owners only/ })).toBeChecked();
        await expect(page.getByText("Default: Workspace owners only")).toBeVisible();
        await expect(page.getByText("owners_only")).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      // P5-IA-5 (C-973): a section that is not editable yet shows its current values read-only, never an empty form.
      test("a section that is not editable shows its current values read-only", async ({ page }) => {
        await openRoute(page, "/admin/data?tab=retention", scheme);
        await expect(page.getByTestId("retention.request_log_days-current")).toHaveText("30 days");
        await expect(page.getByText("Change them in your Helm values")).toBeVisible();
        await expect(page.getByText("No settings in this section yet")).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      // SET-3: providers, the preference order locked by Helm, a Helm endpoint beside a dashboard endpoint.
      test("the LLM endpoints page shows providers, a Helm endpoint and a dashboard endpoint", async ({ page }) => {
        await openRoute(page, "/admin/settings/llm", scheme);
        await expect(page).toHaveURL(/\/admin\/ai-models\?tab=endpoints$/);
        await expect(page.getByRole("heading", { name: "AI models", level: 1 })).toBeVisible();
        await expect(page.getByRole("checkbox", { name: "LiteLLM" })).toBeChecked();
        await expect(page.getByRole("checkbox", { name: "OpenAI", exact: true })).not.toBeChecked();
        await expect(page.getByTestId("llm.provider_preference-locked-value")).toContainText("1. LiteLLM");
        await expect(page.getByText("Managed by Helm")).toBeVisible();
        await expect(page.getByRole("button", { name: "Edit team" })).toBeVisible();
        await expect(page.getByRole("button", { name: "Edit central" })).toHaveCount(0);
        // P4-LLM-6: harness badges, the no-harness notice and the private-address mark with its link to Limits.
        await expect(page.getByLabel("Harnesses this endpoint serves").getByText("Claude Code")).toBeVisible();
        await expect(page.getByText("No enabled harness speaks this API.")).toBeVisible();
        await expect(page.getByRole("link", { name: "Private address not allowed" })).toBeVisible();
        await page.getByRole("button", { name: "Edit team" }).click();
        const dialog = page.getByRole("dialog");
        await expect(dialog.getByRole("button", { name: "Test connection" })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      // SET-4: the Helm provider beside a dashboard provider, the provider dialog and the Email page.
      test("the sign-in page lists providers and opens the provider dialog", async ({ page }) => {
        await openRoute(page, "/admin/settings/sign-in", scheme);
        await expect(page).toHaveURL(/\/admin\/people\?tab=sign-in$/);
        await expect(page.getByRole("heading", { name: "People", level: 1 })).toBeVisible();
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
        await expect(page).toHaveURL(/\/admin\/agents\?tab=defaults$/);
        await expect(page.getByRole("heading", { name: "Agents", level: 1 })).toBeVisible();
        await expect(page.getByTestId("hosted.idle_stop_secs-locked-value")).toHaveText("30 minutes");
        await expect(page.getByRole("region", { name: "Runners and sandbox sizes" })).toContainText("4096");
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("the terminal page explains a missing deployment wiring", async ({ page }) => {
        await openRoute(page, "/admin/settings/terminal", scheme);
        await expect(page).toHaveURL(/\/admin\/agents\?tab=computers$/);
        await expect(page.getByRole("heading", { name: "Agents", level: 1 })).toBeVisible();
        await expect(page.getByText(/does not provide the terminal/)).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("the limits page lists a Helm rate class and warns about private destinations", async ({ page }) => {
        await openRoute(page, "/admin/settings/limits", scheme);
        await expect(page).toHaveURL(/\/admin\/platform\?tab=limits$/);
        await expect(page.getByRole("heading", { name: "Platform", level: 1 })).toBeVisible();
        await expect(page.getByText("Managed by Helm")).toBeVisible();
        await expect(page.getByText(/Private destinations\./)).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("the email page offers a test e-mail", async ({ page }) => {
        await openRoute(page, "/admin/settings/email", scheme);
        await expect(page).toHaveURL(/\/admin\/platform\?tab=email$/);
        await expect(page.getByRole("heading", { name: "Platform", level: 1 })).toBeVisible();
        await expect(page.getByRole("button", { name: "Send test e-mail" })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });
    });
  }
}
