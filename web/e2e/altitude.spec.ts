import { expect as baseExpect, test, type Page } from "@playwright/test";

import { serious } from "./support/axe";
import { agent, id, mockApi, org, user, ws } from "./support/mock-api";
import { overflow, titled } from "./support/open";

// The first request of a route compiles it in `next dev`; on a loaded host that outlasts the default 5 s.
const expect = baseExpect.configure({ timeout: 30_000 });

// P5-REFINE-3 (D-406, C-1056): three altitudes and the per-person "Show more controls" switch. A member starts at Use
// (Overview, Agents; an agent without Terminal, Tests, SSH…; one collapsed line for the model settings), the footer counts
// what is hidden, **Show** turns the stored switch on and reveals everything in place; an owner of a team workspace
// starts at Set up. Direct links still open hidden pages.

const person = { ...user, is_instance_admin: false };
const meAs = (orgRole: string, wsRole: string) => ({
  "/v1/me": {
    user: person,
    memberships: [{ org: { ...org, my_role: orgRole }, status: "active", workspaces: [{ ...ws, my_role: wsRole, personal: false }] }],
    default_org_id: id(1),
    client_address: "203.0.113.7",
  },
  [`/v1/orgs/${id(1)}`]: { ...org, my_role: orgRole },
  [`/v1/workspaces/${id(2)}`]: { ...ws, my_role: wsRole },
});

/** The mock with the switch's server state kept across requests: GET answers it, PATCH stores it. */
async function open(page: Page, path: string, scheme: "light" | "dark", orgRole: string, wsRole: string, more: boolean) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => {
    if (m.type() === "error" && !/^Failed to load resource/.test(m.text())) errors.push(m.text());
  });
  await mockApi(page, meAs(orgRole, wsRole));
  let stored = more;
  const patches: unknown[] = [];
  await page.route("**/v1/me/onboarding", async (route) => {
    if (route.request().method() === "PATCH") {
      const body = route.request().postDataJSON() as { more_controls?: boolean };
      patches.push(body);
      if (typeof body.more_controls === "boolean") stored = body.more_controls;
    }
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ welcome_seen: true, guide_dismissed: [], tour_completed_at: null, notices_seen: ["renames-2026-11"], more_controls: stored }),
    });
  });
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await titled(page);
  return { patches, errors };
}

const agentUrl = `/o/acme/w/main/agents/${agent.handle}`;

for (const scheme of ["light", "dark"] as const) {
  for (const [width, label] of [[1256, "desktop"], [390, "phone"]] as const) {
    test.describe(`altitudes (${scheme}, ${label})`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("a member at Use sees Overview and Agents, a collapsed line, and Show reveals the rest and is stored", async ({ page }) => {
        const { patches, errors } = await open(page, `${agentUrl}?tab=instructions`, scheme, "member", "member", false);
        if (width >= 768) {
          const nav = page.getByRole("navigation", { name: "Workspace" });
          await expect.poll(() => nav.getByRole("link").allTextContents()).toEqual(["Overview", "Agents"]);
        }
        await expect(page.getByLabel("Name")).toBeVisible();
        await expect(page.getByLabel("Effort")).toHaveCount(0);
        await expect(page.getByText("Model roles, effort and agent program")).toBeVisible();
        const footer = page.locator("[data-more-controls-line]");
        await expect(footer).toContainText(/more controls? hidden/);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);

        await footer.getByRole("button", { name: "Show" }).click();
        await expect(footer).toContainText("Showing more controls");
        await expect(page.getByLabel("Effort")).toBeVisible();
        expect(patches).toEqual([{ more_controls: true }]);
        // The state survives a reload (it is stored server-side).
        await page.reload();
        await expect(page.getByLabel("Effort")).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(errors).toEqual([]);
      });

      test("a direct link to ?tab=evals still opens the Tests page at Use", async ({ page }) => {
        await open(page, `${agentUrl}?tab=evals`, scheme, "member", "member", false);
        await expect(page.getByRole("heading", { name: /Tests|Evals/ }).first()).toBeVisible();
      });

      test("an owner of a team workspace starts at Set up and a member with the switch on never sees owner-only topics", async ({ page }) => {
        await open(page, "/o/acme/w/main/agents", scheme, "owner", "owner", false);
        if (width >= 768) {
          const nav = page.getByRole("navigation", { name: "Workspace" });
          await expect.poll(() => nav.getByRole("link").allTextContents()).toContain("Skills");
        }
      });

      test("a member with the switch on does not get People or Settings", async ({ page }) => {
        await open(page, "/o/acme/w/main/agents", scheme, "member", "member", true);
        if (width >= 768) {
          const nav = page.getByRole("navigation", { name: "Workspace" });
          await expect.poll(() => nav.getByRole("link").allTextContents()).toEqual(["Overview", "Agents", "Skills", "Apps & accounts", "Internet access", "Activity"]);
        }
      });
    });
  }
}
