import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";

import { id, mockApi } from "./support/mock-api";
import { overflow } from "./support/open";

// FIX-133 (D-240; docs/14 §Connect window): connect GitHub — every method offers "Read only" (recommended) / "Read
// and write", each method says what GitHub enforces and what Aurora enforces, and the choice is sent as `access`.
// Both themes at 1280 px and 390 px, no horizontal scroll, axe with no rule disabled. Screenshots stay outside the repo.

const SHOTS = "/tmp/aurora-screens/fix-133";
mkdirSync(SHOTS, { recursive: true });

const tool = (tid: string, name: string, group: "read" | "write") => ({
  id: tid, name, group, host: "api.github.com", path: `/${tid}`, methods: group === "read" ? ["GET"] : ["POST"], connect_methods: [], wildcard: false,
});
const github = {
  app_id: "github", name: "GitHub", package_id: "github", package_version: "2.0.0", available: true, category: "development", connection_count: 0,
  description: "Repositories", has_dark_icon: false, app_restrictions: { custom_only: false, block_untooled: false },
  connect: [
    {
      id: "github_app", kind: "hook", access_levels: [], default_scopes: [], scopes: [], recommended: true, label: "GitHub App",
      description: "An app installed on the repositories you choose. GitHub enforces the app's permissions and repositories. With Read only chosen, Aurora also asks GitHub for a read-only token.",
      fields: [
        { name: "app_id", label: "GitHub App ID or client ID", type: "text", store: "metadata", required: true },
        { name: "installation_id", label: "Installation ID", type: "text", store: "metadata", required: true },
        { name: "private_key", label: "Private key (.pem file)", type: "file", store: "credentials", required: true },
        { name: "repositories", label: "Limit to repositories (optional)", type: "text", store: "metadata", required: false },
      ],
    },
    {
      id: "token", kind: "fields", access_levels: [], default_scopes: [], scopes: [], recommended: false, label: "Personal access token",
      description: "Acts as you. Use a fine-grained token: GitHub itself then limits it. A classic token is all or nothing per scope; choose Read only and Aurora blocks writes for you.",
      fields: [{ name: "access_token", label: "Personal access token", type: "secret", store: "credentials", required: true }],
    },
    {
      id: "oauth", kind: "oauth2", client_key: "github", access_levels: [], default_scopes: [], scopes: [], recommended: false, label: "OAuth",
      description: "Sign in with GitHub and act as you. GitHub has no read-only repository scope, so GitHub does not stop writes here: choose Read only and Aurora blocks them.",
      fields: [],
    },
  ],
  credentials: [], hosts: [], metadata: [], oauth_clients: [],
  tools: [tool("get_repo", "Get a repository", "read"), tool("create_issue", "Create an issue", "write")],
};

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("GitHub connect: Read only on every method, per-method copy, the choice is sent as access", async ({ page }) => {
        const posted: Record<string, unknown>[] = [];
        await mockApi(page);
        await page.emulateMedia({ colorScheme: scheme });
        await page.route("**/v1/**", async (route) => {
          const p = new URL(route.request().url()).pathname;
          const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
          if (p === "/v1/apps/github" && route.request().method() === "GET") return json(github);
          if (p === "/v1/apps/github/connect") {
            posted.push(route.request().postDataJSON());
            return json({ kind: "connected", connection: { id: id(91) }, missing_scopes: [] });
          }
          return route.fallback();
        });
        await page.goto("/o/acme/w/main/apps/github/connect");

        const methods = page.getByRole("radiogroup", { name: "How do you want to connect?" });
        await expect(methods.getByText("GitHub enforces the app's permissions and repositories.")).toBeVisible();
        const access = page.getByRole("radiogroup", { name: "What may agents do with this connection?" });
        // Offered on every method; Read only is the preselected, recommended choice.
        for (const method of ["GitHub App", "Personal access token", "OAuth"]) {
          await methods.getByText(method, { exact: true }).click();
          await expect(access.getByRole("radio", { name: /Read only/ })).toBeChecked();
          await expect(access.getByText("Recommended")).toBeVisible();
          await expect(access.getByText("Read and write")).toBeVisible();
        }
        await expect(methods.getByText(/GitHub does not stop writes here/)).toBeVisible();

        await methods.getByText("Personal access token", { exact: true }).click();
        await page.getByLabel("Personal access token", { exact: true }).fill("not-a-real-token");
        await access.getByText("Read and write", { exact: true }).click();
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
        await page.screenshot({ path: `${SHOTS}/connect-access-${scheme}-${width}.png` });
        expect(await serious(page)).toEqual([]);

        await access.getByText("Read only", { exact: true }).click();
        await page.getByRole("button", { name: "Connect", exact: true }).click();
        await expect.poll(() => posted.length).toBe(1);
        expect(posted[0]).toMatchObject({ method_id: "token", access: "read" });
      });
    });
  }
}
