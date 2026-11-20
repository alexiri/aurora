import { expect as baseExpect, test, type Locator, type Page } from "@playwright/test";

import { id, mockApi, org, user, ws } from "./support/mock-api";
import { openRoute } from "./support/open";

// The first request of a route compiles it in `next dev`; on a loaded host that outlasts the default 5 s.
const expect = baseExpect.configure({ timeout: 30_000 });

/** Opens a route with the mock API's answers replaced (`openRoute` installs a fresh, unmodified mock). */
async function openWith(page: Page, path: string, scheme: "light" | "dark", overrides: Record<string, unknown>) {
  await mockApi(page, overrides);
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await expect(page.getByRole("navigation").first()).toBeVisible();
}

// P5-IA-1 (C-960, C-961, C-962): one structure for every level. The navigation lists each level's topics by role, the
// level badge names where a page lives, and every old route lands on its new topic page and tab (query and anchor kept).

const member = { ...user, is_instance_admin: false };
const meAs = (u: typeof user, orgRole: string, wsRole: string) => ({
  "/v1/me": {
    user: u,
    memberships: [{ org: { ...org, my_role: orgRole }, status: "active", workspaces: [{ ...ws, my_role: wsRole, personal: false }] }],
    default_org_id: id(1),
    client_address: "203.0.113.7",
  },
  [`/v1/orgs/${id(1)}`]: { ...org, my_role: orgRole },
  [`/v1/workspaces/${id(2)}`]: { ...ws, my_role: wsRole },
});

const labels = (nav: Locator) => nav.getByRole("link").allTextContents();

for (const scheme of ["light", "dark"] as const) {
  test.describe(`navigation by role (${scheme})`, () => {
    // (P5-REFINE-17, rec 8) The organization is the "Manage Acme Labs" area; the rail stays a workspace rail.
    test("a member's Manage area lists exactly Overview, AI models and Internet access", async ({ page }) => {
      await openWith(page, "/o/acme", scheme, meAs(member, "member", "member"));
      const nav = page.getByRole("navigation", { name: "Manage Acme Labs" });
      await expect.poll(() => labels(nav)).toEqual(["Overview", "AI models", "Internet access"]);
      await expect(page.getByRole("link", { name: "Administration" })).toHaveCount(0);
      await expect(page.getByRole("complementary", { name: "Sidebar" }).getByRole("navigation", { name: "Organization" })).toHaveCount(0);
    });

    test("an organization owner's Manage area lists every organization topic", async ({ page }) => {
      await openWith(page, "/o/acme", scheme, meAs(member, "owner", "owner"));
      const nav = page.getByRole("navigation", { name: "Manage Acme Labs" });
      await expect.poll(() => labels(nav)).toEqual([
        "Overview", "Agents", "People", "Apps & accounts", "Chat platforms", "AI models", "Internet access", "Data", "Settings",
      ]);
      await expect(page.getByRole("link", { name: /^Back to / })).toBeVisible();
    });

    test("a workspace member sees no Approvals, People or Settings", async ({ page }) => {
      await openWith(page, "/o/acme/w/main", scheme, meAs(member, "member", "member"));
      const nav = page.getByRole("complementary", { name: "Sidebar" }).getByRole("navigation", { name: "Workspace" });
      await expect.poll(() => labels(nav)).toEqual(["Overview", "Agents", "Skills", "Apps & accounts", "Internet access", "Activity"]);
    });

    test("an instance admin reaches Administration from the footer link", async ({ page }) => {
      await openRoute(page, "/o/acme/w/main", scheme);
      const sidebar = page.getByRole("complementary", { name: "Sidebar" });
      await sidebar.getByRole("link", { name: "Administration" }).click();
      await expect(page).toHaveURL(/\/admin$/);
      await expect(page.getByRole("navigation", { name: "Administration" }).getByRole("link")).toHaveText([
        "Overview", "Agents", "Apps & accounts", "AI models", "Internet access", "People", "Chat platforms", "Data", "Platform", "Settings",
      ]);
    });

    // D-389: the rail says where you are (the switcher, or Administration's header); no header carries a level pill.
    test("the rail names the level and no page header carries a pill", async ({ page }) => {
      const sidebar = page.getByRole("complementary", { name: "Sidebar" });
      await openRoute(page, "/admin/data", scheme);
      await expect(sidebar.getByText("You're in")).toBeVisible();
      await expect(page.getByTestId("level-badge")).toHaveCount(0);
      await openRoute(page, "/o/acme/data", scheme);
      await expect(sidebar.getByTestId("scope-switcher")).toContainText("Acme Labs");
      await expect(page.getByTestId("level-badge")).toHaveCount(0);
      await openRoute(page, "/o/acme/w/main/settings", scheme);
      await expect(sidebar.getByTestId("scope-switcher")).toContainText("Main workspace");
      await expect(page.getByTestId("level-badge")).toHaveCount(0);
    });
  });
}

// One parametrised test: every old route lands on its new topic page and tab (C-960).
const MOVED: [string, RegExp][] = [
  ["/admin/setup", /\/admin$/],
  ["/admin/users", /\/admin\/people\?tab=users$/],
  ["/admin/orgs", /\/admin\/people\?tab=organizations$/],
  [`/admin/orgs/${id(1)}`, new RegExp(`/admin/people/organizations/${id(1)}$`)],
  ["/admin/identity", /\/admin\/people\?tab=sign-in$/],
  ["/admin/placement", /\/admin\/people\?tab=new-users$/],
  ["/admin/plugins", /\/admin\/apps\?tab=apps$/],
  ["/admin/plugins/github", /\/admin\/apps\/packages\/github$/],
  ["/admin/addon-sources", /\/admin\/apps\?tab=advanced#package-sources$/],
  ["/admin/trusted-keys", /\/admin\/apps\?tab=advanced#publishers$/],
  ["/admin/host-claims", /\/admin\/apps\?tab=advanced#host-conflicts$/],
  ["/admin/instances", /\/admin\/apps\?tab=self-hosted$/],
  ["/admin/oauth-clients", /\/admin\/apps\?tab=oauth-clients$/],
  ["/admin/connectors", /\/admin\/chat-platforms$/],
  ["/admin/secret-stores", /\/admin\/platform\?tab=secret-stores$/],
  ["/admin/runners", /\/admin\/platform\?tab=runners$/],
  ["/admin/storage", /\/admin\/data\?tab=storage$/],
  ["/admin/audit", /\/admin\/data\?tab=audit$/],
  ["/admin/settings/llm", /\/admin\/ai-models\?tab=endpoints$/],
  ["/admin/settings/email", /\/admin\/platform\?tab=email$/],
  ["/o/acme/members", /\/o\/acme\/people\?tab=members$/],
  ["/o/acme/groups", /\/o\/acme\/people\?tab=groups$/],
  [`/o/acme/groups/${id(401)}`, new RegExp(`/o/acme/people/groups/${id(401)}$`)],
  ["/o/acme/credentials", /\/o\/acme\/apps\?tab=accounts$/],
  ["/o/acme/credentials?tab=llm", /\/o\/acme\/apps\?tab=accounts&kind=ai-key$/],
  ["/o/acme/credentials?tab=oauth", /\/o\/acme\/apps\?tab=oauth-clients$/],
  ["/o/acme/apps", /\/o\/acme\/apps\?tab=apps$/],
  ["/o/acme/governance", /\/o\/acme\/ai-models$/],
  ["/o/acme/governance?tab=egress", /\/o\/acme\/internet-access$/],
  ["/o/acme/governance?tab=apps", /\/o\/acme\/apps\?tab=apps$/],
  ["/o/acme/governance?tab=byo", /\/o\/acme\/agents\?tab=byo$/],
  ["/o/acme/policy", /\/o\/acme\/internet-access$/],
  ["/o/acme/skills", /\/o\/acme\/agents\?tab=skills$/],
  ["/o/acme/skills/libraries", /\/o\/acme\/agents\/skills\/libraries$/],
  ["/o/acme/channels", /\/o\/acme\/chat-platforms$/],
  ["/o/acme/instances", /\/o\/acme\/apps\?tab=self-hosted$/],
  ["/o/acme/api-keys", /\/o\/acme\/settings\?tab=api-keys$/],
  ["/o/acme/audit", /\/o\/acme\/data\?tab=audit$/],
  ["/o/acme/storage", /\/o\/acme\/data\?tab=storage$/],
  ["/o/acme/settings/directory", /\/o\/acme\/people\?tab=directory-sync$/],
  ["/o/acme/w/main/credentials", /\/o\/acme\/w\/main\/apps\?tab=accounts$/],
  ["/o/acme/w/main/credentials?tab=llm", /\/o\/acme\/w\/main\/apps\?tab=accounts&kind=ai-key$/],
  ["/o/acme/w/main/credentials?tab=apps", /\/o\/acme\/w\/main\/apps\?tab=apps$/],
  ["/o/acme/w/main/credentials?tab=proposals", /\/o\/acme\/w\/main\/apps\?tab=accounts#requests$/],
  ["/o/acme/w/main/policy", /\/o\/acme\/w\/main\/internet-access$/],
  ["/o/acme/w/main/settings?tab=access", /\/o\/acme\/w\/main\/people$/],
];

test.describe("every old route lands on its new page (C-960)", () => {
  for (const [from, to] of MOVED) {
    test(`${from}`, async ({ page }) => {
      await mockApi(page);
      await page.goto(from);
      await expect(page).toHaveURL(to);
      await expect(page.getByText("This page could not be found.")).toHaveCount(0);
      await expect(page.getByRole("heading", { level: 1 }).first()).toBeVisible();
    });
  }

  test("a connect link keeps its parameter and opens the Connect dialog at the new route", async ({ page }) => {
    await mockApi(page);
    await page.goto("/o/acme/w/main/credentials?tab=apps&connect=github");
    await expect(page).toHaveURL(/\/o\/acme\/w\/main\/apps\?tab=apps&connect=github$/);
    // The connect journey (P5-UX-3; titled "<App> for <Agent>" since P5-POLISH-5), the workspace's one agent preselected.
    await expect(page.getByRole("dialog", { name: "GitHub for Daneel" })).toBeVisible();
  });

  test("a reconnect link opens the Reconnect dialog on Accounts", async ({ page }) => {
    await mockApi(page);
    await page.goto("/o/acme/w/main/credentials?tab=connections&reconnect=c1");
    await expect(page).toHaveURL(/\/o\/acme\/w\/main\/apps\?tab=accounts&reconnect=c1$/);
  });
});
