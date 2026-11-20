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
        // (P5-REFINE-18) The line names the count and links to Set in Helm.
        await expect(page.getByText(/1 setting is set in Helm\./)).toBeVisible();
        await expect(page.getByRole("link", { name: "See them on Set in Helm" })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("a section shows a locked setting read-only and the editable ones", async ({ page }) => {
        // The section lives on What agents may do › AI keys and rules; the old address redirects there (C-960, C-971).
        await openRoute(page, "/admin/settings/governance", scheme);
        await expect(page).toHaveURL(/\/admin\/internet-access\?tab=ai-keys$/);
        await expect(page.getByRole("heading", { name: "What agents may do", level: 1 })).toBeVisible();
        await expect(page.locator("[id='governance.llm.require_key_origin-c']")).toBeChecked();
        // The egress and BYO keys of the same section are not on this page (C-971: three topics).
        await expect(page.locator("[id='governance.egress.minimum_posture-c']")).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
        // What agents may do › Internet access holds the egress keys: the Helm-locked one reads as text.
        await openRoute(page, "/admin/internet-access", scheme);
        await expect(page.getByRole("heading", { name: "What agents may do", level: 1 })).toBeVisible();
        // (P5-REFINE-18, D-410) The preset card first, saying what Helm keeps; today's fields are behind Customise.
        await expect(page.getByTestId("preset-state")).toContainText("set in Helm");
        await page.getByRole("button", { name: /Customise/ }).click();
        // The locked value reads in words, never the stored id (D-341).
        await expect(page.getByTestId("governance.egress.minimum_posture-locked-value")).toHaveText("Approved websites only");
        await expect(page.getByText("config.governance.egress.minimum_posture").first()).toBeVisible();
        await expect(page.locator("[id='governance.egress.minimum_posture-c']")).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
        // What agents may do › Bring your own holds the BYO agents and agent tokens groups (the old Agents address redirects).
        await openRoute(page, "/admin/agents?tab=byo", scheme);
        await expect(page).toHaveURL(/\/admin\/internet-access\?tab=byo$/);
        await expect(page.getByRole("heading", { name: "Agent tokens" })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      // P5-IA-5 (C-972): the BYO choice reads in words, as choice cards with a sentence each, never `owners_only`.
      test("the BYO choice reads in words", async ({ page }) => {
        await openRoute(page, "/admin/internet-access?tab=byo", scheme);
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
        await expect(page).toHaveURL(/\/admin\/ai-models$/);
        await expect(page.getByRole("heading", { name: "AI endpoints", level: 1 })).toBeVisible();
        await expect(page.getByRole("checkbox", { name: "LiteLLM" })).toBeChecked();
        await expect(page.getByRole("checkbox", { name: "OpenAI", exact: true })).not.toBeChecked();
        await expect(page.getByTestId("llm.provider_preference-locked-value")).toContainText("1. LiteLLM");
        await expect(page.getByText("Set by the installation").first()).toBeVisible();
        await expect(page.getByRole("button", { name: "Edit team" })).toBeVisible();
        await expect(page.getByRole("button", { name: "Edit central" })).toHaveCount(0);
        // P4-LLM-6: harness badges, the no-harness notice and the private-address mark with its link to Limits.
        await expect(page.getByLabel("Harnesses this endpoint serves").getByText("Claude Code")).toBeVisible();
        await expect(page.getByText("No enabled harness speaks this API.")).toBeVisible();
        await expect(page.getByRole("link", { name: "Private address not allowed" })).toBeVisible();
        await page.getByRole("button", { name: "Edit team" }).click();
        const dialog = page.getByRole("dialog");
        await expect(dialog.getByRole("button", { name: "Test connection" })).toBeVisible();
        // P5-MODELS-3 (C-875): roles and model details, and the no-roles notice of a non-Anthropic endpoint.
        await expect(dialog.getByRole("group", { name: "Model roles" })).toBeVisible();
        await expect(dialog.getByRole("group", { name: "Model details" })).toBeVisible();
        await expect(dialog.getByTestId("no-roles-notice")).toBeVisible();
        await dialog.getByRole("button", { name: "Add model details" }).click();
        await expect(dialog.getByLabel("Model id 1")).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      // SET-4: the Helm provider beside a dashboard provider, the provider dialog and the Email page.
      test("the sign-in page lists providers and opens the provider dialog", async ({ page }) => {
        await openRoute(page, "/admin/settings/sign-in", scheme);
        await expect(page).toHaveURL(/\/admin\/people\?tab=sign-in$/);
        await expect(page.getByRole("heading", { name: "People", level: 1 })).toBeVisible();
        await expect(page.getByText("Set by the installation").first()).toBeVisible();
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
        await expect(page).toHaveURL(/\/admin\/internet-access\?tab=defaults$/);
        await expect(page.getByRole("heading", { name: "What agents may do", level: 1 })).toBeVisible();
        await expect(page.getByTestId("hosted.idle_stop_secs-locked-value")).toHaveText("30 minutes");
        await expect(page.getByRole("region", { name: "Runners and sandbox sizes" })).toContainText("4096");
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      // FIX-172 (D-374): the programs are ticked by name, limited to what runners offer; the default is picked from them.
      test("the agents page ticks harnesses by name and saves their ids", async ({ page }) => {
        let put: Record<string, unknown> | null = null;
        await openRoute(page, "/admin/internet-access?tab=defaults", scheme);
        // Registered after the mock, so these answer first (the last route wins).
        await page.route("**/v1/admin/settings/agents/preview", (route) =>
          route.fulfill({
            json: { generation: 2, diff: [], loosening: [], affected_orgs: [], affected_orgs_total: 0, warnings: [] },
          }),
        );
        await page.route("**/v1/admin/settings/agents", async (route) => {
          if (route.request().method() !== "PUT") return route.fallback();
          put = route.request().postDataJSON() as Record<string, unknown>;
          return route.fallback();
        });
        const group = page.getByRole("group", { name: "Programs agents may run" });
        await expect(group.getByLabel("Claude Code")).toBeChecked();
        // Only what a runner offers can be ticked; the names are words, never ids.
        await expect(group.getByLabel("Pi")).toBeDisabled();
        await expect(group.getByLabel("OpenCode")).toBeDisabled();
        await group.getByLabel("goose").check();
        const pick = page.getByLabel("Program new agents use");
        await expect(pick.locator("option")).toHaveText(["Claude Code", "goose"]);
        await pick.selectOption("goose");
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
        await page.getByRole("button", { name: "Save", exact: true }).click();
        await expect.poll(() => put).not.toBeNull();
        expect(put).toMatchObject({ values: { "hosted.enabled_harnesses": ["claude-code", "goose"], "hosted.default_harness": "goose" } });
      });

      test("the terminal page shows one Unavailable card for a missing deployment wiring, and no form", async ({ page }) => {
        await openRoute(page, "/admin/settings/terminal", scheme);
        await expect(page).toHaveURL(/\/admin\/platform\?tab=computers$/);
        await expect(page.getByRole("heading", { name: "Health & runners", level: 1 })).toBeVisible();
        await expect(page.getByText(/does not provide the terminal/)).toBeVisible();
        await expect(page.getByRole("region", { name: "The web terminal isn't available on this Aurora" })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("the limits page lists a Helm rate class and warns about private destinations", async ({ page }) => {
        await openRoute(page, "/admin/settings/limits", scheme);
        await expect(page).toHaveURL(/\/admin\/platform\?tab=limits$/);
        await expect(page.getByRole("heading", { name: "Health & runners", level: 1 })).toBeVisible();
        await expect(page.getByText("Set by the installation").first()).toBeVisible();
        await expect(page.getByText(/Entries in the allowed-private list open an address range/)).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("the email page offers a test e-mail", async ({ page }) => {
        await openRoute(page, "/admin/settings/email", scheme);
        await expect(page).toHaveURL(/\/admin\/platform\?tab=email$/);
        await expect(page.getByRole("heading", { name: "Health & runners", level: 1 })).toBeVisible();
        await expect(page.getByRole("button", { name: "Send test e-mail" })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });
    });
  }
}
