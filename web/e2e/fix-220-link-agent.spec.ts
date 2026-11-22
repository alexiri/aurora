import { expect, test, type Page, type Route } from "@playwright/test";

import { mockApi } from "./support/mock-api";
import { openThemed, overflow } from "./support/open";
import { serious } from "./support/files";

// FIX-220 (D-461, C-1147): the sign-in link page asks which agent answers the person's direct messages to the shared bot,
// preselects the only one, and sends it with the confirm; the success text names it. Zero serious/critical axe violations
// in both themes at 1280 and 390 px, no horizontal scroll at 390 px.

const T = "2026-01-01T00:00:00Z";
const INTEG = "00000000-0000-4000-8000-0000000000b1";
const AGENT = "00000000-0000-4000-8000-0000000000c1";
const PREVIEW = {
  integration_id: INTEG, connector_id: "mattermost", tenant_label: "Acme Chat", platform_user_ref: "U123",
  platform_display_name: "Ada L", platform_email_hint: null, expires_at: "2099-01-01T00:00:00Z",
};
const OPTIONS = {
  org_id: "00000000-0000-4000-8000-0000000000aa",
  shared_bot: { handle: "daneel", display_name: "Daneel" },
  items: [{ agent_id: AGENT, name: "Rex", handle: "rex", workspace_id: "00000000-0000-4000-8000-0000000000f1", workspace_name: "Main", attached: false }],
};
const LINK = {
  id: "00000000-0000-4000-8000-0000000000d1", integration_id: INTEG, connector_id: "mattermost", tenant_label: "Acme Chat",
  platform_user_ref: "U123", user: { id: "u1", display_name: "Ada" }, method: "sign_in_link", created_at: T,
};

const reply = (route: Route, body: unknown, status = 200) =>
  route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

async function mockLink(page: Page, confirmed: { body: unknown }) {
  await mockApi(page);
  await page.route("**/v1/channel-links/preview**", (route) => reply(route, PREVIEW));
  await page.route("**/v1/me/dm-default-options**", (route) => reply(route, OPTIONS));
  await page.route("**/v1/channel-links/confirm", (route) => {
    confirmed.body = route.request().postDataJSON();
    return reply(route, LINK);
  });
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("the link page preselects the only agent and sends it with the confirm", async ({ page }) => {
        const confirmed: { body: unknown } = { body: null };
        await mockLink(page, confirmed);
        await openThemed(page, "/link/channel?token=aui_fix220", scheme);
        const select = page.getByLabel("Agent that answers your direct messages to @daneel");
        await expect(select).toBeVisible();
        await expect(select).toHaveValue(AGENT);
        await expect(select.locator("option")).toHaveText(["Decide later", "Rex (@rex) · Main · will be added to @daneel"]);
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
        const button = page.getByRole("button", { name: "Confirm link" });
        await expect(button).toBeEnabled();
        await button.click();
        await expect(page.getByText("Linked. Message @daneel again and Rex answers.")).toBeVisible();
        expect(confirmed.body).toEqual({ token: "aui_fix220", platform_user_ref: "U123", agent_id: AGENT });
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
      });
    });
  }
}
