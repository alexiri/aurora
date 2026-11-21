import { expect, test, type Page } from "@playwright/test";

import { id, mockApi, org, ws } from "./support/mock-api";
import { overflow } from "./support/open";
import { serious } from "./support/axe";

// P5-UX-5 (C-919, C-920; docs/14 §Agents, accounts and access §10): a workspace owner's People page has Add people (an
// e-mail and a role; one sentence; the link to send; who to ask for an invitation), the list "Waiting for first sign-in"
// with Remove, and Welcome's Add people link opens the dialog. Both themes at 1280 px and 390 px, no horizontal scroll,
// axe with no rule disabled. The real-stack journey is add-people-journey.spec.ts.

async function check(page: Page) {
  expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
  expect(await serious(page)).toEqual([]);
}

const inDays = (n: number) => new Date(Date.now() + n * 86_400_000 - 3_600_000).toISOString();

/** The People page with an in-memory waiting list that the add and Remove routes change. */
async function mockPeople(page: Page, scheme: "light" | "dark", entry: "open_signup" | "invitation_needed") {
  await mockApi(page);
  const waiting: { id: string; email: string; role: string; expires_at: string; created_at: string }[] = [];
  const posted: unknown[] = [];
  await page.route("**/v1/**", async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: b === null ? "" : JSON.stringify(b) });
    if (p === `/v1/orgs/${org.id}`) return json({ ...org, contacts: { owners: [{ name: "Grace Hopper" }], admins: [{ name: "Ada Lovelace" }, { name: "Alan Turing" }] } });
    if (p === `/v1/workspaces/${ws.id}/access/pending` && req.method() === "GET") return json({ items: waiting, next_cursor: null });
    if (p === `/v1/workspaces/${ws.id}/access/by-email` && req.method() === "POST") {
      const body = req.postDataJSON() as { email: string; role: string };
      posted.push(body);
      waiting.push({ id: id(700 + waiting.length), email: body.email.toLowerCase(), role: body.role, expires_at: inDays(30), created_at: inDays(0) });
      return json({ result: "pending", org_entry: entry });
    }
    const del = p.match(new RegExp(`^/v1/workspaces/${ws.id}/access/pending/([^/]+)$`));
    if (del && req.method() === "DELETE") {
      const i = waiting.findIndex((w) => w.id === del[1]);
      if (i >= 0) waiting.splice(i, 1);
      return json(null, 204);
    }
    return route.fallback();
  });
  await page.emulateMedia({ colorScheme: scheme });
  return { waiting, posted };
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("Add people: a person who must be invited too gets the sentence, the link and who to ask; the waiting list shows them and Remove removes them", async ({ page }) => {
        const { posted } = await mockPeople(page, scheme, "invitation_needed");
        await page.goto("/o/acme/w/main/people");
        await page.getByRole("heading", { level: 1, name: "People" }).waitFor();
        await expect(page.getByRole("list", { name: "Waiting for first sign-in" })).toHaveCount(0);
        await page.getByRole("button", { name: "Add people" }).click();
        const dialog = page.getByRole("dialog", { name: "Add people" });
        await expect(dialog.getByRole("radio", { name: "Member" })).toBeChecked();
        // Approver is always offered (batch 6, workspace-4).
        await expect(dialog.getByRole("radio", { name: "Approver" })).toHaveCount(1);
        await check(page);
        await dialog.getByLabel("Who").fill("Carol@Example.org");
        await dialog.getByRole("button", { name: "Add", exact: true }).click();
        // The mock records the POST when it arrives, a moment after the click (CI read [] once): poll.
        await expect.poll(() => posted).toEqual([{ email: "Carol@Example.org", role: "member" }]);
        await expect(dialog.getByText("Carol@Example.org will have access as Member after they first sign in.")).toBeVisible();
        await expect(dialog.getByText("Aurora does not e-mail them. Send them this link:")).toBeVisible();
        await expect(dialog.getByText(/\/o\/acme\/w\/main$/)).toBeVisible();
        await expect(dialog.getByText(/They also need to be invited to Acme Labs\. Ask an organization admin\./)).toBeVisible();
        await check(page);
        await dialog.getByRole("button", { name: "Done" }).click();
        const list = page.getByRole("list", { name: "Waiting for first sign-in" });
        await expect(list).toContainText("carol@example.org · Member · expires in 30 days");
        await check(page);
        await list.getByRole("button", { name: "Remove carol@example.org" }).click();
        await expect(page.getByRole("list", { name: "Waiting for first sign-in" })).toHaveCount(0);
      });

      test("open sign-up: no invitation sentence", async ({ page }) => {
        await mockPeople(page, scheme, "open_signup");
        await page.goto("/o/acme/w/main/people");
        await page.getByRole("button", { name: "Add people" }).click();
        const dialog = page.getByRole("dialog", { name: "Add people" });
        await dialog.getByLabel("Who").fill("dan@example.org");
        await dialog.getByRole("button", { name: "Add", exact: true }).click();
        await expect(dialog.getByText("dan@example.org will have access as Member after they first sign in.")).toBeVisible();
        await expect(dialog.getByText(/They also need to be invited/)).toHaveCount(0);
      });

      test("Welcome's Add people link opens the dialog", async ({ page }) => {
        await mockPeople(page, scheme, "open_signup");
        await page.goto("/o/acme/w/main/welcome");
        await page.getByRole("link", { name: "Add people" }).click();
        // The App Router changes the URL once the People route has loaded; the first visit in `next dev` compiles it (the
        // other specs that warm it may not have run yet), which can take longer than the default 5 s.
        await expect(page).toHaveURL(/\/o\/acme\/w\/main\/people\?add=1$/, { timeout: 30_000 });
        await expect(page.getByRole("dialog", { name: "Add people" })).toBeVisible();
        await check(page);
      });
    });
  }
}
