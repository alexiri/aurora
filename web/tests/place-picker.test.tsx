import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AppShell } from "@/components/app-shell";
import { buildPlaces, orgTarget, placeCount } from "@/lib/nav/places";
import { wrap } from "./support/render";
import { json } from "./support/api";

// P5-NAV-3 (D-508 (f), (g), (k); docs/design/navigation nav-3a, nav-3d): the place picker for three people, Manage at
// the rail's foot, and the phone's top bar with the picker as a sheet. Only places a person can enter are listed.

const nav = vi.hoisted(() => ({ pathname: "/o/acme/w/mine", params: {} as Record<string, string | undefined> }));
vi.mock("next/navigation", () => ({
  useParams: () => nav.params,
  usePathname: () => nav.pathname,
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
}));

const now = "2026-11-10T10:00:00Z";
const ME = "u1";

interface Mine {
  slug: string;
  name: string;
  role: string;
  personal?: boolean;
}
interface OrgFixture {
  id: string;
  slug: string;
  name: string;
  role: "owner" | "admin" | "member";
  mine: Mine[];
  /** Workspaces of the organization this person is not in: an administrator's list shows them. */
  others?: number;
  /** `/v1/me`'s answer to "may this person create a workspace here". */
  canCreateWorkspaces?: boolean;
}
interface World {
  instanceAdmin: boolean;
  orgCreation: "anyone" | "instance_admins";
  orgs: OrgFixture[];
  /** The slug of a workspace this person reads through an open emergency grant. */
  reading?: string;
}
let world: World;

const row = (o: OrgFixture, slug: string, name: string, my_role: string | null, over: Record<string, unknown> = {}) => ({
  id: `${o.slug}-${slug}`, org_id: o.id, slug, name, my_role, administers: my_role == null && o.role !== "member",
  memory_language: "en", timezone: "UTC", created_at: now, updated_at: now, personal_of: null, emergency_reader: false, ...over,
});
const listOf = (o: OrgFixture) => [
  ...o.mine.map((w) => row(o, w.slug, w.name, w.role, w.personal ? { personal_of: { id: ME, name: "Alice Admin" } } : {})),
  ...Array.from({ length: o.others ?? 0 }, (_, i) => row(o, `hidden-${i}`, `Secret ${i}`, null)),
];
const orgBody = (o: OrgFixture) => ({ id: o.id, slug: o.slug, name: o.name, my_role: o.role, app_posture: "open", external_id: "x", settings: {}, created_at: now });

function serve() {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const p = url.pathname;
      if (p === "/v1/me") {
        return json(200, {
          user: { id: ME, email: "a@example.test", display_name: "Alice Admin", is_instance_admin: world.instanceAdmin },
          default_org_id: null,
          memberships: world.orgs.map((o) => ({
            org: { id: o.id, slug: o.slug, name: o.name },
            role: o.role,
            status: "active",
            ...(o.canCreateWorkspaces === undefined ? {} : { can_create_workspaces: o.canCreateWorkspaces }),
            workspaces: o.mine.map((w) => ({ id: `${o.slug}-${w.slug}`, slug: w.slug, name: w.name, my_role: w.role, personal: w.personal === true })),
          })),
        });
      }
      if (p === "/v1/instance") return json(200, { org_creation: world.orgCreation, features: {} });
      if (p === "/v1/resolve") {
        const o = world.orgs.find((x) => x.slug === url.searchParams.get("org")) ?? world.orgs[0]!;
        const ws = url.searchParams.get("workspace");
        const reading = ws && ws === world.reading ? row(o, ws, "Under review", null, { emergency_reader: true }) : undefined;
        return json(200, { org: orgBody(o), workspace: ws ? (reading ?? listOf(o).find((w) => w.slug === ws) ?? null) : null });
      }
      if (p === "/v1/attention/counts") {
        const o = world.orgs.find((x) => x.id === url.searchParams.get("org"));
        return json(200, {
          administration: world.instanceAdmin ? { count: 1, topics: {} } : null,
          organization: o && o.role !== "member" ? { org_id: o.id, count: 2, topics: {} } : null,
          workspace: null,
        });
      }
      for (const o of world.orgs) {
        if (p === `/v1/orgs/${o.id}`) return json(200, orgBody(o));
        if (p === `/v1/orgs/${o.id}/workspaces`) return json(200, { items: listOf(o), next_cursor: null });
      }
      const one = p.match(/^\/v1\/workspaces\/([^/]+)$/);
      if (one) {
        for (const o of world.orgs) {
          if (world.reading && one[1] === `${o.slug}-${world.reading}`) return json(200, row(o, world.reading, "Under review", null, { emergency_reader: true }));
          const w = listOf(o).find((x) => x.id === one[1]);
          if (w) return json(200, w);
        }
      }
      return json(200, { items: [], next_cursor: null });
    }),
  );
}

const acme = (over: Partial<OrgFixture> = {}): OrgFixture => ({
  id: "o1", slug: "acme", name: "Acme", role: "member", mine: [{ slug: "mine", name: "Bob Builder", role: "owner", personal: true }], ...over,
});

const bob = (): World => ({ instanceAdmin: false, orgCreation: "anyone", orgs: [acme()] });
const alice = (): World => ({
  instanceAdmin: true,
  orgCreation: "instance_admins",
  orgs: [
    acme({
      role: "owner",
      mine: [
        { slug: "mine", name: "Alice Admin", role: "owner", personal: true },
        { slug: "default", name: "Default", role: "owner" },
        { slug: "platform", name: "Platform team", role: "member" },
      ],
      others: 3,
    }),
    { id: "o2", slug: "globex", name: "Globex", role: "member", mine: [{ slug: "main", name: "Main", role: "member" }] },
  ],
});
const carol = (): World => ({
  instanceAdmin: false,
  orgCreation: "instance_admins",
  orgs: [
    acme({
      mine: [
        { slug: "platform", name: "Platform team", role: "member" },
        ...Array.from({ length: 8 }, (_, i) => ({ slug: `team-${i}`, name: `Team ${i}`, role: "member" })),
      ],
    }),
    { id: "o2", slug: "globex", name: "Globex", role: "member", mine: [{ slug: "plat", name: "Platform", role: "member" }] },
    { id: "o3", slug: "initech", name: "Initech", role: "admin", mine: [] },
  ],
});

function shell(w: World, pathname = "/o/acme/w/mine", params: Record<string, string> = { org: "acme", ws: "mine" }) {
  world = w;
  nav.pathname = pathname;
  nav.params = params;
  serve();
  return wrap(<AppShell>Page</AppShell>);
}

const sidebar = () => screen.getByRole("complementary", { name: "Sidebar" });
const card = () => within(sidebar()).getByTestId("scope-switcher");
async function open(): Promise<HTMLElement> {
  fireEvent.click(await within(card()).findByRole("button"));
  return screen.findByRole("dialog", { name: "Switch workspace or organization" });
}
const headings = (d: HTMLElement) =>
  within(d)
    .getAllByRole("group")
    .map((g) => document.getElementById(g.getAttribute("aria-labelledby") ?? "")?.textContent?.trim() ?? "");

beforeEach(() => {
  window.localStorage.clear();
  window.sessionStorage.clear();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the picker for Bob (nav-3a): only what he can use", () => {
  it("lists his workspace and New organization: no search, no Manage, no Administration, no other organizations", async () => {
    shell(bob());
    const d = await open();
    expect(await within(d).findByRole("link", { name: /Your workspace Owner · Acme/ })).toHaveAttribute("aria-current", "true");
    expect(headings(d)).toEqual(["Workspaces in Acme"]);
    expect(within(d).getByRole("button", { name: "New organization" })).toBeInTheDocument();
    expect(within(d).queryByRole("combobox")).toBeNull();
    expect(within(d).queryByRole("link", { name: /Manage|Administration/ })).toBeNull();
    expect(within(d).queryByText("Other organizations")).toBeNull();
    expect(within(sidebar()).queryByRole("link", { name: /^Manage/ })).toBeNull();
  });

  it("a member who may create workspaces is offered New workspace, though they can't open Manage", async () => {
    shell({ ...bob(), orgCreation: "instance_admins", orgs: [acme({ canCreateWorkspaces: true })] });
    const d = await open();
    expect(await within(d).findByRole("button", { name: "New workspace in Acme" })).toBeInTheDocument();
    expect(within(d).queryByRole("link", { name: /Manage/ })).toBeNull();
    expect(within(d).queryByRole("button", { name: "New organization" })).toBeNull();
  });

  it("a member who may not create workspaces isn't", async () => {
    shell({ ...bob(), orgs: [acme({ canCreateWorkspaces: false })] });
    const d = await open();
    await within(d).findByRole("button", { name: "New organization" });
    expect(within(d).queryByRole("button", { name: /New workspace/ })).toBeNull();
  });

  it("with nothing else to open and nothing to make, the card has no menu", async () => {
    shell({ ...bob(), orgCreation: "instance_admins" });
    await waitFor(() => expect(card()).toHaveTextContent("Your workspace Acme"));
    await new Promise((r) => setTimeout(r, 60));
    expect(within(card()).queryByRole("button")).toBeNull();
  });

  it("focus goes to the first row, arrows move, and Escape returns it to the card", async () => {
    shell(bob());
    const d = await open();
    const first = await within(d).findByRole("link", { name: /Your workspace/ });
    await waitFor(() => expect(document.activeElement).toBe(first));
    fireEvent.keyDown(first, { key: "ArrowDown" });
    expect(document.activeElement).toBe(within(d).getByRole("button", { name: "New organization" }));
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Switch workspace or organization" })).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(within(card()).getByRole("button")));
  });
});

describe("the picker for Alice (nav-3a): every place she can go, labelled by kind", () => {
  it("search, her workspaces with their role, All n, Manage, Administration, the other organization and New", async () => {
    shell(alice());
    const d = await open();
    await within(d).findByRole("link", { name: /All 6 workspaces/ });
    expect(within(d).getByRole("combobox")).toHaveAttribute("placeholder", "Find a workspace or organization");
    expect(headings(d)).toEqual(["Workspaces in Acme", "Organization", "Administration", "Other organizations"]);
    const ws = within(d).getByRole("group", { name: "Workspaces in Acme" });
    expect(within(ws).getAllByRole("link").map((a) => a.textContent?.replace(/\s+/g, " ").trim())).toEqual([
      "A Your workspace Owner · Acme", "D Default Owner · Acme", "P Platform team Member · Acme", "All 6 workspaces…",
    ]);
    // Her own rows only: the three she administers but is not in are not places in the picker.
    expect(within(d).queryByText(/Secret/)).toBeNull();
    expect(within(d).getByRole("link", { name: /All 6 workspaces/ })).toHaveAttribute("href", "/o/acme/workspaces");
    const manage = within(d).getByRole("link", { name: /Manage Acme/ });
    expect(manage).toHaveAttribute("href", "/o/acme");
    expect(manage).toHaveTextContent("People, apps, AI models, data");
    expect(manage).toHaveTextContent("Owner");
    await waitFor(() => expect(within(manage).getByTestId("manage-switcher-badge")).toHaveTextContent("2"));
    const admin = within(d).getByRole("link", { name: /^Administration/ });
    expect(admin).toHaveAttribute("href", "/admin");
    expect(admin).toHaveTextContent("Settings for everyone on this Aurora");
    await waitFor(() => expect(within(admin).getByTestId("picker-admin-badge")).toHaveTextContent("1"));
    const globex = within(d).getByRole("link", { name: /Globex/ });
    expect(globex).toHaveTextContent("Member · opens your last workspace there");
    expect(globex).toHaveAttribute("href", "/o/globex/w/main");
    expect(within(d).getByRole("button", { name: "New workspace in Acme" })).toBeInTheDocument();
    expect(within(d).getByRole("button", { name: "New organization" })).toBeInTheDocument();
  });

  it("the rail's foot has Manage Acme with its count above Administration; inside Manage the menu is it", async () => {
    shell(alice());
    const manage = await within(sidebar()).findByRole("link", { name: /^Manage Acme/ });
    expect(manage).toHaveAttribute("href", "/o/acme");
    await waitFor(() => expect(within(manage).getByTestId("rail-manage-badge")).toHaveTextContent("2"));
    const admin = within(sidebar()).getByRole("link", { name: /^Administration/ });
    expect(manage.compareDocumentPosition(admin) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // The card carries no badge of its own: the foot does.
    expect(within(card()).queryByTestId("switcher-attention-badge")).toBeNull();
    cleanup();
    shell(alice(), "/o/acme/people", { org: "acme" });
    await within(sidebar()).findByRole("navigation", { name: "Manage Acme" });
    expect(within(sidebar()).queryByTestId("rail-manage-badge")).toBeNull();
  });

  it("opens from Administration's card too, about the organization she came from, with Administration current", async () => {
    shell(alice(), "/admin", {});
    const d = await open();
    expect(await within(d).findByRole("link", { name: /^Administration/ })).toHaveAttribute("aria-current", "true");
    expect(within(d).getByRole("group", { name: "Workspaces in Acme" })).toBeInTheDocument();
    expect(within(sidebar()).getByRole("link", { name: /^Manage Acme/ })).toBeInTheDocument();
  });

  it("opens from Your account's card too", async () => {
    shell(alice(), "/account", {});
    const d = await open();
    expect(await within(d).findByRole("link", { name: /Manage Acme/ })).toBeInTheDocument();
  });
});

describe("many places: search first (nav-3a)", () => {
  it("finds across organizations, grouped by organization, and the keys open the highlighted one", async () => {
    shell(carol(), "/o/acme/w/platform", { org: "acme", ws: "platform" });
    const d = await open();
    const field = within(d).getByRole("combobox");
    await waitFor(() => expect(document.activeElement).toBe(field));
    fireEvent.change(field, { target: { value: "plat" } });
    const options = await within(d).findAllByRole("option");
    expect(options.map((o) => o.getAttribute("href"))).toEqual(["/o/acme/w/platform", "/o/globex/w/plat"]);
    expect(headings(d)).toEqual(["Acme", "Globex"]);
    expect(options[0]).toHaveTextContent("Platform team Workspace · Member");
    expect(options[0]).toHaveAttribute("aria-selected", "true");
    expect(field).toHaveAttribute("aria-activedescendant", options[0]!.id);
    expect(d).toHaveTextContent("10 workspaces, 3 organizations");
    expect(d).toHaveTextContent("↑↓ to move · ⏎ to open · Esc to close");
    const clicked: string[] = [];
    d.addEventListener("click", (e) => {
      const a = (e.target as HTMLElement).closest("a");
      if (a) clicked.push(a.getAttribute("href")!);
      e.preventDefault();
    });
    fireEvent.keyDown(field, { key: "ArrowDown" });
    expect(within(d).getAllByRole("option")[1]).toHaveAttribute("aria-selected", "true");
    expect(document.activeElement).toBe(field);
    fireEvent.keyDown(field, { key: "Enter" });
    expect(clicked).toEqual(["/o/globex/w/plat"]);
  });

  it("an organization's name finds its workspaces too", async () => {
    shell(carol(), "/o/acme/w/platform", { org: "acme", ws: "platform" });
    const d = await open();
    fireEvent.change(within(d).getByRole("combobox"), { target: { value: "globex" } });
    await waitFor(() => expect(within(d).getAllByRole("option").map((o) => o.getAttribute("href"))).toContain("/o/globex/w/plat"));
    expect(within(d).getByRole("group", { name: "Globex" })).toHaveTextContent("Platform Workspace · Member");
    expect(headings(d)).toEqual(["Globex"]);
  });

  it("finds Manage only where she may open it, and no Administration without being an instance admin", async () => {
    shell(carol(), "/o/acme/w/platform", { org: "acme", ws: "platform" });
    const d = await open();
    const field = within(d).getByRole("combobox");
    fireEvent.change(field, { target: { value: "manage" } });
    expect((await within(d).findAllByRole("option")).map((o) => o.getAttribute("href"))).toEqual(["/o/initech"]);
    fireEvent.change(field, { target: { value: "administration" } });
    expect(await within(d).findByText(/Nothing matches/)).toBeInTheDocument();
  });

  it("a member's All n opens the whole list in the picker; another organization with no workspace opens its Manage", async () => {
    shell(carol(), "/o/acme/w/platform", { org: "acme", ws: "platform" });
    const d = await open();
    const ws = within(d).getByRole("group", { name: "Workspaces in Acme" });
    await waitFor(() => expect(within(ws).getAllByRole("link")).toHaveLength(5));
    fireEvent.click(within(ws).getByRole("button", { name: "All 9 workspaces…" }));
    expect(within(ws).getAllByRole("link")).toHaveLength(9);
    const initech = within(d).getByRole("link", { name: /Initech/ });
    expect(initech).toHaveTextContent("Admin · opens Manage Initech");
    expect(initech).toHaveAttribute("href", "/o/initech");
  });
});

describe("an emergency reader (FIX-277, D-478)", () => {
  const reader = (open_: boolean): World => ({
    instanceAdmin: false, orgCreation: "instance_admins",
    orgs: [acme({ role: "owner", others: 1 })],
    reading: open_ ? "review" : undefined,
  });

  it("the workspace read under their own open grant is a place, labelled so", async () => {
    shell(reader(true), "/o/acme/w/review", { org: "acme", ws: "review" });
    const d = await open();
    const mine = await within(d).findByRole("link", { name: /Under review Emergency read access · Acme/ });
    expect(mine).toHaveAttribute("aria-current", "true");
  });

  it("and not once the grant is over: the workspace answer alone decides", async () => {
    shell(reader(false), "/o/acme/w/review", { org: "acme", ws: "review" });
    const d = await open();
    await within(d).findByRole("link", { name: /Your workspace/ });
    await new Promise((r) => setTimeout(r, 60));
    expect(within(d).queryByRole("link", { name: /Under review|Emergency/ })).toBeNull();
  });
});

describe("the phone's top bar (nav-3d)", () => {
  const bar = () => screen.getByTestId("topbar-place");

  it("shows the place card in short and opens the same picker as a sheet, with no native select", async () => {
    shell(alice(), "/o/acme/people", { org: "acme" });
    await waitFor(() => expect(bar()).toHaveTextContent("Acme · Organization"));
    fireEvent.click(within(bar()).getByRole("button"));
    const sheet = await screen.findByRole("dialog", { name: "Switch workspace or organization" });
    expect(await within(sheet).findByRole("link", { name: /Manage Acme/ })).toHaveAttribute("aria-current", "true");
    expect(within(sheet).getByRole("combobox").tagName).toBe("INPUT");
    expect(sheet.querySelector("select")).toBeNull();
    fireEvent.click(within(sheet).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Switch workspace or organization" })).toBeNull());
  });

  it("is plain text for a person with nowhere else to go", async () => {
    shell({ ...bob(), orgCreation: "instance_admins" });
    await waitFor(() => expect(bar()).toHaveTextContent("Your workspace · Acme"));
    await new Promise((r) => setTimeout(r, 60));
    expect(within(bar()).queryByRole("button")).toBeNull();
  });
});

describe("places come only from what the person may read", () => {
  const memberships = [
    { org: { id: "o1", slug: "acme", name: "Acme" }, role: "owner", status: "active", workspaces: [{ slug: "a", name: "A", my_role: "owner" }] },
    { org: { id: "o2", slug: "gone", name: "Gone" }, role: "admin", status: "suspended", workspaces: [] },
  ];
  const base = { viewer: { id: ME, display_name: "Alice" }, instanceAdmin: false, memberships, recent: [] };

  it("an administrator's list of an organization adds none of the workspaces they are not in", () => {
    const [a] = buildPlaces({
      ...base,
      open: { id: "o1", slug: "acme", name: "Acme" },
      openList: [{ slug: "a", name: "A", my_role: "owner" }, { slug: "x", name: "X", my_role: null }],
      reading: { slug: "x", name: "X", my_role: null, emergency_reader: false },
    });
    expect(a!.workspaces.map((w) => w.slug)).toEqual(["a"]);
    expect(a!.total).toBe(2);
  });

  it("a suspended membership offers no Manage, and an organization with nothing to open has no target", () => {
    const gone = buildPlaces(base)[1]!;
    expect(gone.manage).toBe(false);
    expect(orgTarget(gone, ME, memberships)).toBeUndefined();
    expect(placeCount(buildPlaces(base), false)).toBe(2);
  });
});
