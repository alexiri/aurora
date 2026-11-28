import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AppShell } from "@/components/app-shell";
import { ManageGate } from "@/components/org/manage-gate";
import { OrgOverview } from "@/components/org/org-overview";
import { OrgWorkspaces } from "@/components/org/org-workspaces";
import { backLink, lastWorkspace, parsePlaceRecord } from "@/lib/nav/manage-return";
import { homeTarget } from "@/lib/nav/landing";
import { wrap } from "./support/render";
import { json } from "./support/api";

// P5-NAV-2 (D-508, C-1178): the rail shows one place with its place card and back line; Manage <org> replaces the rail;
// Workspaces is its own page; the Overview is summary cards and Needs you; members don't see Manage.

const nav = vi.hoisted(() => ({
  pathname: "/o/acme",
  params: { org: "acme" } as Record<string, string | undefined>,
  replace: vi.fn(),
}));
vi.mock("next/navigation", () => ({
  useParams: () => nav.params,
  usePathname: () => nav.pathname,
  useRouter: () => ({ push: vi.fn(), replace: nav.replace }),
  useSearchParams: () => new URLSearchParams(),
}));

const now = "2026-11-10T10:00:00Z";
const ME = "u1";

interface World {
  orgRole: "owner" | "admin" | "member";
  instanceAdmin: boolean;
  /** The caller's own workspaces in Acme. */
  mine: { slug: string; name: string; personal: boolean }[];
}
let world: World;

const wsRow = (id: string, slug: string, name: string, my_role: string | null, over: Record<string, unknown> = {}) => ({
  id, org_id: "o1", slug, name, my_role, administers: my_role == null, memory_language: "en", timezone: "UTC", created_at: now, updated_at: now, personal_of: null, ...over,
});

const allWorkspaces = () => [
  wsRow("w-ops", "ops", "Ops", null),
  wsRow("w-alice", "alice", "Alice Admin", "owner", { personal_of: { id: ME, name: "Alice Admin" } }),
  wsRow("w-bob", "bob", "Bob Builder", null, { personal_of: { id: "u2", name: "Bob Builder" } }),
];

const item = {
  id: "broken_account:s1", kind: "broken_account", topic: "accounts", subject_id: "s1", level: "organization", state: "open", own: false,
  title: "The GitHub account for Acme failed to refresh its token", detail: "Workspaces that use it stop until it is renewed.",
  since: now, href: "/o/acme/apps?tab=accounts", decision: true, actors: [], actors_total: 0, can_decline: false,
  action: { label: "Fix", href: "/o/acme/apps?tab=accounts" },
};

function serve() {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const p = url.pathname;
      const org = { id: "o1", slug: "acme", name: "Acme", my_role: world.orgRole, app_posture: "open", external_id: "x", settings: {}, created_at: now };
      if (p === "/v1/me") {
        return json(200, {
          user: { id: ME, email: "alice@example.test", display_name: "Alice Admin", is_instance_admin: world.instanceAdmin },
          default_org_id: null,
          memberships: [
            {
              org: { id: "o1", slug: "acme", name: "Acme" },
              role: world.orgRole,
              status: "active",
              workspaces: world.mine.map((w) => ({ id: `w-${w.slug}`, slug: w.slug, name: w.name, my_role: "owner", personal: w.personal })),
            },
          ],
        });
      }
      if (p === "/v1/resolve") {
        const slug = url.searchParams.get("workspace");
        return json(200, { org, workspace: slug ? allWorkspaces().find((w) => w.slug === slug) : null });
      }
      if (p === "/v1/orgs/o1") return json(200, org);
      if (p === "/v1/orgs/o1/workspaces") return json(200, { items: allWorkspaces(), next_cursor: null });
      const one = p.match(/^\/v1\/workspaces\/([^/]+)$/);
      if (one) {
        const w = allWorkspaces().find((x) => x.id === one[1]) ?? allWorkspaces()[0]!;
        return json(200, { ...w, audience: { people: 3, owners: [{ id: "u3", display_name: "Grace Hopper" }], owners_total: 3, emergency: null, emergency_active_more: 0 } });
      }
      if (p === "/v1/orgs/o1/members") return json(200, { items: [{ user_id: ME }, { user_id: "u2" }], next_cursor: null });
      if (p === "/v1/orgs/o1/agents") {
        return json(200, { items: ["w-ops", "w-ops", "w-bob"].map((w, i) => ({ id: `a${i}`, handle: `a${i}`, name: `Agent ${i}`, kind: "hosted", state: "ready", workspace: { id: w, slug: w, name: w } })), next_cursor: null });
      }
      if (p === "/v1/orgs/o1/accounts") {
        return json(200, { items: [{ kind: "ai_key", ref: { type: "secret", id: "k1" } }, { kind: "app", ref: { type: "connection", id: "c1" } }], next_cursor: null });
      }
      if (p === "/v1/orgs/o1/attention") return json(200, { items: url.searchParams.get("state") === "done" ? [] : [item], count: 1 });
      if (p === "/v1/orgs/o1/invitations") return json(200, { items: [], next_cursor: null });
      if (p === "/v1/attention/counts") return json(200, { administration: null, organization: { org_id: "o1", count: 1, topics: { apps: 1 } }, workspace: null });
      if (p === "/v1/instance") return json(200, { org_creation: "instance_admins", features: {} });
      return json(200, { items: [], next_cursor: null });
    }),
  );
}

const owner: World = { orgRole: "owner", instanceAdmin: false, mine: [{ slug: "alice", name: "Alice Admin", personal: true }, { slug: "ops2", name: "Ops two", personal: false }] };

function shell(pathname: string, params: Record<string, string | undefined>, over: Partial<World> = {}) {
  world = { ...owner, ...over };
  nav.pathname = pathname;
  nav.params = params;
  serve();
  return wrap(<AppShell>Page</AppShell>);
}

const sidebar = () => screen.getByRole("complementary", { name: "Sidebar" });
const record = (over: Record<string, unknown> = {}) =>
  window.sessionStorage.setItem(
    "aurora.place-return",
    JSON.stringify({ userId: ME, orgSlug: "acme", wsSlug: "ops2", wsName: "Ops two", path: "/o/acme/w/ops2/agents/daneel?tab=memory", reader: false, ...over }),
  );

beforeEach(() => {
  window.localStorage.clear();
  window.sessionStorage.clear();
  window.history.replaceState(null, "", "/");
  nav.replace.mockClear();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("Manage <org> replaces the rail (D-508(b))", () => {
  it("shows the organization's grouped menu, and no workspace menu beside it", async () => {
    shell("/o/acme/people", { org: "acme" });
    const menu = await within(sidebar()).findByRole("navigation", { name: "Manage Acme" });
    await waitFor(() => expect(within(menu).getAllByRole("link")).toHaveLength(11));
    expect(within(menu).getAllByRole("link").map((a) => a.getAttribute("href"))).toEqual([
      "/o/acme", "/o/acme/workspaces", "/o/acme/people", "/o/acme/ai-models", "/o/acme/apps?tab=accounts", "/o/acme/chat-platforms",
      "/o/acme/internet-access", "/o/acme/skills", "/o/acme/agents", "/o/acme/data", "/o/acme/settings",
    ]);
    const groups = within(menu).getAllByRole("group");
    expect(groups.map((g) => g.querySelector("p")?.textContent)).toEqual(["What agents can use", "Oversight"]);
    expect(within(groups[1]!).getAllByRole("link").map((a) => a.textContent)).toEqual(["Agents", "Data"]);
    expect(within(menu).getByRole("link", { name: "People" })).toHaveAttribute("aria-current", "page");
    expect(screen.queryByRole("navigation", { name: "Workspace" })).toBeNull();
    // The place card: the organization, and what it is.
    expect(within(sidebar()).getByTestId("scope-switcher")).toHaveTextContent("Acme Organization");
    // Today's counts: the total on Overview, a topic's own part; none at zero.
    await waitFor(() => expect(within(menu).getByTestId("manage-attention-badge-overview")).toHaveTextContent("1"));
    expect(within(menu).getByTestId("manage-attention-badge-apps")).toHaveTextContent("1");
    expect(within(menu).queryByTestId("manage-attention-badge-people")).toBeNull();
  });

  it("the workspace's manage page sits under Workspaces, though its address has a workspace parameter too", async () => {
    // `/o/[org]/workspaces/[ws]`: Next passes `ws` here as it does under `/w/`.
    shell("/o/acme/workspaces/bob", { org: "acme", ws: "bob" });
    const menu = await within(sidebar()).findByRole("navigation", { name: "Manage Acme" });
    expect(await within(menu).findByRole("link", { name: "Workspaces" })).toHaveAttribute("aria-current", "page");
    expect(screen.queryByRole("navigation", { name: "Workspace" })).toBeNull();
    expect(within(sidebar()).getByTestId("scope-switcher")).toHaveTextContent("Acme Organization");
  });
});

describe("the back line (D-508(h), C-1178)", () => {
  it("returns to the workspace and page you came from", async () => {
    record();
    shell("/o/acme", { org: "acme" });
    const back = await within(sidebar()).findByRole("link", { name: "Back to Ops two" });
    expect(back).toHaveAttribute("href", "/o/acme/w/ops2/agents/daneel?tab=memory");
  });

  it("opened from a link with no record, goes to your last workspace in the organization", async () => {
    window.localStorage.setItem(`aurora.recent-scopes:${ME}`, JSON.stringify(["acme/ops2"]));
    shell("/o/acme", { org: "acme" });
    expect(await within(sidebar()).findByRole("link", { name: "Back to Ops two" })).toHaveAttribute("href", "/o/acme/w/ops2");
  });

  it("is hidden with no workspace to go back to", async () => {
    shell("/o/acme", { org: "acme" }, { mine: [] });
    await within(sidebar()).findByRole("navigation", { name: "Manage Acme" });
    expect(within(sidebar()).queryByTestId("place-back")).toBeNull();
  });

  it("never follows another person's record, or one that leaves the workspace", async () => {
    record({ userId: "someone-else" });
    shell("/o/acme", { org: "acme" });
    expect(await within(sidebar()).findByRole("link", { name: "Back to Your workspace" })).toHaveAttribute("href", "/o/acme/w/alice");
    const base = { userId: ME, orgSlug: "acme", wsSlug: "ops2", wsName: "x", reader: false };
    for (const path of [
      "https://evil.example/o/acme/w/ops2", "//evil.example", "/\\evil", "/o/acme/w/ops2/../../../admin", "/o/acme/w/ops2/%2e%2e/x",
      "/o/acme/w/ops2-evil", "/admin", "javascript:alert(1)",
    ]) {
      expect(parsePlaceRecord(JSON.stringify({ ...base, path })), path).toBeNull();
    }
    expect(parsePlaceRecord(JSON.stringify({ ...base, path: "/o/acme/w/ops2?tab=a" }))?.path).toBe("/o/acme/w/ops2?tab=a");
  });

  it("drops a record whose workspace is gone, unless it was read under emergency access", () => {
    const memberships = [{ org: { slug: "acme" }, workspaces: [{ slug: "ops2", name: "Ops two" }] }];
    const gone = { userId: ME, orgSlug: "acme", wsSlug: "gone", wsName: "Gone", path: "/o/acme/w/gone", reader: false };
    expect(backLink(gone, ME, memberships, "acme")?.href).toBe("/o/acme/w/ops2");
    expect(backLink({ ...gone, reader: true }, ME, memberships, "acme")?.href).toBe("/o/acme/w/gone");
    expect(backLink(gone, ME, undefined, "acme")).toBeUndefined();
  });

  it("the last workspace is the one used last, else the personal one, else the first", () => {
    const m = [{ org: { slug: "acme" }, workspaces: [{ slug: "team", name: "Team" }, { slug: "me", name: "Me", personal: true }] }];
    expect(lastWorkspace(ME, m, "acme")?.wsSlug).toBe("me");
    window.localStorage.setItem(`aurora.recent-scopes:${ME}`, JSON.stringify(["acme/team"]));
    expect(lastWorkspace(ME, m, "acme")?.wsSlug).toBe("team");
    expect(lastWorkspace(ME, m, "other")).toBeUndefined();
  });

  it("remembers the workspace page, with its query, before a click leaves it", async () => {
    window.history.replaceState(null, "", "/o/acme/w/alice/agents");
    shell("/o/acme/w/alice/agents", { org: "acme", ws: "alice" });
    await waitFor(() => expect(window.sessionStorage.getItem("aurora.place-return")).toContain("/o/acme/w/alice/agents"));
    // A tab chosen in the query does not change the path: the click that leaves reads the address again.
    window.history.replaceState(null, "", "/o/acme/w/alice/agents?view=archived");
    fireEvent.click(document.body);
    expect(parsePlaceRecord(window.sessionStorage.getItem("aurora.place-return"))).toMatchObject({
      path: "/o/acme/w/alice/agents?view=archived", wsName: "Your workspace", userId: ME,
    });
    // A workspace shows no back line: it is the home.
    expect(within(sidebar()).queryByTestId("place-back")).toBeNull();
  });

  it("does not remember a workspace this person only administers (they are sent to its manage page)", async () => {
    window.history.replaceState(null, "", "/o/acme/w/ops");
    shell("/o/acme/w/ops", { org: "acme", ws: "ops" });
    await within(sidebar()).findByRole("navigation", { name: "Workspace" });
    await new Promise((r) => setTimeout(r, 60));
    fireEvent.click(document.body);
    expect(window.sessionStorage.getItem("aurora.place-return")).toBeNull();
  });
});

describe("the place card of Administration and Your account (nav-4)", () => {
  it("Your account: the person's card, with the way back", async () => {
    record();
    shell("/account", {});
    await waitFor(() => expect(within(sidebar()).getByTestId("place-card")).toHaveTextContent("Alice Admin Your account"));
    expect(within(sidebar()).getByRole("link", { name: "Back to Ops two" })).toBeInTheDocument();
    expect(within(sidebar()).getByRole("navigation", { name: "Account" })).toBeInTheDocument();
  });

  it("Administration: 'Administration · This Aurora', with the way back", async () => {
    record();
    shell("/admin", {}, { instanceAdmin: true });
    await waitFor(() => expect(within(sidebar()).getByTestId("place-card")).toHaveTextContent("Administration This Aurora"));
    expect(within(sidebar()).getByRole("link", { name: "Back to Ops two" })).toBeInTheDocument();
    expect(within(sidebar()).queryByText("You're in")).toBeNull();
  });
});

describe("members don't see Manage (D-508(e), C-1178)", () => {
  const gate = (pathname: string, params: Record<string, string>, over: Partial<World>) => {
    world = { ...owner, ...over };
    nav.pathname = pathname;
    nav.params = params;
    serve();
    return wrap(<ManageGate>Manage page</ManageGate>);
  };

  it.each(["/o/acme", "/o/acme/people", "/o/acme/ai-models", "/o/acme/workspaces", "/o/acme/workspaces/bob", "/o/acme/people/groups/g1"])(
    "sends a member from %s to the workspace they used last there, drawing nothing of Manage",
    async (pathname) => {
      window.localStorage.setItem(`aurora.recent-scopes:${ME}`, JSON.stringify(["acme/ops2"]));
      gate(pathname, { org: "acme" }, { orgRole: "member" });
      await waitFor(() => expect(nav.replace).toHaveBeenCalledWith("/o/acme/w/ops2"));
      expect(screen.queryByText("Manage page")).toBeNull();
    },
  );

  it("draws nothing of Manage while the session is still on its way, even when the organization answered first", async () => {
    const mounted = vi.fn();
    function ManagePage() {
      mounted();
      return <p>Manage page</p>;
    }
    world = { ...owner, orgRole: "member" };
    nav.pathname = "/o/acme/people";
    nav.params = { org: "acme" };
    serve();
    const answer = globalThis.fetch;
    vi.stubGlobal("fetch", async (input: Request | string | URL) => {
      const url = new URL(input instanceof Request ? input.url : String(input), "http://localhost");
      if (url.pathname === "/v1/me") await new Promise((r) => setTimeout(r, 150));
      return answer(input);
    });
    wrap(
      <ManageGate>
        <ManagePage />
      </ManageGate>,
    );
    await waitFor(() => expect(nav.replace).toHaveBeenCalled());
    expect(mounted).not.toHaveBeenCalled();
  });

  it("a session that cannot be read still sends a member away rather than breaking", async () => {
    world = { ...owner, orgRole: "member" };
    nav.pathname = "/o/acme/people";
    nav.params = { org: "acme" };
    serve();
    const answer = globalThis.fetch;
    vi.stubGlobal("fetch", async (input: Request | string | URL) => {
      const url = new URL(input instanceof Request ? input.url : String(input), "http://localhost");
      return url.pathname === "/v1/me" ? json(500, { error: { code: "internal", message: "x" } }) : answer(input);
    });
    wrap(<ManageGate>Manage page</ManageGate>);
    await waitFor(() => expect(nav.replace).toHaveBeenCalledWith("/"));
    expect(screen.queryByText("Manage page")).toBeNull();
  });

  it("sends a member with no workspace there to /", async () => {
    gate("/o/acme/people", { org: "acme" }, { orgRole: "member", mine: [] });
    await waitFor(() => expect(nav.replace).toHaveBeenCalledWith("/"));
  });

  it.each([
    ["an owner", { orgRole: "owner" as const }],
    ["an admin", { orgRole: "admin" as const }],
    ["an instance admin who is a member", { orgRole: "member" as const, instanceAdmin: true }],
  ])("lets %s in", async (_who, over) => {
    gate("/o/acme/people", { org: "acme" }, over);
    expect(await screen.findByText("Manage page")).toBeInTheDocument();
    expect(nav.replace).not.toHaveBeenCalled();
  });

  it("leaves workspace pages alone", async () => {
    gate("/o/acme/w/ops2", { org: "acme", ws: "ops2" }, { orgRole: "member" });
    expect(screen.getByText("Manage page")).toBeInTheDocument();
    await new Promise((r) => setTimeout(r, 40));
    expect(nav.replace).not.toHaveBeenCalled();
  });

  it("a member's rail lists no Manage entry while they are on the way out", async () => {
    shell("/o/acme/people", { org: "acme" }, { orgRole: "member" });
    const menu = await within(sidebar()).findByRole("navigation", { name: "Manage Acme" });
    expect(within(menu).queryAllByRole("link")).toHaveLength(0);
  });

  it("/ never lands a member on Manage: one of their workspaces, or nothing", () => {
    const args = { isInstanceAdmin: false, placementSaved: undefined, welcomeSeen: true };
    const member = (workspaces: { slug: string; personal?: boolean }[]) => ({ memberships: [{ org: { slug: "acme" }, role: "member", status: "active", workspaces }] });
    expect(homeTarget({ ...args, landing: "/o/acme", me: member([]) })).toBeNull();
    expect(homeTarget({ ...args, landing: "/o/acme", me: member([{ slug: "lab" }]) })).toBe("/o/acme/w/lab");
    expect(homeTarget({ ...args, landing: "/o/acme", me: { memberships: [{ org: { slug: "acme" }, role: "owner", status: "active", workspaces: [] }] } })).toBe("/o/acme");
  });
});

describe("Manage › Overview (nav-3b)", () => {
  it("is titled with the organization, with summary cards that link and what needs the owner", async () => {
    world = owner;
    serve();
    wrap(<OrgOverview slug="acme" />);
    expect(await screen.findByRole("heading", { level: 1, name: "Acme" })).toBeInTheDocument();
    expect(screen.getByText("Everything Acme's workspaces share")).toBeInTheDocument();
    const cards = within(screen.getByRole("list", { name: "Summary" }));
    for (const [name, href] of [
      ["Workspaces 3", "/o/acme/workspaces"], ["People 2", "/o/acme/people"], ["Agents 3", "/o/acme/agents"], ["AI keys 1", "/o/acme/apps?tab=accounts&kind=ai-key"],
    ]) {
      expect(await cards.findByRole("link", { name })).toHaveAttribute("href", href);
    }
    const needs = await screen.findByTestId("org-needs-you");
    expect(within(needs).getByLabelText("1 waiting")).toBeInTheDocument();
    expect(within(needs).getByRole("link", { name: item.title })).toHaveAttribute("href", item.href);
    expect(needs).toHaveTextContent("Apps & accounts · Workspaces that use it stop until it is renewed.");
    // The list of workspaces moved to its own page.
    expect(screen.queryByRole("link", { name: /Ops/ })).toBeNull();
  });
});

describe("Manage › Workspaces (D-508(c))", () => {
  it("lists every workspace: kind, owners, agents, and how to open it", async () => {
    world = owner;
    serve();
    wrap(<OrgWorkspaces slug="acme" />);
    expect(await screen.findByRole("heading", { level: 1, name: "Workspaces in Acme" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New workspace" })).toBeInTheDocument();
    const table = await screen.findByRole("table", { name: "Workspaces" });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(3);
    // Team first, then personal ones by their owner's name; managed ones say so and open the manage page.
    expect(within(rows[0]!).getByRole("link", { name: "Ops Manage" })).toHaveAttribute("href", "/o/acme/workspaces/ops");
    await waitFor(() => expect(rows[0]!).toHaveTextContent("Grace Hopper +2"));
    await waitFor(() => expect(within(rows[0]!).getAllByRole("cell")[3]).toHaveTextContent("2"));
    expect(within(rows[0]!).getAllByRole("cell")[1]).toHaveTextContent("Team");
    expect(within(rows[1]!).getByRole("link", { name: "Your workspace" })).toHaveAttribute("href", "/o/acme/w/alice");
    expect(within(rows[1]!).getAllByRole("cell")[1]).toHaveTextContent("Personal");
    expect(within(rows[1]!).getAllByRole("cell")[2]).toHaveTextContent("Alice Admin");
    expect(within(rows[2]!).getByRole("link", { name: "Bob's workspace Manage" })).toHaveAttribute("href", "/o/acme/workspaces/bob");
    expect(within(rows[2]!).getAllByRole("cell")[3]).toHaveTextContent("1");
  });
});
