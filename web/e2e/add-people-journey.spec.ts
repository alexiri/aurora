import { expect, test, type Page } from "@playwright/test";

// P5-UX-5 (D-325, D-328, SEC-128, C-919, C-920) against a real stack (the dev IdP): Bob, an ordinary member who owns
// his personal workspace, adds a second dev-IdP user by e-mail BEFORE that user has ever signed in; after the first
// sign-in the second user sees Bob's workspace. Nothing here makes anyone a member of the organization: Carol gets into
// it the way every new person does (the instance's placement).
//
// Needs the kind dev stack (`scripts/e2e/m1.sh --kind`, AURORA_E2E_BASE_URL); not part of the mocked project.

test.describe.configure({ mode: "serial" });

async function signInAs(page: Page, email: string) {
  await page.goto("/auth/login");
  await page.goto(`/v1/auth/oidc/dev/start?login_hint=${encodeURIComponent(email)}&return_to=%2F`);
  await page.waitForURL((u) => !u.pathname.startsWith("/v1/") && !u.pathname.startsWith("/dev-idp"), { timeout: 30_000 });
}

test("a member who owns a personal workspace adds a person who has never signed in; at their first sign-in they see the workspace", async ({
  browser,
  page,
  baseURL,
}) => {
  test.setTimeout(5 * 60_000);
  const stamp = Date.now().toString(36);
  const bob = `bob-${stamp}@example.test`;
  const carol = `carol-${stamp}@example.test`;

  // Bob's first sign-in lands on his personal workspace's Welcome page.
  await signInAs(page, bob);
  await expect(page).toHaveURL(/\/o\/[^/]+\/w\/[^/]+\/welcome$/, { timeout: 30_000 });
  const base = new URL(page.url()).pathname.replace(/\/welcome$/, "");

  // Welcome's **Add people** opens the dialog on the People page.
  await page.getByRole("link", { name: "Add people" }).click();
  await expect(page).toHaveURL(new RegExp(`${base}/people\\?add=1$`));
  const dialog = page.getByRole("dialog", { name: "Add people" });
  await expect(dialog).toBeVisible();
  await dialog.getByLabel("Who").fill(carol);
  await dialog.getByRole("button", { name: "Add", exact: true }).click();
  await expect(dialog.getByText(`${carol} will have access as Member after they first sign in.`)).toBeVisible();
  await expect(dialog.getByText("Aurora does not e-mail them. Send them this link:")).toBeVisible();
  await dialog.getByRole("button", { name: "Done" }).click();
  await expect(page.getByRole("list", { name: "Waiting for first sign-in" })).toContainText(carol);

  // Carol, who has never signed in, signs in for the first time and sees Bob's workspace.
  const ctx = await browser.newContext({ baseURL });
  try {
    const other = await ctx.newPage();
    await signInAs(other, carol);
    await other.goto(base);
    await expect(other.getByRole("heading", { level: 1 })).toBeVisible();
    await expect(other.getByText("Only workspace owners")).toHaveCount(0);
    await expect(other.getByRole("navigation", { name: "Workspace" })).toBeVisible();
  } finally {
    await ctx.close();
  }

  // The row was used: Bob's waiting list is empty again, and Carol is listed with her role.
  await page.reload();
  await expect(page.getByRole("list", { name: "Waiting for first sign-in" })).toHaveCount(0);
  await expect(page.getByRole("table", { name: "People with access to this workspace" })).toContainText(carol);
});
