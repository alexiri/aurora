import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { mockApi, readinessEmpty } from "./support/mock-api";
import { overflow, type Scheme } from "./support/open";

// P4-ONB-2 (D-141): the instance setup wizard, the Welcome page and guide, and the tour, with the API mocked
// (no backend). Screenshots of each screen in both themes at 1280 and 390 px go to /tmp/aurora-screens/p4-onb-2/;
// axe has no serious or critical violation; nothing scrolls sideways at 390 px; the tour works from the keyboard.
// The three journeys against a real stack (admin wizard to a chat answer, Welcome with and without a starter grant)
// are in onboarding-journeys.spec.ts.

const SHOTS = "/tmp/aurora-screens/p4-onb-2";

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
}

type Scenario = "wizard" | "welcome-key" | "welcome-ready" | "overview-guide";

/** Opens a screen with the mock, overriding what makes the scenario (later routes win). */
async function open(page: Page, path: string, scheme: Scheme, scenario: Scenario) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  const json = (b: unknown) => ({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
  if (scenario === "wizard") {
    await page.route("**/v1/admin/placement", (r) =>
      r.fulfill(
        json({
          saved: false, signup_mode: "open", mode: "personal_org", default_org: null, default_org_role: "member", shared_workspaces: [],
          personal_workspace: { enabled: false, role: "owner", name_template: "{preferred_username}|{email_local}" }, starter_grants: [],
          personal_orgs: { count: 0, items: [] }, warnings: ["placement_unsaved"], updated_at: null, updated_by: null,
        }),
      ),
    );
  }
  if (scenario === "welcome-key" || scenario === "overview-guide") {
    await page.route(/\/v1\/agents\/[^/]+\/model-readiness$/, (r) => r.fulfill(json(readinessEmpty)));
    await page.route("**/v1/llm/providers**", (r) =>
      r.fulfill(
        json({
          providers: [{ id: "litellm", display_name: "LiteLLM", auth_modes: ["bearer"] }],
          endpoints: [{ id: "litellm", provider: "litellm", display_name: "AIGW", permitted: true, key_scopes: [] }],
        }),
      ),
    );
  }
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, `${path} threw`).toEqual([]);
}

const SCREENS: { name: string; path: string; scenario: Scenario; ready: string }[] = [
  { name: "wizard", path: "/admin/setup", scenario: "wizard", ready: "Set up Aurora" },
  { name: "welcome-key", path: "/o/acme/w/main/welcome", scenario: "welcome-key", ready: "Get Daneel talking" },
  { name: "welcome-ready", path: "/o/acme/w/main/welcome", scenario: "welcome-ready", ready: "Get Daneel talking" },
  { name: "overview-guide", path: "/o/acme/w/main", scenario: "overview-guide", ready: "Get Daneel talking" },
];

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      for (const s of SCREENS) {
        test(`${s.name}: screenshot, axe, no horizontal scroll`, async ({ page }) => {
          await open(page, s.path, scheme, s.scenario);
          await expect(page.getByRole("heading", { name: new RegExp(s.ready) }).first()).toBeVisible();
          // Let lists and the form settle before the picture.
          await page.waitForTimeout(300);
          await page.screenshot({ path: `${SHOTS}/${s.name}-${scheme}-${width}.png`, fullPage: true });
          expect(await serious(page)).toEqual([]);
          if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
        });
      }

      test("tour: coach marks over the navigation", async ({ page }) => {
        await open(page, "/o/acme/w/main/welcome", scheme, "welcome-ready");
        await page.getByRole("button", { name: "Show me around" }).click();
        const card = page.getByRole("dialog", { name: "Overview" });
        await expect(card).toBeVisible();
        if (width === 390) {
          // The drawer opens as the tour goes.
          await expect(page.getByRole("navigation", { name: "Workspace" })).toBeVisible();
        }
        await page.screenshot({ path: `${SHOTS}/tour-${scheme}-${width}.png` });
        expect(await serious(page)).toEqual([]);
        await page.getByRole("dialog").getByRole("button", { name: "Next" }).click();
        await expect(page.getByRole("dialog", { name: "Agents and Chat" })).toBeVisible();
        await page.screenshot({ path: `${SHOTS}/tour-2-${scheme}-${width}.png` });
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
      });
    });
  }
}

test.describe("tour from the keyboard", () => {
  test.use({ viewport: { width: 1280, height: 900 } });

  test("Next, Back and Esc; focus is managed and returns; reduced motion is honoured", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await open(page, "/o/acme/w/main/welcome", "light", "welcome-ready");
    const opener = page.getByRole("button", { name: "Show me around" });
    await opener.focus();
    await page.keyboard.press("Enter");
    const first = page.getByRole("dialog", { name: "Overview" });
    await expect(first).toBeFocused();
    await expect(first.getByText(/Stop 1 of \d/)).toBeVisible();
    await page.keyboard.press("ArrowRight");
    await expect(page.getByRole("dialog", { name: "Agents and Chat" })).toBeFocused();
    await page.keyboard.press("ArrowLeft");
    await expect(page.getByRole("dialog", { name: "Overview" })).toBeFocused();
    // The highlighted link is the real navigation entry.
    await expect(page.locator("[data-tour='overview'][data-tour-active='true']")).toHaveCount(1);
    // Tab stays reachable inside the card: Back is disabled on the first stop, Next is the first control.
    await page.keyboard.press("Tab");
    await expect(page.getByRole("dialog").getByRole("button", { name: "Next" })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(opener).toBeFocused();
    await expect(page.locator("[data-tour-active]")).toHaveCount(0);
  });

  test("the wizard says what is ready now and what Helm value changes it", async ({ page }) => {
    await open(page, "/admin/setup", "light", "wizard");
    await expect(page.getByRole("list", { name: "Setup steps" })).toBeVisible();
    await expect(page.getByText("CERN SSO")).toBeVisible();
    await expect(page.getByText("llm.endpoints[].default_model")).toBeVisible();
  });
});
