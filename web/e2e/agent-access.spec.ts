import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { openAccess } from "./support/access";
import { overflow } from "./support/open";

// P5-UX-10 (C-934): "What exactly?" on an agent's Access tab. An agent on the organization's AWS account, limited to
// Read only, shows its changes as Blocked with "Limited by Acme" and its own choice beside it; the table is text for
// someone who may not change the access. Both themes at 1280 px and 390 px, no horizontal scroll, axe with no rule
// disabled (the same screens also run in stage4-axe, stage4-routes and stage2-narrow).

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      // P5-TOOLASK (C-1042, C-1043): an action its app asks about first by default says so, and Allowed is the override.
      test("an action the app asks about first by default says so and can be overridden with Allowed", async ({ page }) => {
        await openAccess(page, "table", scheme);
        const region = page.getByRole("listitem", { name: "acme-bot" }).getByRole("region", { name: /^What Daneel can do in GitHub/ });
        const merge = region.getByRole("listitem").filter({ hasText: "Merge a pull request" });
        await expect(merge.getByText(/Asks first \(app default\)/)).toBeVisible();
        await expect(merge.getByLabel("Merge a pull request: what happens")).toHaveValue("ask");
        // Only that action carries the note.
        await expect(region.getByText(/Asks first \(app default\)/)).toHaveCount(1);
        await merge.getByLabel("Merge a pull request: what happens").selectOption("allow");
        await expect(merge.getByLabel("Merge a pull request: what happens")).toHaveValue("allow");
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        const { violations } = await new AxeBuilder({ page }).analyze();
        expect(violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id)).toEqual([]);
      });

      test("an organization's Read only limit shows the changes as Blocked, Limited by Acme", async ({ page }) => {
        await openAccess(page, "table-limited", scheme);
        const region = page.getByRole("listitem", { name: "AWS prod" }).getByRole("region", { name: /^What Daneel can do in AWS/ });
        const upload = region.getByRole("listitem").filter({ hasText: "Upload an object" });
        await expect(upload.getByText("Blocked")).toBeVisible();
        await expect(upload.getByText("Limited by Acme")).toBeVisible();
        await expect(upload.getByText("Daneel's access: Asks you")).toBeVisible();
        const list = region.getByRole("listitem").filter({ hasText: "List buckets" });
        await expect(list.getByText("Allowed")).toBeVisible();
        await expect(list.getByText(/Limited by/)).toHaveCount(0);
        // Text, no controls, for someone who may not change the access; no rule id anywhere.
        await expect(region.getByRole("combobox")).toHaveCount(0);
        await expect(region.getByText(/rule id|[0-9a-f]{8}-[0-9a-f]{4}-/i)).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        const { violations } = await new AxeBuilder({ page }).analyze();
        expect(violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id)).toEqual([]);
      });
    });
  }
}
