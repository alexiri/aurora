import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AccessControl } from "@/components/accounts/access-control";
import { startingDecisions } from "@/components/accounts/actions-table";
import { AccountsList } from "@/components/accounts/accounts-list";
import { GiveAccessPanel } from "@/components/credentials/give-access";
import { agentLabel, entryText, preselectAgents, splitGiven } from "@/lib/accounts";
import type { Account, AgentAccess } from "@/lib/api/accounts";

// (P5-UX-4, C-917, C-925, C-938) The one list of accounts, the access choice and the actions table.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops" }),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
  usePathname: () => "/o/acme/w/ops/apps",
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
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
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

const DANEEL = { id: "a1", name: "Daneel", handle: "daneel" };
const GISKARD = { id: "a2", name: "Giskard", handle: "giskard" };
const OPS = { id: "w1", name: "Ops" };
const entry = (over: Partial<AgentAccess> = {}): AgentAccess => ({ agent: DANEEL, workspace: OPS, level: "read", ...over }) as AgentAccess;
const OWNER = { give_access: true, change_access: true, manage: true };
const MEMBER = { give_access: false, change_access: false, manage: false };

const app = (over: Record<string, unknown> = {}): Account =>
  ({
    ref: { type: "connection", id: "c1" }, kind: "app", name: "AWS prod", scope: "workspace", levels: ["read", "ask", "act"],
    app: { app_id: "aws", name: "AWS", custom_only: false }, health: { state: "unknown", last_used_at: null }, agents: [entry()], can: OWNER, ...over,
  }) as Account;
const website = (over: Record<string, unknown> = {}): Account =>
  ({
    ref: { type: "secret", id: "s1" }, kind: "website", name: "Stripe API", scope: "workspace", levels: [], health: { state: "unknown" },
    agents: [entry({ level: "use" })], can: OWNER, ...over,
  }) as Account;
const aiKey = (over: Record<string, unknown> = {}): Account =>
  ({
    ref: { type: "secret", id: "s2" }, kind: "ai_key", name: "Team key", scope: "workspace", levels: [], health: { state: "unknown" },
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
  it("shows the kind chip, who can use it and the menu for each kind, for an owner", async () => {
    listWs([app(), website(), aiKey()]);
    list();
    const aws = await screen.findByRole("listitem", { name: "AWS prod" });
    expect(within(aws).getByText("App")).toBeInTheDocument();
    expect(within(aws).getByRole("button", { name: "Daneel · Read only: change access" })).toBeInTheDocument();
    expect(within(screen.getByRole("listitem", { name: "Stripe API" })).getByText("Website")).toBeInTheDocument();
    expect(within(screen.getByRole("listitem", { name: "Team key" })).getByText("AI key")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^More for / })).toHaveLength(3);
  });

  it("offers Give access, Rename, Reconnect, Turn off and Remove for an app; Replace and Test for a website, never a rule or host", async () => {
    listWs([app(), website()]);
    list();
    await openMenu("AWS prod");
    const names = (await screen.findAllByRole("menuitem")).map((m) => m.textContent);
    expect(names).toEqual(["Give access", "Rename", "Reconnect", "Turn off", "Remove"]);
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    cleanup();
    listWs([website()]);
    list();
    await openMenu("Stripe API");
    expect((await screen.findAllByRole("menuitem")).map((m) => m.textContent)).toEqual(["Give access", "Rename", "Replace", "Test", "Remove"]);
  });

  it("says Turn on for a turned-off account and the health line says why an account stopped", async () => {
    listWs([app({ health: { state: "disabled" } }), app({ ref: { type: "connection", id: "c2" }, name: "Jira", health: { state: "sign_in_expired" } })]);
    list();
    expect(await screen.findByText("Turned off")).toBeInTheDocument();
    expect(screen.getByText(/Sign-in expired/)).toBeInTheDocument();
    await openMenu("AWS prod");
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
    expect(within(gitlab).getByText("Shared by your organization")).toBeInTheDocument();
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

  it("an organization's list has Add an AI key only and its own empty state", async () => {
    routes["GET /v1/orgs/o1/accounts"] = () => json(200, { items: [], next_cursor: null });
    wrap(<AccountsList scope={{ kind: "organization", id: "o1" }} orgId="o1" canOwn />);
    expect(await screen.findByText("No accounts for the whole organization yet")).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("button", { name: "Add" }), { key: "Enter" });
    expect((await screen.findAllByRole("menuitem")).map((m) => m.textContent)).toEqual(["Add an AI key"]);
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
    expect(within(used).getByText("Daneel thinks with claude-sonnet through this key.")).toBeInTheDocument();
    expect(within(used).getAllByRole("radio").length).toBeGreaterThan(0);
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
    const radios = await screen.findAllByRole("radio");
    expect(radios.map((r) => r.closest("label")?.textContent ?? "")).toEqual([
      expect.stringContaining("Read only"),
      expect.stringContaining("Ask before changes"),
      expect.stringContaining("Change without asking"),
    ]);
    expect(radios[0]).toBeChecked();
    fireEvent.click(radios[1]);
    await waitFor(() => expect(sent("PUT /v1/agents/a1/grants/connections/c1")).toHaveLength(1));
    expect(sent("PUT /v1/agents/a1/grants/connections/c1")[0].body).toEqual({ level: "ask" });
  });

  it("offers only the choices within an organization's limit, and says who limited them", async () => {
    put();
    control(app({ levels: ["read", "ask"], limit: "ask", managed_by: { level: "org_admin", people: [], more: 0, unreachable: false } }), entry({ level: "ask" }));
    expect(await screen.findAllByRole("radio")).toHaveLength(2);
    expect(screen.queryByText("Change without asking")).toBeNull();
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
    expect(screen.getAllByRole("radio").every((r) => !(r as HTMLInputElement).checked)).toBe(true);
  });

  it("gives people who cannot change the access text and no controls", () => {
    put();
    control(app({ can: MEMBER }), entry({ level: "ask", always_allowed: [{ tool_id: "merge", name: "Merge a pull request" }] }), false);
    expect(screen.getByText("Ask before changes")).toBeInTheDocument();
    expect(screen.getByText(/Merge a pull request/)).toBeInTheDocument();
    expect(screen.queryByRole("radio")).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
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

  async function openTable(e: AgentAccess, a: Account = app()) {
    routes["GET /v1/apps/aws"] = () => json(200, { app_id: "aws", name: "AWS", tools });
    routes["GET /v1/agents/a1/grants"] = () =>
      json(200, { secrets: [], connections: [{ connection_id: "c1", mode: "custom", allow_tools: ["get"], ask_tools: ["push"], ask_timeout_secs: null }], converted_by_app: [] });
    routes["PUT /v1/agents/a1/grants/connections/c1"] = () => json(200, {});
    routes["GET /v1/workspaces/w1/accounts"] = () => json(200, { items: [], next_cursor: null });
    wrap(<AccessControl account={a} entry={e} workspaceId="w1" canChange />);
    fireEvent.click(await screen.findByRole("button", { name: "Custom…" }));
    return screen.findByRole("dialog");
  }

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
