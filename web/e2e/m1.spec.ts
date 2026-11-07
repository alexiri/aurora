import { readFileSync } from "node:fs";

import { expect, test, type Page } from "@playwright/test";

const PASSWORD = "correct-horse-battery-staple";

function invitationLink(logPath: string, email: string): string | null {
  const lines = readFileSync(logPath, "utf8").split("\n").reverse();
  for (const line of lines) {
    if (!line.includes("/join?token=")) continue;
    // Log lines are JSON; the link may be JSON-escaped (\/ is unlikely, but handle it).
    const m = line.replace(/\\\//g, "/").match(/https?:\/\/[^\s"\\]+\/join\?token=[A-Za-z0-9_\-.~%]+/);
    if (m && (line.includes(email) || !line.includes("@"))) return m[0];
  }
  return null;
}

// Dev-mode hydration can lag behind first paint; retry the click until the dialog opens.
async function openDialog(page: Page, button: string) {
  await expect(async () => {
    await page.getByRole("button", { name: button }).click();
    await expect(page.getByRole("dialog")).toBeVisible({ timeout: 1500 });
  }).toPass({ timeout: 20_000 });
}

async function signUp(page: Page, name: string, email: string | null) {
  await page.getByLabel("Name", { exact: true }).fill(name);
  if (email) await page.getByLabel("Email", { exact: true }).fill(email);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
}

test("sign-up -> workspace -> invite -> accept", async ({ page, browser }) => {
  const apiLog = process.env.AURORA_E2E_API_LOG;
  expect(apiLog, "AURORA_E2E_API_LOG must be set (run via scripts/e2e/m1.sh)").toBeTruthy();
  const run = Date.now().toString(36);
  const emailA = `a-${run}@example.test`;
  const emailB = `b-${run}@example.test`;

  // User A signs up (open mode) and lands in the default workspace.
  await page.goto("/auth/signup");
  await signUp(page, "Alice", emailA);
  await expect(page.getByRole("navigation", { name: "Workspace" })).toBeVisible({ timeout: 30_000 });
  await expect(page).toHaveURL(/\/o\/[^/]+\/w\/[^/]+$/);

  // Create workspace "Research" from the org page.
  const orgUrl = page.url().replace(/\/w\/[^/]+$/, "");
  await page.goto(orgUrl);
  await openDialog(page, "New workspace");
  const wsDialog = page.getByRole("dialog");
  await wsDialog.getByLabel("Name", { exact: true }).fill("Research");
  await wsDialog.getByRole("button", { name: "Create workspace" }).click();

  // Overview shows Research with Daneel.
  await expect(page.getByRole("heading", { name: "Research" })).toBeVisible();
  await expect(page.getByRole("list", { name: "Agents" }).getByText("Daneel", { exact: true })).toBeVisible();

  // Org Members -> invite B with the Research binding.
  // (Workspace pages have no link to org Members; go via the org URL.)
  await page.goto(`${orgUrl}/members`);
  await expect(page).toHaveURL(/\/members$/);
  await openDialog(page, "Invite member");
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Email", { exact: true }).fill(emailB);
  await dialog.getByLabel("Role").selectOption("member");
  await dialog.getByLabel("Research").selectOption("member"); // P2-012: per-workspace access select (was a checkbox)
  await dialog.getByRole("button", { name: "Send invitation" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByText(emailB).first()).toBeVisible();

  // Read the link from the API's log-only mailer.
  let link: string | null = null;
  await expect
    .poll(
      () => {
        link = invitationLink(apiLog!, emailB);
        return link;
      },
      { timeout: 15_000 },
    )
    .not.toBeNull();
  const token = new URL(link!).searchParams.get("token")!;

  // User B, fresh context.
  const ctx = await browser.newContext();
  try {
    const b = await ctx.newPage();
    await b.goto(`/join?token=${encodeURIComponent(token)}`);
    await expect(b.getByText(emailB).first()).toBeVisible();
    await b.getByRole("link", { name: "Create account" }).click();
    await expect(b.getByLabel("Email", { exact: true })).toHaveValue(emailB);
    await signUp(b, "Bob", null) /* email is read-only, prefilled from the invitation */;
    await expect(b).toHaveURL(/\/join\?token=/);
    await b.getByRole("button", { name: "Accept invitation" }).click();

    // Lands in the org and sees Research with Daneel.
    await expect(b).toHaveURL(/\/o\/[^/]+$/, { timeout: 30_000 });
    await b.getByRole("link", { name: /Research/ }).click();
    await expect(b.getByRole("heading", { name: "Research" })).toBeVisible();
    await expect(b.getByRole("list", { name: "Agents" }).getByText("Daneel", { exact: true })).toBeVisible();
  } finally {
    await ctx.close();
  }
});
