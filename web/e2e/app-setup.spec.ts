import { expect, test } from "@playwright/test";

import { extraClients } from "./support/mock-api";
import { openRoute, overflow } from "./support/open";

// P5-UX-9 (C-932, C-933, C-946; docs/14 §Agents, accounts and access §4): one setup page per app for a workspace and for
// an organization, the workspace's OAuth clients managed there, the app's values to copy, and which apps the organization
// allows on its Apps tab. The API is mocked (e2e/support/mock-api.ts).

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("a workspace owner registers Linear's client on its page, the status turns Ready and Connect opens the journey", async ({ page }) => {
        await openRoute(page, "/o/acme/w/main/apps/linear", scheme);
        await expect(page.getByTestId("ws-app-setup")).toBeVisible();
        const ways = page.getByTestId("setup-ways");
        await expect(ways.getByText("Needs setup")).toBeVisible();
        await expect(ways.getByText(/Sign in with Linear needs a sign-in app: none is set up for this workspace/)).toBeVisible();
        // not an organization admin: who can do it for everyone is named by role (D-407)
        await expect(ways.getByText(/Ask an organization admin/)).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);

        await ways.getByRole("button", { name: "Register a sign-in app for this workspace" }).click();
        const dialog = page.getByRole("dialog", { name: "Register a sign-in app" });
        await expect(dialog.getByLabel("App")).toHaveValue("linear|linear");
        await expect(dialog.getByRole("option", { name: "Linear — for Sign in with Linear" })).toHaveCount(1);
        await expect(dialog.getByText("https://aurora.example.org/v1/oauth/callback")).toBeVisible();
        await dialog.getByLabel("Client ID").fill("lin_abc");
        await dialog.getByLabel("Client secret", { exact: true }).fill("lin_secret");
        // (D-375) Show reveals what was typed; the button stays beside the input and nothing scrolls sideways.
        await dialog.getByRole("button", { name: "Show Client secret" }).click();
        await expect(dialog.getByLabel("Client secret", { exact: true })).toHaveValue("lin_secret");
        await expect(dialog.getByLabel("Client secret", { exact: true })).toHaveAttribute("type", "text");
        const box = await dialog.getByLabel("Client secret", { exact: true }).boundingBox();
        const btn = await dialog.getByRole("button", { name: "Hide Client secret" }).boundingBox();
        expect(box && btn && btn.x >= box.x + box.width - 1 && Math.abs(btn.y - box.y) < box.height).toBe(true);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        await dialog.getByRole("button", { name: "Hide Client secret" }).click();
        await expect(dialog.getByLabel("Client secret", { exact: true })).toHaveAttribute("type", "password");
        await dialog.getByRole("button", { name: "Add" }).click();
        await expect(dialog).toBeHidden();

        expect(extraClients).toHaveLength(1);
        expect(extraClients[0]).toMatchObject({ package_id: "linear", client_key: "linear" });
        await expect(ways.getByText("Needs setup")).toHaveCount(0);
        await expect(ways.getByText("Ready")).toHaveCount(2);
        await expect(page.getByTestId("setup-headline")).toContainText("People here can connect Linear with an API key or Sign in with Linear today.");
        // the client is listed on the page, named by the way it is used
        await expect(page.getByTestId("setup-where").getByText("Linear — for Sign in with Linear").first()).toBeVisible();

        await page.getByRole("button", { name: "Connect Linear" }).click();
        await expect(page.getByTestId("ws-connect-journey")).toBeVisible();
      });

      test("the AWS page shows the organization's external id with Copy", async ({ page }) => {
        await openRoute(page, "/o/acme/apps/aws-keys", scheme);
        await expect(page.getByTestId("org-app-setup")).toBeVisible();
        const values = page.getByTestId("setup-values");
        await expect(values.getByText("aurora-ext-7f3a9c")).toBeVisible();
        await expect(values.getByRole("button", { name: "Copy" })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
      });

      test("the Apps tab's Details goes to the setup page; the guide shows the viewer's values", async ({ page }) => {
        await openRoute(page, "/o/acme/w/main/apps?tab=apps", scheme);
        await page.getByRole("link", { name: "Details of Linear" }).click();
        await expect(page).toHaveURL(/\/o\/acme\/w\/main\/apps\/linear$/);
        await expect(page.getByRole("heading", { name: "Setup guide" })).toBeVisible();
        await expect(page.getByTestId("setup-guide")).toContainText("https://aurora.example.org/v1/oauth/callback");
        await expect(page.getByTestId("setup-can-do")).toContainText("Open an issue with a title and a body.");
      });

      test("an organization admin switches an app off on the Apps tab: the choice saves at once, no Save (P5-POLISH-9)", async ({ page }) => {
        await openRoute(page, "/o/acme/apps?tab=apps", scheme);
        const githubSwitch = page.getByRole("switch", { name: "People may connect GitHub" });
        await expect(githubSwitch).toBeChecked();
        const put = page.waitForRequest((r) => r.method() === "PUT" && r.url().endsWith("/app-availability"));
        // Not a pointer click: at 390 px the dev server's own indicator (nextjs-portal) covers the bottom-left of the page.
        await githubSwitch.dispatchEvent("click");
        const body = (await put).postDataJSON() as { posture: string; allow: { app_id: string }[] };
        expect(body.posture).toBe("restricted");
        expect(body.allow.map((e) => e.app_id)).not.toContain("github");
        await expect(page.getByRole("button", { name: "Save" })).toHaveCount(0);
      });

      test("Administration's catalogue entry links to the OAuth clients of the app, with its form open", async ({ page }) => {
        await openRoute(page, "/admin/apps/packages/github", scheme);
        await page.getByRole("link", { name: "Sign-in apps for this app" }).click();
        await expect(page).toHaveURL(/\/admin\/apps\?tab=oauth-clients&package=github/);
        await expect(page.locator("#oc-app")).toHaveValue("github|github");
      });
    });
  }
}
