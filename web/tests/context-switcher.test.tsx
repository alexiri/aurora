import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AppShell } from "@/components/app-shell";
import { wrap } from "./support/render";
import { json } from "./support/api";

// D-389: one switcher at the top of the rail (workspace, then organization), personal workspaces named from the
// viewer's side, the Administration rail with "Back to <Org>", and the account menu headed by the display name.

const nav = vi.hoisted(() => ({
  pathname: "/",
  params: {} as Record<string, string | undefined>,
}));
vi.mock("next/navigation", () => ({
  useParams: () => nav.params,
  usePathname: () => nav.pathname,
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
}));

const now = "2026-11-10T10:00:00Z";
const mkOrg = (id: string, slug: string, name: string, role = "member") => ({
  id, slug, name, my_role: role, app_posture: "open", external_id: "x", settings: {}, created_at: now, role,
});
const mkWs = (id: string, org: string, slug: string, name: string, role: string | null = "member", personal_of: { id: string; name: string } | null = null) => ({
  id, org_id: org, slug, name, my_role: role, memory_language: "en", timezone: "UTC", created_at: now, updated_at: now, personal_of,
});

interface World {
  admin: boolean;
  orgRole: string;
  /** Workspaces of "Users"; the first is Bob's personal one. */
  usersCount: number;
  /** (FIX-235) Workspaces of "Users" this person is NOT in (`my_role: null`): the org list shows them, the switcher does not. */
  notMine?: number;
  acmeCount: number;
  /** Further organizations with one workspace each. */
  extraOrgs: number;
  /** `/v1/instance` `org_creation`: who may create an organization. */
  orgCreation: "instance_admins" | "anyone";
  /** The Administration total `GET /v1/attention/counts` returns for an instance admin (C-1082). */
  attention: number;
  /** The Administration count per topic. */
  attentionTopics?: Record<string, number>;
  /** The organization's total (owners and admins only; `null` for everyone else). */
  orgAttention?: number;
}
let world: World;

const BOB = { id: "u1", name: "Bob Builder" };

function usersWorkspaces() {
  const list = [mkWs("w1", "o1", "bob", "Bob Builder", "owner", BOB)];
  for (let i = 2; i <= world.usersCount; i++) list.push(mkWs(`w${i}`, "o1", `team-${i}`, `Team ${i}`));
  for (let i = 0; i < (world.notMine ?? 0); i++) list.push(mkWs(`nw${i}`, "o1", `other-${i}`, `Not mine ${i}`, null));
  return list;
}

function mockFetch() {
  const users = mkOrg("o1", "users", "Users", world.orgRole);
  const acme = mkOrg("o2", "acme", "Acme Labs");
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const p = url.pathname;
      const mine = usersWorkspaces();
      if (p === "/v1/me") {
        const memberships = [
          {
            org: { id: "o1", slug: "users", name: "Users" },
            role: world.orgRole,
            status: "active",
            // (FIX-235) The session lists only the workspaces the person is in; the org list carries the rest with `my_role: null`.
            workspaces: mine.filter((w) => w.my_role != null).map((w) => ({ id: w.id, slug: w.slug, name: w.name, my_role: w.my_role, personal: w.personal_of !== null })),
          },
        ];
        if (world.acmeCount > 0) {
          memberships.push({
            org: { id: "o2", slug: "acme", name: "Acme Labs" },
            role: "member",
            status: "active",
            workspaces: Array.from({ length: world.acmeCount }, (_, i) => ({ id: `a${i}`, slug: `a-${i}`, name: `Acme ${i}`, my_role: "member", personal: false })),
          });
        }
        for (let i = 1; i <= world.extraOrgs; i++) {
          memberships.push({
            org: { id: `x${i}`, slug: `x${i}`, name: `Org x${i}` },
            role: "member",
            status: "active",
            workspaces: [{ id: `x${i}w`, slug: "main", name: "Main", my_role: "member", personal: false }],
          });
        }
        return json(200, {
          user: { id: "u1", email: "bob@example.org", display_name: "Bob Builder", is_instance_admin: world.admin },
          default_org_id: null,
          memberships,
        });
      }
      if (p === "/v1/me/onboarding" && req?.method === "PATCH") {
        const body = (await req.clone().json()) as { more_controls: boolean };
        return json(200, { welcome_seen: true, guide_dismissed: [], tour_completed_at: null, notices_seen: [], more_controls: body.more_controls });
      }
      if (p === "/v1/attention/counts") {
        // (C-1082) `null` for a scope the caller cannot act on: Administration for instance admins, the organization for its owners and admins.
        const orgId = url.searchParams.get("org");
        const isOrgAdmin = world.orgRole === "owner" || world.orgRole === "admin";
        return json(200, {
          administration: world.admin ? { count: world.attention, topics: world.attentionTopics ?? {} } : null,
          organization: orgId && isOrgAdmin && world.orgAttention !== undefined ? { org_id: orgId, count: world.orgAttention, topics: { "chat-platforms": world.orgAttention } } : null,
          workspace: null,
        });
      }
      if (p === "/v1/instance") return json(200, { org_creation: world.orgCreation, email_delivery: "smtp", features: {} });
      if (p === "/v1/orgs") return json(200, { items: [users, acme] });
      if (p === "/v1/resolve") {
        const slug = url.searchParams.get("org");
        const wsSlug = url.searchParams.get("workspace");
        const org = slug === "acme" ? acme : users;
        return json(200, { org, workspace: wsSlug ? (mine.find((w) => w.slug === wsSlug) ?? mine[0]) : null });
      }
      if (p === "/v1/orgs/o1/workspaces") return json(200, { items: mine });
      if (p === "/v1/orgs/o1") return json(200, { ...users, frozen: null });
      if (p.startsWith("/v1/workspaces/")) return json(200, { ...mine[0], frozen: null });
      if (p.endsWith("/approvals")) return json(200, { items: [] });
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

function render(pathname: string, params: Record<string, string | undefined>, over: Partial<World> = {}) {
  world = { admin: false, orgRole: "member", usersCount: 1, acmeCount: 0, extraOrgs: 0, orgCreation: "instance_admins", attention: 0, ...over };
  nav.pathname = pathname;
  nav.params = params;
  mockFetch();
  return wrap(<AppShell>Page</AppShell>);
}

const onWorkspace = (over: Partial<World> = {}) => render("/o/users/w/bob/agents", { org: "users", ws: "bob" }, over);
const switcher = () => screen.getByTestId("scope-switcher");
async function openSwitcher() {
  const trigger = await within(switcher()).findByRole("button");
  fireEvent.click(trigger);
  return screen.findByPlaceholderText("Find a workspace or organization");
}

beforeEach(() => {
  window.localStorage.clear();
  window.sessionStorage.clear();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("scope switcher", () => {
  it("one organization, one workspace: a plain, non-interactive 'Your workspace / Users', no pill", async () => {
    onWorkspace();
    await waitFor(() => expect(within(switcher()).getByText("Your workspace")).toBeInTheDocument());
    expect(within(switcher()).getByText("Users")).toBeInTheDocument();
    expect(within(switcher()).queryByRole("button")).toBeNull();
    expect(screen.queryByTestId("level-badge")).toBeNull();
    expect(screen.queryByTestId("org-selector")).toBeNull();
    expect(screen.queryByTestId("workspace-selector")).toBeNull();
  });

  it("many: at most five workspaces of the current organization, then 'All n in Org', and the other organization as one row", async () => {
    onWorkspace({ usersCount: 8, acmeCount: 4 });
    await openSwitcher();
    const links = await screen.findAllByRole("link", { name: /Team \d|Your workspace/ });
    expect(links).toHaveLength(5);
    expect(screen.getByRole("link", { name: "All 8 in Users" })).toHaveAttribute("href", "/o/users");
    expect(screen.getByRole("button", { name: /Acme Labs · 4 workspaces/ })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "All workspaces" })).toBeNull();
    // The current workspace is marked and read from the viewer's side.
    const mine = screen.getByRole("link", { name: /Your workspace/ });
    expect(mine).toHaveAttribute("aria-current", "true");
    expect(mine).toHaveTextContent("Owner");
  });

  it("(FIX-235, D-477) lists only the workspaces the person is in in their own right, and 'All n in' counts those", async () => {
    onWorkspace({ usersCount: 6, notMine: 2 });
    await openSwitcher();
    // The six own workspaces (five listed), none of the `my_role: null` ones.
    const links = await screen.findAllByRole("link", { name: /Team \d|Your workspace/ });
    expect(links).toHaveLength(5);
    expect(screen.queryByRole("link", { name: /Not mine/ })).toBeNull();
    // The count covers only the listed (own) workspaces, not the whole organization.
    expect(screen.getByRole("link", { name: "All 6 in Users" })).toBeInTheDocument();
  });

  it("recently used workspaces come first (kept in this browser)", async () => {
    window.localStorage.setItem("aurora.recent-scopes:u1", JSON.stringify(["users/team-8", "users/team-7"]));
    onWorkspace({ usersCount: 8 });
    // The current workspace is recorded once the session has loaded; open the menu after that.
    await within(switcher()).findByText("Your workspace");
    await openSwitcher();
    const names = (await screen.findAllByRole("link", { name: /Team \d|Your workspace/ })).map((a) => a.textContent ?? "");
    // The current workspace is the most recent of all, then the stored order.
    expect(names[0]).toContain("Your workspace");
    expect(names[1]).toContain("Team 8");
    expect(names[2]).toContain("Team 7");
  });

  it("search finds workspaces and organizations, grouped", async () => {
    onWorkspace({ usersCount: 8, acmeCount: 4 });
    const input = await openSwitcher();
    fireEvent.change(input, { target: { value: "acme" } });
    expect(await screen.findByText("Organizations")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Acme Labs/ })).toBeInTheDocument();
    expect(screen.getByText("Workspaces")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Acme 0/ })).toHaveAttribute("href", "/o/acme/w/a-0");
    fireEvent.change(input, { target: { value: "Team 5" } });
    expect(await screen.findByRole("link", { name: /Team 5/ })).toBeInTheDocument();
    expect(screen.queryByText("Organizations")).toBeNull();
    fireEvent.change(input, { target: { value: "zzz" } });
    expect(await screen.findByText(/Nothing matches/)).toBeInTheDocument();
  });

  it("an organization result opens its workspaces, and the back arrow returns", async () => {
    onWorkspace({ usersCount: 2, acmeCount: 7 });
    const input = await openSwitcher();
    fireEvent.change(input, { target: { value: "acme" } });
    fireEvent.click(await screen.findByRole("button", { name: /Acme Labs/ }));
    expect(await screen.findByRole("link", { name: "All 7 in Acme Labs" })).toHaveAttribute("href", "/o/acme");
    expect(screen.getAllByRole("link", { name: /Acme \d/ })).toHaveLength(5);
    fireEvent.click(screen.getByRole("button", { name: "Acme Labs" }));
    expect(await screen.findByRole("link", { name: /Your workspace/ })).toBeInTheDocument();
  });

  it("more than three organizations: 'All n organizations' lists them all", async () => {
    onWorkspace({ usersCount: 2, acmeCount: 1, extraOrgs: 3 });
    await openSwitcher();
    // Three other organizations are listed; Users, Acme and three more make five.
    const all = await screen.findByRole("button", { name: "All 5 organizations" });
    fireEvent.click(all);
    expect(await screen.findByRole("button", { name: /Org x3 · 1 workspace/ })).toBeInTheDocument();
  });

  it("New workspace only when the viewer may create one", async () => {
    onWorkspace({ usersCount: 2, acmeCount: 1 });
    await openSwitcher();
    await screen.findAllByRole("link", { name: /Team 2/ });
    expect(screen.queryByRole("button", { name: /New workspace/ })).toBeNull();
    expect(screen.queryByRole("link", { name: "Manage Users" })).toBeNull();
    cleanup();
    onWorkspace({ usersCount: 2, acmeCount: 1, orgRole: "owner" });
    await openSwitcher();
    expect(await screen.findByRole("button", { name: "New workspace in Users" })).toBeInTheDocument();
    // (P5-REFINE-17, rec 8) The row opens the organization's Manage area on its Overview.
    expect(screen.getByRole("link", { name: "Manage Users" })).toHaveAttribute("href", "/o/users");
  });

  // (P5-REFINE-17 #17, G3, C-1082) What waits in the organization is on its Manage row and on the closed switcher.
  it("(G3) the Manage row and the switcher carry the organization's count, for its owners and admins only", async () => {
    onWorkspace({ usersCount: 2, acmeCount: 1, orgRole: "owner", orgAttention: 3 });
    const trigger = await screen.findByTestId("switcher-attention-badge");
    expect(trigger).toHaveTextContent("3");
    await openSwitcher();
    const row = await screen.findByRole("link", { name: /^Manage Users/ });
    await waitFor(() => expect(within(row).getByTestId("manage-switcher-badge")).toHaveTextContent("3"));
    expect(within(row).getByText("3 need your attention")).toBeInTheDocument();
    cleanup();
    // A member never gets one (the API answers `null`), and a zero shows nothing.
    onWorkspace({ usersCount: 2, acmeCount: 1, orgRole: "member", orgAttention: 3 });
    await screen.findByRole("button", { name: /Bob Builder/ });
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByTestId("switcher-attention-badge")).toBeNull();
    cleanup();
    onWorkspace({ usersCount: 2, acmeCount: 1, orgRole: "owner", orgAttention: 0 });
    await openSwitcher();
    await screen.findByRole("link", { name: /^Manage Users/ });
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByTestId("manage-switcher-badge")).toBeNull();
    expect(screen.queryByTestId("switcher-attention-badge")).toBeNull();
  });

  it("(org-5) New organization is the last row only where sign-up lets this person create one", async () => {
    onWorkspace({ usersCount: 2, acmeCount: 1 });
    await openSwitcher();
    await screen.findAllByRole("link", { name: /Team 2/ });
    expect(screen.queryByRole("button", { name: "New organization" })).toBeNull();
    cleanup();
    onWorkspace({ usersCount: 2, acmeCount: 1, orgCreation: "anyone" });
    await openSwitcher();
    fireEvent.click(await screen.findByRole("button", { name: "New organization" }));
    expect(await screen.findByRole("dialog", { name: "New organization" })).toBeInTheDocument();
  });

  it("keyboard: typing goes to the search field, arrow keys move through the rows, Escape closes", async () => {
    onWorkspace({ usersCount: 3, acmeCount: 1 });
    const input = await openSwitcher();
    await screen.findAllByRole("link", { name: /Team 2/ });
    expect(document.activeElement).toBe(input);
    fireEvent.keyDown(input, { key: "ArrowDown" });
    const first = screen.getAllByRole("link", { name: /Your workspace|Team/ })[0]!;
    expect(document.activeElement).toBe(first);
    fireEvent.keyDown(first, { key: "ArrowUp" });
    expect(document.activeElement).toBe(input);
    fireEvent.keyDown(input, { key: "Escape" });
    await waitFor(() => expect(screen.queryByPlaceholderText("Find a workspace or organization")).toBeNull());
  });

  it("the organization's pages show the organization as the button's first line, never 'All workspaces'", async () => {
    render("/o/users", { org: "users" }, { usersCount: 3 });
    await waitFor(() => expect(within(switcher()).getByText("Organization")).toBeInTheDocument());
    expect(within(switcher()).queryByText("All workspaces")).toBeNull();
  });
});

describe("Administration rail", () => {
  it("has Back to <Org> (where you came from), the level header and no switcher", async () => {
    window.sessionStorage.setItem("aurora.admin-return", JSON.stringify({ href: "/o/acme/w/a-0", name: "Acme Labs" }));
    render("/admin/people", {}, { admin: true });
    const back = await screen.findByRole("link", { name: "Back to Acme Labs" });
    expect(back).toHaveAttribute("href", "/o/acme/w/a-0");
    expect(screen.getByText("You're in")).toBeInTheDocument();
    expect(screen.queryByTestId("scope-switcher")).toBeNull();
    expect(screen.queryByText("Select organization")).toBeNull();
    expect((await screen.findByRole("link", { name: "People & sign-in" })).getAttribute("aria-current")).toBe("page");
    // (P5-REFINE-18 #1) The rail is in two halves, then the footer entry.
    const rail = screen.getByRole("navigation", { name: "Administration" });
    const groups = within(rail).getAllByRole("group");
    expect(groups.map((g) => g.getAttribute("aria-labelledby") && g.querySelector("p")?.textContent)).toEqual(["Policy for everyone", "This installation"]);
    expect(within(groups[0]!).getAllByRole("link").map((a) => a.textContent)).toEqual(["What agents may do", "Apps", "Chat platforms", "People & sign-in", "Data"]);
    expect(within(groups[1]!).getAllByRole("link").map((a) => a.textContent)).toEqual(["Health & runners", "AI endpoints", "Set in Helm", "All settings"]);
    // "Administration" is said once: the header's "You're in Administration"; the footer entry is only outside Administration.
    const shell = screen.getByRole("complementary", { name: "Sidebar" });
    expect(within(shell).queryByRole("link", { name: "Administration" })).toBeNull();
    expect(within(shell).getAllByText("Administration")).toHaveLength(1);
  });

  it("outside Administration the sidebar's foot has the Administration entry", async () => {
    onWorkspace({ admin: true });
    const shell = screen.getByRole("complementary", { name: "Sidebar" });
    expect(await within(shell).findByRole("link", { name: "Administration" })).toHaveAttribute("href", "/admin");
  });

  it.each(["javascript:alert(1)", "//evil.example/x", "https://evil.example/", "/\\evil"])("ignores a remembered href that is not an internal path: %s", async (href) => {
    window.sessionStorage.setItem("aurora.admin-return", JSON.stringify({ href, name: "Evil" }));
    render("/admin", {}, { admin: true, usersCount: 2 });
    const back = await screen.findByRole("link", { name: "Back to Users" });
    expect(back).toHaveAttribute("href", "/o/users/w/bob");
  });

  it("without a remembered place goes back to the first workspace", async () => {
    render("/admin", {}, { admin: true, usersCount: 2 });
    const back = await screen.findByRole("link", { name: "Back to Users" });
    expect(back).toHaveAttribute("href", "/o/users/w/bob");
  });

  it("remembers the organization page you were on", async () => {
    onWorkspace({ admin: true });
    await waitFor(() => expect(window.sessionStorage.getItem("aurora.admin-return")).toContain("/o/users/w/bob"));
  });

  it("is not rendered for a signed-in member on /admin", async () => {
    render("/admin", {}, { admin: false });
    await screen.findByRole("button", { name: /Bob Builder/ });
    expect(screen.queryByRole("navigation", { name: "Administration" })).toBeNull();
    expect(screen.queryByText("You're in")).toBeNull();
  });
});

describe("Administration count badge (P5-REFINE-18 #18, C-1022)", () => {
  it("shows the count on Overview in the Administration rail, and on the sidebar-foot entry outside it", async () => {
    render("/admin/people", {}, { admin: true, attention: 2 });
    const nav = await screen.findByRole("navigation", { name: "Administration" });
    const overview = within(nav).getByRole("link", { name: /^Overview/ });
    await waitFor(() => expect(within(overview).getByTestId("admin-attention-badge")).toHaveTextContent("2"));
    // Other topics carry no total.
    expect(within(within(nav).getByRole("link", { name: /^People/ })).queryByTestId("admin-attention-badge")).toBeNull();
    // Inside Administration the foot entry is gone (the rail header names it).
    expect(within(screen.getByRole("complementary", { name: "Sidebar" })).queryByRole("link", { name: /^Administration/ })).toBeNull();
    cleanup();
    onWorkspace({ admin: true, attention: 2 });
    const shell = screen.getByRole("complementary", { name: "Sidebar" });
    const foot = await within(shell).findByRole("link", { name: /^Administration 2/ });
    expect(within(foot).getByTestId("admin-attention-badge")).toHaveTextContent("2");
  });

  it("shows the count on the account menu's Administration entry", async () => {
    onWorkspace({ admin: true, attention: 3 });
    const trigger = await screen.findByRole("button", { name: /Bob Builder/ });
    fireEvent.keyDown(trigger, { key: "Enter" });
    const entry = await screen.findByRole("menuitem", { name: /^Administration/ });
    await waitFor(() => expect(within(entry).getByTestId("admin-attention-badge")).toHaveTextContent("3"));
  });

  it("(G4) a topic entry carries its own part, and no badge at zero", async () => {
    render("/admin/people", {}, { admin: true, attention: 2, attentionTopics: { "chat-platforms": 1 } });
    const nav = await screen.findByRole("navigation", { name: "Administration" });
    const chat = within(nav).getByRole("link", { name: /^Chat platforms/ });
    await waitFor(() => expect(within(chat).getByTestId("admin-attention-badge-chat-platforms")).toHaveTextContent("1"));
    expect(within(chat).getByText("1 needs your attention")).toBeInTheDocument();
    // Overview carries the total once; People has none of its own.
    expect(within(within(nav).getByRole("link", { name: /^Overview/ })).getByTestId("admin-attention-badge")).toHaveTextContent("2");
    expect(within(within(nav).getByRole("link", { name: /^People/ })).queryByTestId("admin-attention-badge-people")).toBeNull();
  });

  it("shows no badge for a member, and none for an empty list", async () => {
    render("/admin/people", {}, { admin: true, attention: 0 });
    await screen.findByRole("navigation", { name: "Administration" });
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByTestId("admin-attention-badge")).toBeNull();
    cleanup();
    onWorkspace({ admin: false, attention: 4 });
    await screen.findByRole("button", { name: /Bob Builder/ });
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByTestId("admin-attention-badge")).toBeNull();
  });
});

describe("account menu", () => {
  async function openMenu() {
    const trigger = await screen.findByRole("button", { name: /Bob Builder/ });
    fireEvent.keyDown(trigger, { key: "Enter" });
  }

  it("has the per-person Show more controls switch above Show me around; toggling it does not close the menu (D-406)", async () => {
    onWorkspace();
    await openMenu();
    const sw = await screen.findByRole("menuitemcheckbox", { name: "Show more controls" });
    // The test client starts with the switch on (tests/support/render.tsx).
    expect(sw).toHaveAttribute("aria-checked", "true");
    const order = [...document.querySelectorAll("[role^='menuitem']")].map((i) => i.textContent);
    expect(order.indexOf("Show more controls")).toBeLessThan(order.indexOf("Show me around"));
    fireEvent.click(sw);
    // Optimistic: it reads off at once; the menu is still open.
    await waitFor(() => expect(screen.getByRole("menuitemcheckbox", { name: "Show more controls" })).toHaveAttribute("aria-checked", "false"));
    expect(screen.getByText("bob@example.org")).toBeInTheDocument();
  });

  it("lists Keyboard shortcuts only while the switch is on (P5-REFINE-19)", async () => {
    onWorkspace();
    await openMenu();
    expect(await screen.findByRole("menuitem", { name: "Keyboard shortcuts" })).toBeInTheDocument();
    fireEvent.click(await screen.findByRole("menuitemcheckbox", { name: "Show more controls" }));
    await waitFor(() => expect(screen.queryByRole("menuitem", { name: "Keyboard shortcuts" })).toBeNull());
  });

  it("has Search with the palette shortcut in the rail's footer (P5-REFINE-19)", async () => {
    onWorkspace();
    const shell = screen.getByRole("complementary", { name: "Sidebar" });
    const search = within(shell).getByRole("button", { name: /^Search/ });
    expect(search).toHaveTextContent(/Ctrl K|⌘K/);
  });

  it("is headed by the display name and e-mail, with Sign out apart", async () => {
    onWorkspace();
    await openMenu();
    expect(await screen.findByText("bob@example.org")).toBeInTheDocument();
    const items = (await screen.findAllByRole("menuitem")).map((i) => i.textContent);
    // (P5-REFINE-19) "Keyboard shortcuts" is an Expert entry: it shows with Show more controls on.
    expect(items).toEqual(["Your account", "Keyboard shortcuts", "Show me around", "Sign out"]);
    expect(screen.getAllByRole("separator")).toHaveLength(2);
  });

  it("offers Administration to instance admins only", async () => {
    onWorkspace({ admin: true });
    await openMenu();
    const items = (await screen.findAllByRole("menuitem")).map((i) => i.textContent);
    expect(items).toEqual(["Your account", "Administration", "Keyboard shortcuts", "Show me around", "Sign out"]);
  });
});

describe("navigation entries", () => {
  it("carries Administration at the foot of the sidebar for instance admins only (C-962)", async () => {
    onWorkspace({ admin: true });
    const shell = screen.getByRole("complementary", { name: "Sidebar" });
    expect((await within(shell).findByRole("link", { name: "Administration" })).getAttribute("href")).toBe("/admin");
    cleanup();
    onWorkspace({ admin: false });
    await screen.findByRole("button", { name: /Bob Builder/ });
    expect(screen.queryByRole("link", { name: "Administration" })).toBeNull();
  });

  it("has no Members item in the workspace nav (Settings › Access owns workspace people)", async () => {
    onWorkspace();
    await screen.findByRole("link", { name: "Settings" });
    expect(screen.queryByRole("link", { name: "Members" })).toBeNull();
  });

  it("shows the account navigation on account pages, with a switcher that is busy until the session loads and then keeps the last scope", async () => {
    render("/account", {});
    // (firstday-2) Loading first, never a live-looking empty card; then the last scope kept for Account pages (P5-REFINE-2).
    expect(within(switcher()).getByText("Loading workspaces…")).toBeInTheDocument();
    expect(await screen.findByRole("link", { name: "Profile" })).toBeInTheDocument();
    expect(await within(switcher()).findByText("Users")).toBeInTheDocument();
  });
});
