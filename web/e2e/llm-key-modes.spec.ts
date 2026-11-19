import { expect, test, type Page } from "@playwright/test";

import { id, mockApi, org } from "./support/mock-api";
import { overflow } from "./support/open";
import { serious } from "./support/axe";

// P4-LLM-6 (C-811, docs/14 §Credentials › LLM keys, §Settings › LLM endpoints): the key form per auth mode
// (Bedrock assume-role with the organization's external id and the trust policy, Azure Entra, GCP key file) and the
// endpoint rows with harness badges and the private-address mark. Both themes at 1280 px and 390 px, no horizontal
// scroll, axe with nothing disabled.

const providers = {
  providers: [
    {
      id: "bedrock", display_name: "Amazon Bedrock", dialects: [], auth_modes: ["aws_sigv4", "aws_assume_role", "api_key"],
      auth_mode_details: [
        { id: "aws_sigv4", value_shape: "aws_sigv4", recommended: false },
        { id: "aws_assume_role", value_shape: "aws_assume_role", recommended: true },
        { id: "api_key", value_shape: "string", recommended: false },
      ],
    },
    {
      id: "azure_openai", display_name: "Azure OpenAI", dialects: [], auth_modes: ["api_key", "azure_entra"],
      auth_mode_details: [
        { id: "api_key", value_shape: "string", recommended: false },
        { id: "azure_entra", value_shape: "azure_entra", recommended: true },
      ],
    },
    {
      id: "vertex", display_name: "Google Vertex AI", dialects: [], auth_modes: ["gcp_service_account"],
      auth_mode_details: [{ id: "gcp_service_account", value_shape: "gcp_service_account", recommended: true }],
    },
  ],
  endpoints: [
    {
      id: "bed", provider: "bedrock", display_name: "Bedrock EU", base_url: "https://bedrock-runtime.eu-central-1.amazonaws.com", dialects: ["anthropic_bedrock"],
      permitted: true, key_scopes: ["organization", "workspace"], auth_modes: ["aws_sigv4", "aws_assume_role", "api_key"], recommended_auth_mode: "aws_assume_role", harnesses: ["claude_code"],
    },
    {
      id: "az", provider: "azure_openai", display_name: "Azure", base_url: "https://res.openai.azure.com", dialects: ["openai_chat"],
      permitted: true, key_scopes: ["organization", "workspace"], auth_modes: ["api_key", "azure_entra"], recommended_auth_mode: "azure_entra", harnesses: [],
    },
    {
      id: "vx", provider: "vertex", display_name: "Vertex", base_url: "https://aiplatform.googleapis.com", dialects: ["anthropic_vertex"],
      permitted: true, key_scopes: ["organization", "workspace"], auth_modes: ["gcp_service_account"], recommended_auth_mode: "gcp_service_account", harnesses: ["claude_code"],
    },
  ],
};

async function open(page: Page, path: string, scheme: "light" | "dark") {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  await page.emulateMedia({ colorScheme: scheme });
  await page.route("**/v1/**", async (route) => {
    const p = new URL(route.request().url()).pathname;
    if (p === "/v1/llm/providers") {
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(providers) });
    }
    if (p === `/v1/orgs/${id(1)}`) {
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ ...org, external_id: "ext-7f3a9c1d" }) });
    }
    return route.fallback();
  });
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, `${path} threw`).toEqual([]);
}

/** (P5-UX-4) The key form opens from the Accounts list's Add › Add an AI key. */
async function openKeyForm(page: Page) {
  await page.getByRole("button", { name: "Add", exact: true }).click();
  await page.getByRole("menuitem", { name: "Add an AI key" }).click();
}

async function clean(page: Page) {
  expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
  expect(await serious(page)).toEqual([]);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("Bedrock: assume a role first, with the external id and the trust policy", async ({ page }) => {
        await open(page, "/o/acme/w/main/apps?tab=accounts&kind=ai-key", scheme);
        await openKeyForm(page);
        await page.getByLabel("Name").fill("Bedrock key");
        await page.getByLabel("Provider endpoint").selectOption("bed");
        const group = page.getByRole("radiogroup", { name: "How Aurora signs in to the provider" });
        await expect(group.getByRole("radio").first()).toBeChecked();
        await expect(group.getByText("Recommended")).toBeVisible();
        await expect(page.getByLabel("Role ARN")).toBeVisible();
        await expect(page.getByText("ext-7f3a9c1d").first()).toBeVisible();
        await expect(page.getByRole("button", { name: "Copy the trust policy" })).toBeVisible();
        await clean(page);
        // The radios are visually hidden inside their cards: the card is what a person clicks.
        await group.getByText("Access keys", { exact: true }).click();
        await expect(group.getByRole("radio", { name: /Access keys/ })).toBeChecked();
        await expect(page.getByLabel("Role ARN")).toHaveCount(0);
        await expect(page.getByLabel("Access key id")).toBeVisible();
        await clean(page);
      });

      test("Azure: Entra fields and the no-harness notice; Vertex: the key file", async ({ page }) => {
        await open(page, "/o/acme/w/main/apps?tab=accounts&kind=ai-key", scheme);
        await openKeyForm(page);
        await page.getByLabel("Provider endpoint").selectOption("az");
        await expect(page.getByText("No enabled harness speaks this endpoint's API", { exact: false })).toBeVisible();
        await expect(page.getByLabel("Tenant id")).toBeVisible();
        await expect(page.getByLabel("Client secret", { exact: true })).toBeVisible();
        await clean(page);
        await page.getByLabel("Provider endpoint").selectOption("vx");
        await expect(page.getByLabel("Service account key (JSON)")).toBeVisible();
        await expect(page.getByText("Models are configured by your administrator.")).toBeVisible();
        await clean(page);
      });
    });
  }
}
