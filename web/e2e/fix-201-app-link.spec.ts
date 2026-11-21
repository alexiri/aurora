import { expect as baseExpect, test } from "@playwright/test";

import { id, mockApi, now, org } from "./support/mock-api";

// FIX-201 (D-438, C-1120): in Administration an app's name links by its package id (`/admin/apps/packages/atlassian`);
// the package page resolves it to a version's UUID and replaces the address. The mock answers 400 `invalid_request` to a
// non-UUID `{id}`, as the server does. An organization's Self-hosted apps tab links to the app's setup page, unchanged.
const expect = baseExpect.configure({ timeout: 30_000 });

const JIRA = {
  app_id: "jira-dc", name: "Jira · Data Center", package_id: "atlassian", package_version: "1.0.0", available: true, category: "developer",
  connection_count: 0, description: "Jira Data Center", has_dark_icon: true, summary: "Jira Data Center", accounts: [], readiness: { state: "ready" },
  app_restrictions: { custom_only: false, block_untooled: false }, serves_library: false,
};
const instance = (extra: Record<string, unknown>) => ({
  id: id(800), app_id: "jira-dc", package_id: "atlassian", host: "its.cern.ch", base_path: "/jira", base_url: "https://its.cern.ch/jira", label: "CERN Jira",
  builtin: false, connections: 0, status: "active", created_at: now, updated_at: now, ...extra,
});

async function open(page: import("@playwright/test").Page) {
  await mockApi(page, {
    "/v1/apps": { items: [JIRA] },
    "/v1/apps/jira-dc": { ...JIRA, hosts: [], methods: [], oauth_clients: [], instances: { label: "Jira server" }, metadata: [], send_only: false, values: {}, clients: { workspace: null, organization: [], instance: { set_up: false } }, can: {} },
    "/v1/admin/app-instances": { items: [instance({ org_id: null })], next_cursor: null },
    [`/v1/orgs/${org.id}/app-instances`]: { items: [instance({ org_id: org.id, org_slug: "acme", org_name: "Acme Labs" })], next_cursor: null },
  });
}

test.describe("an app's name in a Self-hosted apps tab", () => {
  test.use({ viewport: { width: 1280, height: 900 } });

  test("Administration: the name opens the package page, not a form error", async ({ page }) => {
    await open(page);
    await page.goto("/admin/apps?tab=self-hosted");
    const row = page.getByRole("row", { name: /its\.cern\.ch\/jira/ });
    await row.getByRole("link", { name: "Jira · Data Center" }).click();
    // The address is the version's UUID, as when the page is opened from the package list.
    await expect(page).toHaveURL(new RegExp(`/admin/apps/packages/${id(1000)}$`));
    await expect(page.getByRole("heading", { level: 1 })).toContainText("Atlassian");
    await expect(page.getByText(/check the form/i)).toHaveCount(0);
  });

  test("Administration: a package id nobody installed says so", async ({ page }) => {
    await open(page);
    await page.goto("/admin/apps/packages/no-such-package");
    await expect(page.getByText("No app package called no-such-package is installed.")).toBeVisible();
    await expect(page.getByText(/check the form/i)).toHaveCount(0);
  });

  test("an organization's tab still links to the app's setup page", async ({ page }) => {
    await open(page);
    await page.goto("/o/acme/apps?tab=self-hosted");
    await page.getByRole("row", { name: /its\.cern\.ch\/jira/ }).getByRole("link", { name: "Jira · Data Center" }).click();
    await expect(page).toHaveURL(/\/o\/acme\/apps\/jira-dc$/);
  });
});
