import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AccessControl } from "@/components/accounts/access-control";
import { startingDecisions } from "@/components/accounts/actions-table";
import { AccountsList } from "@/components/accounts/accounts-list";
import { GiveAccessPanel } from "@/components/credentials/give-access";
import { agentLabel, entryText, preselectAgents, splitGiven } from "@/lib/accounts";
import type { Account, AgentAccess } from "@/lib/api/accounts";
import { wrap } from "./support/render";
import { json } from "./support/api";

// (P5-UX-4, C-917, C-925, C-938) The one list of accounts, the access choice and the actions table.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops" }),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
  usePathname: () => "/o/acme/w/ops/apps",
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

type Handler = (req: Request, body: unknown) => Response | Promise<Response>;
interface Call { key: string; body: unknown }
let calls: Call[];
let routes: Record<string, Handler>;
const sent = (key: string) => calls.filter((c) => c.key === key);

beforeEach(() => {
  calls = [];
  routes = {};
  window.history.replaceState(null, "", "/o/acme/w/ops/apps");
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      const body = text ? JSON.parse(text) : undefined;
      const key = `${req.method} ${url.pathname}`;
      calls.push({ key, body });
      const h = routes[key];
      return h ? h(req, body) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
});

const DANEEL = { id: "a1", name: "Daneel", handle: "daneel" };
const GISKARD = { id: "a2", name: "Giskard", handle: "giskard" };
const OPS = { id: "w1", name: "Ops" };
const entry = (over: Partial<AgentAccess> = {}): AgentAccess => ({ agent: DANEEL, workspace: OPS, level: "read", ...over }) as AgentAccess;
const OWNER = { give_access: true, change_access: true, manage: true };
const MEMBER = { give_access: false, change_access: false, manage: false };

const app = (over: Record<string, unknown> = {}): Account =>
  ({
    ref: { type: "connection", id: "c1" }, kind: "app", name: "AWS prod", scope: "workspace", levels: ["read", "ask", "act"],
    app: { app_id: "aws", name: "AWS", custom_only: false }, health: { state: "not_used", last_used_at: null }, agents: [entry()], can: OWNER, ...over,
  }) as Account;
const website = (over: Record<string, unknown> = {}): Account =>
  ({
    ref: { type: "secret", id: "s1" }, kind: "website", name: "Stripe API", scope: "workspace", levels: [], health: { state: "not_used" },
    agents: [entry({ level: "use" })], can: OWNER, ...over,
  }) as Account;
const aiKey = (over: Record<string, unknown> = {}): Account =>
  ({
    ref: { type: "secret", id: "s2" }, kind: "ai_key", name: "Team key", scope: "workspace", levels: [], health: { state: "not_used" },
    agents: [entry({ level: "use" })], can: OWNER, ...over,
  }) as Account;

const listWs = (items: Account[]) => {
  routes["GET /v1/workspaces/w1/accounts"] = () => json(200, { items, next_cursor: null });
  routes["GET /v1/workspaces/w1/agents"] = () => json(200, { items: [], next_cursor: null });
  routes["GET /v1/apps"] = () => json(200, { items: [], next_cursor: null });
  routes["GET /v1/agents/a1/grants"] = () => json(200, { secrets: [], connections: [], converted_by_app: [] });
};
const list = (props: Partial<Parameters<typeof AccountsList>[0]> = {}) =>
  wrap(<AccountsList scope={{ kind: "workspace", id: "w1" }} orgId="o1" canOwn {...props} />);
const openMenu = async (name: string) => fireEvent.keyDown(await screen.findByRole("button", { name: `More for ${name}` }), { key: "Enter" });

describe("AccountsList: one row per kind", () => {
  it("shows the kind as its icon (no chip for an app or an AI key), who can use it and the menu for each kind, for an owner", async () => {
    listWs([app(), website(), aiKey()]);
    list();
    const aws = await screen.findByRole("listitem", { name: "AWS prod" });
    expect(within(aws).queryByText("App")).toBeNull();
    expect(within(aws).getByRole("button", { name: "Daneel · Read only" })).toBeInTheDocument();
    expect(within(screen.getByRole("listitem", { name: "Stripe API" })).getByText("Website")).toBeInTheDocument();
    const key = screen.getByRole("listitem", { name: "Team key" });
    expect(within(key).queryByText("AI key")).toBeNull();
    expect(within(key).getByText("AI")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^More for / })).toHaveLength(3);
  });

  it("an organization's starter key says new agents get it automatically; other keys do not (P5-APIGAP-3, C-1049)", async () => {
    routes["GET /v1/orgs/o1/accounts"] = () =>
      json(200, { items: [aiKey({ name: "Starter key", scope: "organization", starter: true }), aiKey({ ref: { type: "secret", id: "s3" }, name: "Other key", scope: "organization", starter: false })], next_cursor: null });
    wrap(<AccountsList scope={{ kind: "organization", id: "o1" }} orgId="o1" canOwn />);
    const starter = await screen.findByRole("listitem", { name: "Starter key" });
    expect(within(starter).getByText("New agents get this key automatically.")).toBeInTheDocument();
    expect(screen.getAllByText("New agents get this key automatically.")).toHaveLength(1);
  });

  it("offers Give access, Rename, Reconnect, Turn off and Remove for an app; Replace and Test for a website, never a rule or host", async () => {
    listWs([app(), website()]);
    // (P5-REFINE-13, access-11) The app offers a test where a method has a probe and Sign in again where one is OAuth.
    routes["GET /v1/apps/aws"] = () => json(200, { app_id: "aws", name: "AWS", connect: [{ id: "keys", kind: "fields", has_probe: true }, { id: "oauth", kind: "oauth2", has_probe: false }], tools: [] });
    list();
    await openMenu("AWS prod");
    await screen.findByRole("menuitem", { name: "Sign in again" });
    const names = (await screen.findAllByRole("menuitem")).map((m) => m.textContent);
    expect(names).toEqual(["Give access", "Test the connection", "Sign in again", "Rename", "Turn off", "Remove"]);
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    cleanup();
    listWs([website()]);
    list();
    await openMenu("Stripe API");
    expect((await screen.findAllByRole("menuitem")).map((m) => m.textContent)).toEqual(["Give access", "Test the connection", "Change website", "Replace", "Rename", "Remove"]);
  });

  it("a calendar link's menu has no test and no sign-in again; the Outgoing webhook's has Send a test (access-11)", async () => {
    listWs([
      app({ ref: { type: "connection", id: "c5" }, name: "Team calendar", levels: [], app: { app_id: "ics", name: "Calendar link", custom_only: false }, agents: [entry({ level: "use" })] }),
      app({ ref: { type: "connection", id: "c6" }, name: "Ops alerts", levels: ["ask", "act"], app: { app_id: "webhook", name: "Outgoing webhook", custom_only: false, send_only: true }, agents: [entry({ level: "ask" })] }),
    ]);
    routes["GET /v1/apps/ics"] = () => json(200, { app_id: "ics", name: "Calendar link", connect: [{ id: "url", kind: "fields", has_probe: false }], tools: [] });
    routes["GET /v1/apps/webhook"] = () => json(200, { app_id: "webhook", name: "Outgoing webhook", connect: [{ id: "url", kind: "fields", has_probe: false }], tools: [] });
    list();
    await openMenu("Team calendar");
    await waitFor(() => expect(sent("GET /v1/apps/ics").length).toBeGreaterThan(0));
    const cal = (await screen.findAllByRole("menuitem")).map((m) => m.textContent);
    expect(cal).not.toContain("Test the connection");
    expect(cal).not.toContain("Sign in again");
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    await openMenu("Ops alerts");
    expect(await screen.findByRole("menuitem", { name: "Send a test" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Test the connection" })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: "Sign in again" })).toBeNull();
  });

  it("a key that works has one status line, not 'Worked' and 'Works'; one being checked still says Checking… (access-15)", async () => {
    const key = (name: string, id: string, state: string) =>
      ({
        ref: { type: "secret", id }, kind: "ai_key", name, scope: "workspace", levels: [], health: { state: "working", last_ok_at: new Date().toISOString() },
        verification: { state }, agents: [], can: OWNER,
      }) as unknown as Account;
    listWs([key("Works key", "k1", "works"), key("New key", "k2", "checking")]);
    list();
    const works = await screen.findByRole("listitem", { name: "Works key" });
    expect(within(works).getByText(/^Worked/)).toBeInTheDocument();
    expect(within(works).queryByText("Works")).toBeNull();
    expect(within(screen.getByRole("listitem", { name: "New key" })).getByText("Checking…")).toBeInTheDocument();
  });

  it("every app row has the level pill, even with one level; a send-only app's levels are worded for sends (access-12, D-419)", async () => {
    listWs([
      app({ ref: { type: "connection", id: "c5" }, name: "Team calendar", levels: [], app: { app_id: "ics", name: "Calendar link", custom_only: false }, agents: [entry({ level: "use" })] }),
      app({ ref: { type: "connection", id: "c6" }, name: "Ops alerts", levels: ["ask", "act"], app: { app_id: "webhook", name: "Outgoing webhook", custom_only: false, send_only: true }, agents: [entry({ level: "ask" })] }),
    ]);
    // On an agent's list (its Access tab) every app row carries the agent's level.
    list({ agent: { id: "a1", name: "Daneel" } });
    const cal = await screen.findByRole("listitem", { name: "Team calendar" });
    expect(within(cal).getByRole("combobox", { name: "Daneel's access to Team calendar" })).toHaveDisplayValue("Read only");
    const ops = screen.getByRole("listitem", { name: "Ops alerts" });
    const select = within(ops).getByRole("combobox", { name: "Daneel's access to Ops alerts" });
    const options = within(select).getAllByRole("option").map((o) => o.textContent);
    expect(options).toContain("Ask before each send");
    expect(options).toContain("Send without asking");
    expect(options).not.toContain("Read only");
  });

  it("Change website edits the website, the path and the way the token is sent in one save, from the website's own record", async () => {
    listWs([website()]);
    routes["GET /v1/workspaces/w1/secrets"] = () =>
      json(200, { items: [{ id: "s1", kind: "generic", name: "Stripe API", host_pattern: "api.stripe.com", path_pattern: null, injection: { kind: "bearer" } }], next_cursor: null });
    routes["PATCH /v1/secrets/s1"] = () => json(200, {});
    list();
    await openMenu("Stripe API");
    fireEvent.click(await screen.findByRole("menuitem", { name: "Change website" }));
    const site = await screen.findByLabelText("Website");
    expect(site).toHaveValue("api.stripe.com");
    fireEvent.change(site, { target: { value: "api.stripe.example" } });
    fireEvent.click(screen.getByText(/^Advanced: path/));
    fireEvent.change(screen.getByLabelText("Path (optional)"), { target: { value: "/v1/*" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(sent("PATCH /v1/secrets/s1")).toHaveLength(1));
    expect(sent("PATCH /v1/secrets/s1")[0]!.body).toEqual({ host_pattern: "api.stripe.example", path_pattern: "/v1/*", injection: { kind: "bearer" } });
    // The name is Rename's: it is not part of this save.
    expect(sent("PATCH /v1/secrets/s1")[0]!.body).not.toHaveProperty("name");
  });

  it("says Turn on for a turned-off account and the health line says why an account stopped", async () => {
    listWs([app({ health: { state: "disabled" } }), app({ ref: { type: "connection", id: "c2" }, name: "Jira", health: { state: "sign_in_expired" } })]);
    list();
    expect(await screen.findByText("Turned off")).toBeInTheDocument();
    expect(screen.getByText("Sign-in expired")).toBeInTheDocument();
    await openMenu("AWS prod");
    expect(await screen.findByRole("menuitem", { name: "Turn on" })).toBeInTheDocument();
  });

  it("FIX-163: an account whose app instance was deleted is not offered Turn on; the row says why and who can add the instance again", async () => {
    routes["GET /v1/orgs/o1"] = () => json(200, { id: "o1", slug: "acme", name: "Acme", contacts: { admins: [{ name: "Ada Lovelace" }], owners: [] } });
    listWs([
      app({ health: { state: "disabled" }, status_reason: "instance_deleted" }),
      app({ ref: { type: "connection", id: "c2" }, name: "Jira", health: { state: "disabled" } }),
    ]);
    list();
    const gone = await screen.findByRole("listitem", { name: "AWS prod" });
    expect(within(gone).getByText("Turned off: the instance of this app was removed")).toBeInTheDocument();
    expect(await within(gone).findByText(/Only organization owners and admins can add the app's instance again\./)).toBeInTheDocument();
    await openMenu("AWS prod");
    expect(await screen.findByRole("menuitem", { name: "Rename" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Turn on" })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: "Turn off" })).toBeNull();
    // Any other reason keeps today's behaviour: a plainly turned-off account is turned on from the menu.
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    await openMenu("Jira");
    expect(await screen.findByRole("menuitem", { name: "Turn on" })).toBeInTheDocument();
  });

  it("marks an account nobody can use, and an organization's account with who manages it", async () => {
    listWs([
      website({ agents: [] }),
      app({ ref: { type: "connection", id: "c9" }, name: "Org GitLab", scope: "organization", can: { ...OWNER, manage: false }, managed_by: { level: "org_admin", people: [], more: 0, unreachable: false } }),
    ]);
    list();
    const stripe = await screen.findByRole("listitem", { name: "Stripe API" });
    expect(within(stripe).getByText("No agent can use this")).toBeInTheDocument();
    const gitlab = screen.getByRole("listitem", { name: "Org GitLab" });
    expect(within(gitlab).getByText(/Shared by your organization/)).toBeInTheDocument();
    expect(within(gitlab).queryByRole("button", { name: /^More for / })).toBeNull();
  });

  it("shows a member the same values as text, with no controls and no Add", async () => {
    listWs([app({ can: MEMBER }), website({ can: MEMBER })]);
    list({ canOwn: false });
    const aws = await screen.findByRole("listitem", { name: "AWS prod" });
    expect(within(aws).getByText("Daneel · Read only")).toBeInTheDocument();
    expect(within(aws).queryByRole("button")).toBeNull();
    expect(screen.queryByRole("button", { name: "Add" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^More for / })).toBeNull();
  });

  it("Add offers Connect an app or website and Add an AI key; the key form opens in place", async () => {
    listWs([app()]);
    routes["GET /v1/llm/providers"] = () => json(200, { providers: [], endpoints: [] });
    routes["GET /v1/orgs/o1"] = () => json(200, { id: "o1", my_role: "owner", settings: {} });
    list();
    fireEvent.keyDown(await screen.findByRole("button", { name: "Add" }), { key: "Enter" });
    expect(await screen.findByRole("menuitem", { name: "Connect an app or website" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitem", { name: "Add an AI key" }));
    expect(await screen.findByRole("button", { name: "Close" })).toBeInTheDocument();
  });

  it("an organization's list adds an app or website for the whole organization or an AI key, with its own empty state (P5-UX-11)", async () => {
    routes["GET /v1/orgs/o1/accounts"] = () => json(200, { items: [], next_cursor: null });
    wrap(<AccountsList scope={{ kind: "organization", id: "o1" }} orgId="o1" canOwn />);
    expect(await screen.findByText("No accounts for the whole organization yet")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connect an app or website" })).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("button", { name: "Add" }), { key: "Enter" });
    expect((await screen.findAllByRole("menuitem")).map((m) => m.textContent)).toEqual(["Connect an app or website", "Add an AI key"]);
  });
});

describe("AccountsList: an agent's Access tab", () => {
  it("lists what the agent can use first, the rest collapsed, with the model line on an AI key and AccessControl in the row", async () => {
    listWs([
      app(),
      aiKey({ agents: [entry({ level: "use", model: "claude-sonnet" })] }),
      website({ ref: { type: "secret", id: "s3" }, name: "Wiki", given: false, agents: [] }),
    ].map((a, i) => (i < 2 ? { ...a, given: true } : a)) as Account[]);
    list({ agent: { id: "a1", name: "Daneel" } });
    const used = await screen.findByRole("list", { name: "Accounts Daneel can use" });
    expect(within(used).getAllByRole("listitem", { name: /AWS prod|Team key/ })).toHaveLength(2);
    expect(within(used).getByText(/Daneel thinks with claude-sonnet through this key/)).toBeInTheDocument();
    expect(within(used).getAllByRole("combobox").length).toBeGreaterThan(0);
    expect(screen.getByText("Not given to Daneel (1)")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Give Daneel an app" })).toBeInTheDocument();
  });
});

describe("AccessControl", () => {
  const put = () => {
    routes["PUT /v1/agents/a1/grants/connections/c1"] = () => json(200, {});
    routes["GET /v1/agents/a1/grants"] = () =>
      json(200, { secrets: [], connections: [{ connection_id: "c1", mode: "custom", allow_tools: ["merge"], ask_tools: ["push"], ask_timeout_secs: null }], converted_by_app: [] });
    routes["GET /v1/workspaces/w1/accounts"] = () => json(200, { items: [], next_cursor: null });
  };
  const control = (account: Account, e: AgentAccess, canChange = true) =>
    wrap(<AccessControl account={account} entry={e} workspaceId="w1" canChange={canChange} />);

  it("offers the three choices in order, the saved one chosen, and one click is one save", async () => {
    put();
    control(app(), entry({ level: "read" }));
    const select = await screen.findByRole("combobox");
    expect(within(select).getAllByRole("option").map((o) => o.textContent)).toEqual([
      "Read only",
      "Ask before changes",
      "Change without asking",
      "Custom…",
      "What exactly?",
    ]);
    expect(select).toHaveValue("read");
    fireEvent.change(select, { target: { value: "ask" } });
    await waitFor(() => expect(sent("PUT /v1/agents/a1/grants/connections/c1")).toHaveLength(1));
    expect(sent("PUT /v1/agents/a1/grants/connections/c1")[0].body).toEqual({ level: "ask" });
  });

  it("offers only the choices within an organization's limit, and says who limited them", async () => {
    put();
    control(app({ levels: ["read", "ask"], limit: "ask", managed_by: { level: "org_admin", people: [], more: 0, unreachable: false } }), entry({ level: "ask" }));
    const select = await screen.findByRole("combobox");
    expect(within(select).getAllByRole("option").map((o) => o.textContent)).toEqual(["Read only", "Ask before changes", "Custom…", "What exactly?"]);
    expect(screen.getByText("Limited by your organization.")).toBeInTheDocument();
  });

  it("renders no control for an account with no choice", () => {
    put();
    control(website(), entry({ level: "use" }));
    expect(screen.queryByRole("radio")).toBeNull();
    expect(screen.queryByRole("button", { name: "Custom…" })).toBeNull();
  });

  it("lists the always allowed changes under Ask before changes; removing one makes the agent ask again", async () => {
    put();
    control(app(), entry({ level: "ask", always_allowed: [{ tool_id: "merge", name: "Merge a pull request" }] }));
    expect((await screen.findByText("Always allowed:")).parentElement).toHaveTextContent("Always allowed: Merge a pull request");
    fireEvent.click(await screen.findByRole("button", { name: "Ask again for Merge a pull request" }));
    await waitFor(() => expect(sent("PUT /v1/agents/a1/grants/connections/c1")).toHaveLength(1));
    expect(sent("PUT /v1/agents/a1/grants/connections/c1")[0].body).toEqual({ mode: "custom", allow_tools: [], ask_tools: ["push", "merge"] });
  });

  it("says how many actions differ when the saved choice is custom, with no choice picked", async () => {
    put();
    control(app(), entry({ level: "custom", custom: { differs: 2, base: "read" } }));
    expect(await screen.findByText(/2 actions differ/)).toBeInTheDocument();
    expect(screen.getByText(/from Read only/)).toBeInTheDocument();
    expect(screen.getByRole("combobox")).toHaveValue("__custom");
  });

  it("gives people who cannot change the access text and no controls", () => {
    put();
    control(app({ can: MEMBER }), entry({ level: "ask", always_allowed: [{ tool_id: "merge", name: "Merge a pull request" }] }), false);
    expect(screen.getByText("Ask before changes")).toBeInTheDocument();
    expect(screen.getByText(/Merge a pull request/)).toBeInTheDocument();
    expect(screen.queryByRole("radio")).toBeNull();
    // The only control is the one that opens the details, which are text for them (C-934).
    expect(screen.getAllByRole("button").map((b) => b.textContent)).toEqual(["What exactly?"]);
  });
});

describe("the actions table", () => {
  const tools = [
    { id: "get", name: "Look up a repository", description: "Reads one repository.", group: "read", host: "api.example.com", methods: ["GET"], path: "/repos", connect_methods: [], wildcard: false },
    { id: "merge", name: "Merge a pull request", description: "Merges it.", group: "write", host: "api.example.com", methods: ["PUT"], path: "/merge", connect_methods: [], wildcard: false },
    { id: "push", name: "Push a commit", description: "Pushes.", group: "write", host: "api.example.com", methods: ["POST"], path: "/push", connect_methods: [], wildcard: false },
  ];
  const saved = (mode: "full" | "custom", allow: string[], ask: string[]) => ({ mode, allow_tools: allow, ask_tools: ask });

  it("starts from the saved choice; Read only and Ask before changes read back as they were saved; act allows everything; a fresh custom_only app allows reads", () => {
    const decide = (e: AgentAccess["level"] | undefined, s: ReturnType<typeof saved> | undefined, customOnly = false) =>
      startingDecisions(tools as never, e ? { level: e } : undefined, s, customOnly);
    expect(decide("read", saved("custom", ["get"], []))).toEqual({ get: "allow", merge: "block", push: "block" });
    expect(decide("ask", saved("custom", ["get", "merge"], ["push"]))).toEqual({ get: "allow", merge: "allow", push: "ask" });
    expect(decide("act", undefined)).toEqual({ get: "allow", merge: "allow", push: "allow" });
    expect(decide(undefined, saved("full", [], []))).toEqual({ get: "allow", merge: "allow", push: "allow" });
    expect(decide(undefined, undefined, true)).toEqual({ get: "allow", merge: "block", push: "block" });
    expect(decide(undefined, undefined, false)).toEqual({ get: "block", merge: "block", push: "block" });
  });

  it("(P5-TOOLASK) an action its app asks about first starts as Asks you even under Act; the override is choosing Allowed", () => {
    const withDefault = tools.map((t) => (t.id === "push" ? { ...t, approval: "ask" } : { ...t, approval: null }));
    expect(startingDecisions(withDefault as never, { level: "act" }, undefined, false)).toEqual({ get: "allow", merge: "allow", push: "ask" });
    expect(startingDecisions(withDefault as never, undefined, saved("full", [], []), false)).toEqual({ get: "allow", merge: "allow", push: "ask" });
    // A custom grant that lists the action as allowed is the workspace's explicit override.
    expect(startingDecisions(withDefault as never, { level: "custom" } as never, saved("custom", ["get", "push"], []), false).push).toBe("allow");
  });

  it("(P5-TOOLASK) the row says Asks first (app default) and offers Allowed as the override", async () => {
    const asked = tools.map((t) => (t.id === "push" ? { ...t, approval: "ask" } : t));
    routes["GET /v1/apps/aws"] = () => json(200, { app_id: "aws", name: "AWS", tools: asked });
    routes["GET /v1/agents/a1/grants"] = () => json(200, { secrets: [], connections: [{ connection_id: "c1", mode: "full", allow_tools: [], ask_tools: [], ask_timeout_secs: null }], converted_by_app: [] });
    routes["PUT /v1/agents/a1/grants/connections/c1"] = () => json(200, {});
    routes["GET /v1/workspaces/w1/accounts"] = () => json(200, { items: [], next_cursor: null });
    routes["GET /v1/agents/a1/apps/aws/access"] = () => json(200, ACCESS);
    wrap(<AccessControl account={app()} entry={entry({ level: "act" })} workspaceId="w1" canChange />);
    // (P5-POLISH-6) "What exactly?" is an option of the access select.
    fireEvent.change(await screen.findByRole("combobox", { name: "Daneel's access to AWS prod" }), { target: { value: "__exactly" } });
    const dlg = await screen.findByRole("region", { name: /What Daneel can do in AWS/ });
    // Only the action with the app default says so.
    expect(within(dlg).getAllByText(/Asks first \(app default\)/)).toHaveLength(1);
    const select = within(dlg).getByLabelText("Push a commit: what happens") as HTMLSelectElement;
    expect(select.value).toBe("ask");
    expect((within(dlg).getByLabelText("Merge a pull request: what happens") as HTMLSelectElement).value).toBe("allow");
    // Allowed is the override: saved as a custom grant that names the action.
    fireEvent.change(select, { target: { value: "allow" } });
    expect(select.value).toBe("allow");
  });

  async function openTable(e: AgentAccess, a: Account = app()) {
    routes["GET /v1/apps/aws"] = () => json(200, { app_id: "aws", name: "AWS", tools });
    routes["GET /v1/agents/a1/grants"] = () =>
      json(200, { secrets: [], connections: [{ connection_id: "c1", mode: "custom", allow_tools: ["get"], ask_tools: ["push"], ask_timeout_secs: null }], converted_by_app: [] });
    routes["PUT /v1/agents/a1/grants/connections/c1"] = () => json(200, {});
    routes["GET /v1/workspaces/w1/accounts"] = () => json(200, { items: [], next_cursor: null });
    routes["GET /v1/agents/a1/apps/aws/access"] = () => json(200, ACCESS);
    wrap(<AccessControl account={a} entry={e} workspaceId="w1" canChange />);
    fireEvent.change(await screen.findByRole("combobox"), { target: { value: "__custom" } });
    return screen.findByRole("region", { name: /What Daneel can do in AWS/ });
  }
  const ACCESS = {
    accounts: [
      {
        ref: { type: "connection", id: "c1" },
        name: "AWS prod",
        tools: [
          { tool_id: "get", name: "Look up a repository", description: "Reads one repository.", access: "read", decision: "allow", own: "allow" },
          { tool_id: "push", name: "Push a commit", description: "Pushes.", access: "write", decision: "block", own: "ask", reason: { layer: "limit", label: "Limited by Acme" } },
          { tool_id: "merge", name: "Merge a pull request", description: "Merges it.", access: "write", decision: "block", own: "allow", reason: { layer: "org_rule", label: "No merging", link: "internet_access" } },
        ],
      },
    ],
  };

  it("opens the same table from What exactly? and from Custom…; reasons appear only where something else decided", async () => {
    const dlg = await openTable(entry({ level: "custom", custom: { differs: 1, base: "read" } }));
    // The chip of an action with a reason shows what actually happens, and why; the one without shows only the control.
    expect(await within(dlg).findByText("Limited by Acme")).toBeInTheDocument();
    expect(within(dlg).getByText(/No merging/)).toBeInTheDocument();
    expect(within(dlg).getByRole("link", { name: "Internet access" })).toHaveAttribute("href", "/o/acme/internet-access");
    expect(within(dlg).getAllByText("Blocked").filter((e) => e.tagName === "SPAN")).toHaveLength(2);
    expect(within(dlg).queryByText(/Look up a repository.*Limited/)).toBeNull();
    // What exactly? opens the same table (one table, one select).
    fireEvent.change(screen.getByRole("combobox", { name: "Daneel's access to AWS prod" }), { target: { value: "__exactly" } });
    expect(await screen.findByRole("region", { name: /What Daneel can do in AWS/ })).toBeInTheDocument();
  });

  it("is text for someone who may not change the access: the chip, the reason and the agent's own choice beside it", async () => {
    routes["GET /v1/agents/a1/apps/aws/access"] = () => json(200, ACCESS);
    wrap(<AccessControl account={app({ can: MEMBER })} entry={entry({ level: "ask" })} workspaceId="w1" canChange={false} />);
    fireEvent.click(await screen.findByRole("button", { name: "What exactly?" }));
    const region = await screen.findByRole("region", { name: /What Daneel can do in AWS/ });
    expect(within(region).queryByRole("combobox")).toBeNull();
    expect(within(region).getByText("Allowed")).toBeInTheDocument();
    expect(within(region).getByText("Limited by Acme")).toBeInTheDocument();
    expect(within(region).getByText("Daneel's access: Asks you")).toBeInTheDocument();
    // Without a reason the agent's own choice is the answer and is not repeated.
    expect(within(region).getAllByText(/Daneel's access:/)).toHaveLength(2);
    expect(within(region).getByRole("heading", { name: "Changing things" })).toBeInTheDocument();
  });

  it("has no details for websites and AI keys", () => {
    wrap(<AccessControl account={website()} entry={entry({ level: "use" })} workspaceId="w1" canChange />);
    expect(screen.queryByRole("button", { name: "What exactly?" })).toBeNull();
  });

  it("shows each action with its saved choice and keeps the method and host under Technical details; Save sends the custom body", async () => {
    const dlg = await openTable(entry({ level: "custom", custom: { differs: 1, base: "read" } }));
    const get = await within(dlg).findByLabelText("Look up a repository: what happens");
    expect(get).toHaveValue("allow");
    expect(within(dlg).getByLabelText("Push a commit: what happens")).toHaveValue("ask");
    expect(within(dlg).getByLabelText("Merge a pull request: what happens")).toHaveValue("block");
    expect(within(dlg).getAllByText("Technical details").length).toBe(3);
    expect(within(dlg).getByText("Reads one repository.")).toBeInTheDocument();
    fireEvent.change(within(dlg).getByLabelText("Merge a pull request: what happens"), { target: { value: "allow" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(sent("PUT /v1/agents/a1/grants/connections/c1")).toHaveLength(1));
    expect(sent("PUT /v1/agents/a1/grants/connections/c1")[0].body).toEqual({ mode: "custom", allow_tools: ["get", "merge"], ask_tools: ["push"] });
  });

  it("will not save with nothing allowed or asked", async () => {
    const dlg = await openTable(entry({ level: "custom", custom: { differs: 1, base: "read" } }));
    fireEvent.change(await within(dlg).findByLabelText("Look up a repository: what happens"), { target: { value: "block" } });
    fireEvent.change(within(dlg).getByLabelText("Push a commit: what happens"), { target: { value: "block" } });
    expect(within(dlg).getByText(/Allow or ask for at least one action/)).toBeInTheDocument();
    expect(within(dlg).getByRole("button", { name: "Save" })).toBeDisabled();
  });
});

describe("picker preselection (C-925) and row format (C-948)", () => {
  it("preselects the contextual agent, else the only agent, else none", () => {
    const agents = [{ id: "a1" }, { id: "a2" }];
    expect(preselectAgents(agents, "a2")).toEqual(["a2"]);
    expect(preselectAgents([{ id: "a1" }], null)).toEqual(["a1"]);
    expect(preselectAgents(agents, null)).toEqual([]);
    expect(preselectAgents(agents, "zz")).toEqual([]);
    expect(preselectAgents([], null)).toEqual([]);
  });

  it("formats an agent as <workspace> · <agent> only when several workspaces are listed", () => {
    expect(agentLabel(DANEEL, OPS, true)).toBe("Ops · Daneel");
    expect(agentLabel(DANEEL, OPS, false)).toBe("Daneel");
    expect(entryText(entry({ level: "ask" }))).toBe("Daneel · Ask before changes");
    expect(entryText(entry({ level: "act" }))).toBe("Daneel · Change without asking");
    expect(entryText(entry({ level: "custom" }))).toBe("Daneel · Custom");
    expect(entryText(entry({ level: "use" }))).toBe("Daneel");
    // An app with no levels (read only) is granted `use`: the pill still says what it can do.
    expect(entryText(entry({ level: "use" }), { ref: { type: "connection", id: "c1" }, levels: [] })).toBe("Daneel · Read only");
    expect(entryText(entry({ level: "use" }), { ref: { type: "secret", id: "s1" }, levels: [] })).toBe("Daneel");
  });

  it("splits the agent's list into given and not given", () => {
    const { given, notGiven } = splitGiven([{ ...app(), given: true }, { ...website(), given: false }, aiKey()] as Account[]);
    expect(given.map((a) => a.name)).toEqual(["AWS prod", "Team key"]);
    expect(notGiven.map((a) => a.name)).toEqual(["Stripe API"]);
  });

  const agentRows = [
    { id: "a1", name: "Daneel", handle: "daneel", kind: "hosted" },
    { id: "a2", name: "Giskard", handle: "giskard", kind: "hosted" },
  ];
  const panel = (contextAgentId?: string) =>
    wrap(
      <GiveAccessPanel
        subject={{ target: { kind: "secret", id: "s1" }, scope: { kind: "workspace", id: "w1" }, orgId: "o1", contextAgentId }}
        onDismiss={() => undefined}
      />,
    );
  beforeEach(() => {
    routes["GET /v1/workspaces/w1/agents"] = () => json(200, { items: agentRows, next_cursor: null });
    routes["GET /v1/secrets/s1/grants"] = () => json(200, { items: [] });
  });

  it("opens with the contextual agent ticked and the button naming it", async () => {
    panel("a2");
    expect(await screen.findByRole("checkbox", { name: "Give Giskard access" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Give Daneel access" })).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Give Giskard access" })).toBeEnabled();
  });

  it("opens with nothing ticked when several agents may be chosen and none is the context", async () => {
    panel();
    expect(await screen.findByRole("checkbox", { name: "Give Daneel access" })).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Give Giskard access" })).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Give access" })).toBeDisabled();
  });

  it("prefixes rows with the workspace when an organization's account lists agents of several", async () => {
    routes["GET /v1/orgs/o1/workspaces"] = () =>
      json(200, { items: [{ id: "w1", org_id: "o1", name: "Ops", my_role: "owner" }, { id: "w2", org_id: "o1", name: "Lab", my_role: "owner" }], next_cursor: null });
    routes["GET /v1/workspaces/w2/agents"] = () => json(200, { items: [{ id: "b1", name: "Hari", handle: "hari", kind: "hosted" }], next_cursor: null });
    wrap(
      <GiveAccessPanel
        subject={{ target: { kind: "secret", id: "s1" }, scope: { kind: "organization", id: "o1" }, orgId: "o1" }}
        onDismiss={() => undefined}
      />,
    );
    expect(await screen.findByText("Ops · Daneel")).toBeInTheDocument();
    expect(screen.getByText("Lab · Hari")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Give access" })).toBeDisabled();
  });
});
