import { expect, test } from "@playwright/test";
import { mkdirSync } from "node:fs";

import { openJourney } from "./support/journey";
import { overflow } from "./support/open";
import { serious } from "./support/axe";

// FIX-133 (D-240) as the connect journey carries it (P5-UX-3, docs/14 §Agents, accounts and access §2–3): the Access step
// offers Read only (recommended, preselected) / Ask before changes / Change without asking; the choice is sent as the
// agent's `grant.level` with the permissions of that level, and Advanced lists every method with what it does. Both themes
// at 1280 px and 390 px, no horizontal scroll, axe with no rule disabled. Screenshots stay outside the repo.

const SHOTS = "/tmp/aurora-screens/fix-133";
mkdirSync(SHOTS, { recursive: true });

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("GitHub connect: Read only is recommended, every method is under Advanced, the choice is sent as the agent's access", async ({ page }) => {
        const saved = await openJourney(page, "/o/acme/w/main/agents/daneel?tab=access&connect=github", scheme);
        const dialog = page.getByTestId("ws-connect-journey");
        await expect(page.getByRole("dialog", { name: "GitHub for Daneel" })).toBeVisible({ timeout: 30_000 });
        const access = dialog.getByRole("radiogroup", { name: "What may Daneel do in GitHub?" });
        await expect(access.getByRole("radio", { name: /Read only/ })).toBeChecked();
        await expect(access.getByText("Recommended")).toBeVisible();
        await expect(access.getByText("Ask before changes", { exact: true })).toBeVisible();
        await access.getByText("Change without asking", { exact: true }).click();
        await dialog.getByRole("button", { name: "Next" }).click();

        // Two ways work for people here: the choice sits at the top of the step (D-388), each with what it does; the
        // GitHub App needs an administrator and is not offered.
        const methods = dialog.getByRole("radiogroup", { name: "Sign in with:" });
        for (const method of ["Personal access token", "OAuth"]) await expect(methods.getByText(method, { exact: true })).toBeVisible();
        await expect(methods.getByText("Suggested")).toBeVisible();

        await dialog.getByLabel("Personal access token", { exact: true }).fill("not-a-real-token");
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
        await page.screenshot({ path: `${SHOTS}/connect-access-${scheme}-${width}.png` });
        expect(await serious(page)).toEqual([]);

        await dialog.getByRole("button", { name: "Connect", exact: true }).click();
        await expect(dialog.getByText("Daneel can now read and change acme without asking.")).toBeVisible();
        expect(saved).toHaveLength(1);
        expect(saved[0]!.body).toMatchObject({ method_id: "token", scopes: ["repo", "read:org"], grant: { level: "act" } });
        expect(saved[0]!.body).not.toHaveProperty("access");
      });
    });
  }
}
