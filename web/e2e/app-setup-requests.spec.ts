import { expect, test, type Page } from "@playwright/test";

import { id, mockApi, now, org, ws } from "./support/mock-api";
import { overflow } from "./support/open";
import { serious } from "./support/axe";

// P5-POLISH-4 (D-386, C-1030, SEC-135; docs/14 §Agents, accounts and access §4): an owner's Overview shows a person's
// request for an app in Needs you, with Set up (the organization's Apps tab at the app) and Not now. Both themes at
// 1280 px and 390 px, no horizontal scroll, axe (serious and critical) clean.

const request = {
  id: id(700),
  org_id: org.id,
  app_id: "google-calendar",
  app_name: "Google Calendar",
  status: "open",
  askers: [{ user_id: id(701), display_name: "Bob Builder", workspace_id: ws.id, asked_at: now }],
  needs: ["oauth_client"],
  unavailable: false,
  created_at: now,
  closed_at: null,
};

async function mockRequests(page: Page) {
  const state = { declined: 0, open: true };
  await mockApi(page);
  await page.route("**/v1/**", async (route) => {
    const p = new URL(route.request().url()).pathname;
    const m = route.request().method();
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (p === `/v1/orgs/${org.id}/app-setup-requests` && m === "GET") return json({ items: state.open ? [request] : [] });
    if (p === `/v1/orgs/${org.id}/app-setup-requests/${request.id}/decline` && m === "POST") {
      state.declined += 1;
      state.open = false;
      return json({ ...request, status: "declined" });
    }
    return route.fallback();
  });
  return state;
}

async function open(page: Page, scheme: "light" | "dark") {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto("/o/acme/w/main");
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, "the Overview threw").toEqual([]);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("the owner sees the request, Set up opens the app's place in Apps, and the page stays tidy", async ({ page }) => {
        await mockRequests(page);
        await open(page, scheme);
        await expect(page.getByText("Bob Builder wants Google Calendar")).toBeVisible();
        await expect(page.getByText("A sign-in app for Google Calendar needs to be registered once.")).toBeVisible();
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
        await expect(page.getByRole("link", { name: "Set up" })).toHaveAttribute("href", "/o/acme/apps?tab=oauth-clients&app=google-calendar");
      });

      test("Not now confirms, then declines and the row goes", async ({ page }) => {
        const state = await mockRequests(page);
        await open(page, scheme);
        await page.getByRole("button", { name: "Not now" }).click();
        await expect(page.getByText("Bob Builder will be told you aren't setting up Google Calendar now.")).toBeVisible();
        await page.getByRole("dialog").getByRole("button", { name: "Not now" }).click();
        await expect.poll(() => state.declined).toBe(1);
        await expect(page.getByText("Bob Builder wants Google Calendar")).toBeHidden();
      });
    });
  }
}
