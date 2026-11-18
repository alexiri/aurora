import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConnectJourney } from "@/components/apps/connect-journey/connect-journey";
import {
  buildJourneyConnect,
  connectFailure,
  doneSentence,
  hostOf,
  levelsForApp,
  nextStep,
  prevStep,
  scopesFor,
  stepsOf,
  tickList,
  type JourneyShape,
} from "@/components/apps/connect-journey/logic";
import { ApiError } from "@/lib/api/client";
import type { ConnectMethod } from "@/lib/api/apps";

// P5-UX-3 (C-914, `14 §3`): the connect journey: its steps and skips, the Done sentences, the payload of each access
// choice, blocked tiles and methods, the grant_failed retry and the deep-link entry (an app given, an agent fixed).

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useSearchParams: () => new URLSearchParams(""),
  usePathname: () => "/o/acme/w/main/apps",
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
}));

const shape = (over: Partial<JourneyShape> = {}): JourneyShape => ({
  appGiven: false,
  agentFixed: false,
  noAgents: false,
  website: false,
  hasExisting: false,
  useExisting: false,
  hasChoice: true,
  noAgentChosen: false,
  ...over,
});

describe("the step machine and its skips", () => {
  it("a full journey: app, agent, access, sign in, done", () => {
    expect(stepsOf(shape())).toEqual(["app", "agent", "access", "signin", "done"]);
  });
  it("a given app skips the App step; a fixed agent skips the Agent step", () => {
    expect(stepsOf(shape({ appGiven: true, agentFixed: true }))).toEqual(["access", "signin", "done"]);
  });
  it("an app that can only read has no Access step", () => {
    expect(stepsOf(shape({ appGiven: true, agentFixed: true, hasChoice: false }))).toEqual(["signin", "done"]);
  });
  it("an existing account adds the Account step; using it skips only Sign in", () => {
    const s = shape({ appGiven: true, agentFixed: true, hasExisting: true });
    expect(stepsOf(s)).toEqual(["account", "access", "signin", "done"]);
    expect(stepsOf({ ...s, useExisting: true })).toEqual(["account", "access", "done"]);
  });
  it("a website has no Account or Access step", () => {
    expect(stepsOf(shape({ website: true, hasExisting: true }))).toEqual(["app", "agent", "signin", "done"]);
  });
  it("no agent chosen skips Access; a workspace without agents skips the Agent step", () => {
    expect(stepsOf(shape({ noAgentChosen: true }))).toEqual(["app", "agent", "signin", "done"]);
    expect(stepsOf(shape({ noAgents: true, noAgentChosen: true }))).toEqual(["app", "signin", "done"]);
  });
  it("next and back follow the list", () => {
    const s = shape({ appGiven: true, agentFixed: true });
    expect(nextStep(s, "access")).toBe("signin");
    expect(prevStep(s, "signin")).toBe("access");
    expect(prevStep(s, "access")).toBeNull();
  });
});

describe("words and payloads", () => {
  it("each Done sentence", () => {
    const d = (level: "read" | "ask" | "act" | null, agentChosen = true) => doneSentence({ agent: "Daneel", app: "GitHub", level, agentChosen });
    expect(d("read")).toBe("Daneel can now read GitHub.");
    expect(d("ask")).toBe("Daneel can now read GitHub, and asks you before each change.");
    expect(d("act")).toBe("Daneel can now read and change GitHub without asking.");
    expect(d(null)).toBe("Daneel can now use GitHub.");
    expect(d("read", false)).toBe("GitHub is connected. No agent can use it yet.");
  });

  const method = {
    id: "token",
    kind: "fields",
    fields: [{ name: "access_token", label: "Token", type: "secret", store: "credentials", required: true }],
    scopes: [
      { scope: "repo", name: "repo", access: "write" },
      { scope: "read:org", name: "read:org", access: "read" },
    ],
    scope_presets: { read: ["read:org"], write: ["repo", "read:org"] },
    default_scopes: [],
    access_levels: [],
    probe_scopes: [],
    show_context: [],
  } as unknown as ConnectMethod;

  it("the permissions line for Read only and for Ask before changes", () => {
    expect(tickList(method, "read")).toEqual(["read:org"]);
    expect(tickList(method, "ask")).toEqual(["repo", "read:org"]);
    expect(scopesFor(method, "act")).toEqual(["repo", "read:org"]);
    expect(scopesFor(method, "read", ["x"])).toEqual(["x"]);
  });

  it("the payload for each access choice", () => {
    const base = { workspaceId: "w1", method, fields: { access_token: "t" }, agentIds: ["a1"], hasWriteTools: true };
    expect(buildJourneyConnect({ ...base, level: "read" })).toMatchObject({ grant: { agent_ids: ["a1"], level: "read" }, scopes: ["read:org"] });
    expect(buildJourneyConnect({ ...base, level: "ask" })).toMatchObject({ grant: { agent_ids: ["a1"], level: "ask" }, scopes: ["repo", "read:org"] });
    expect(buildJourneyConnect({ ...base, level: "act" })).toMatchObject({ grant: { agent_ids: ["a1"], level: "act" } });
    // no agent: only the account, read-only until someone gives it access
    const none = buildJourneyConnect({ ...base, agentIds: [], level: "read" });
    expect(none).not.toHaveProperty("grant");
    expect(none).toMatchObject({ access: "read" });
    // no choice to make: the grant is "read", which allows every read the app has
    expect(buildJourneyConnect({ ...base, level: null, hasWriteTools: false })).toMatchObject({ grant: { level: "read" } });
  });

  it("an app with no write tools offers no choice; a custom-only app offers no change-without-asking", () => {
    const tool = (group: string) => ({ id: group, name: group, group });
    expect(levelsForApp({ tools: [tool("read")], app_restrictions: { custom_only: false, block_untooled: false } } as never)).toEqual([]);
    expect(levelsForApp({ tools: [tool("write")], app_restrictions: { custom_only: false, block_untooled: false } } as never)).toEqual(["read", "ask", "act"]);
    expect(levelsForApp({ tools: [tool("write")], app_restrictions: { custom_only: true, block_untooled: false } } as never)).toEqual(["read", "ask"]);
  });

  it("failures name the app (C-958)", () => {
    const err = (cause: string, extra: object = {}) => new ApiError("connect_probe_failed", "x", { cause, ...extra }, 422);
    expect(connectFailure(err("rejected"), "GitHub")).toBe("GitHub did not accept this token. Check that it is complete and has not expired.");
    expect(connectFailure(err("unreachable", { host: "git.example.org" }), "GitHub")).toBe("Aurora could not reach GitHub (git.example.org).");
    expect(connectFailure(err("unexpected", { upstream_status: 502 }), "GitHub")).toBe("GitHub answered with an unexpected error (502).");
  });

  it("a website's host is read from what was typed", () => {
    expect(hostOf("https://api.example.com/v1")).toBe("api.example.com");
    expect(hostOf(" api.example.com ")).toBe("api.example.com");
  });
});

// ---- the component ----

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

class FakeChannel {
  static open = new Set<FakeChannel>();
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  constructor(public name: string) {
    FakeChannel.open.add(this);
  }
  postMessage(data: unknown) {
    for (const c of FakeChannel.open) if (c !== this && c.name === this.name) c.onmessage?.({ data });
  }
  close() {
    FakeChannel.open.delete(this);
  }
}

const field = { name: "access_token", label: "Token", type: "secret", store: "credentials", required: true, help_url: "https://github.com/settings/tokens" };
const method = (over: Record<string, unknown>) => ({
  kind: "fields", label: null, description: null, recommended: false, fields: [], scopes: [], default_scopes: [], access_levels: [],
  probe_scopes: [], client_key: null, callback: null, audience: "person", scope_presets: { read: [], write: [] }, show_context: [], ...over,
});
const DETAIL = {
  app_id: "github", name: "GitHub", package_id: "github", package_version: "2.0.0", description: "d", available: true, connection_count: 0,
  has_dark_icon: false, category: "development", api_docs: null, connect_note: null, label_hint: null, credential_free: false,
  app_restrictions: { custom_only: false, block_untooled: false },
  connect: [
    method({
      id: "token", label: "Token", fields: [field], recommended: true,
      scopes: [{ scope: "repo", name: "repo", access: "write" }, { scope: "read:org", name: "read:org", access: "read" }],
      scope_presets: { read: ["read:org"], write: ["repo", "read:org"] },
    }),
    method({ id: "oauth", kind: "oauth2", label: "Sign in", client_key: "gh", callback: "query" }),
  ],
  tools: [{ id: "list", name: "List issues", group: "read" }, { id: "create", name: "Create issue", group: "write" }],
  hosts: [], agent_instructions: null, credentials: [], metadata: [], oauth_clients: [], instances: null,
};
const planMethod = (id: string, kind: string, over: Record<string, unknown> = {}) => ({
  id, kind, label: id, description: null, recommended: false, audience: "person", needs: [], ready: true, scope_presets: { read: [], write: [] }, ...over,
});
const PLAN = { app_id: "github", available: true, suggested_method: "token", methods: [planMethod("token", "fields"), planMethod("oauth", "oauth2")] };
const CONN = { id: "c9", scope: "workspace", app_id: "github", scopes: [], missing_scopes: [], label: "me", status: "active", access: "read" };
const ACCOUNT = {
  ref: { type: "connection", id: "c1" }, kind: "app", name: "GitHub · bob-builder", scope: "workspace", levels: ["read", "ask", "act"],
  app: { app_id: "github", name: "GitHub", custom_only: false }, health: { state: "unknown" }, agents: [], connected_by: { name: "Bob" }, given: false,
  can: { give_access: true, change_access: true, manage: true },
};
const WHO = { action: "set_up_oauth", role: "org_admin", people: [{ id: "u1", name: "Ada Lovelace" }], you_can: false };

interface Call { method: string; path: string; body: unknown }
function stub(extra: Record<string, (c: Call) => Response> = {}) {
  const calls: Call[] = [];
  const routes: Record<string, (c: Call) => Response> = {
    "GET /v1/apps": () => json(200, { items: [{ ...DETAIL, connection_count: 0 }], next_cursor: null }),
    "GET /v1/apps/github": () => json(200, DETAIL),
    "GET /v1/apps/github/guide": () => json(404, { error: { code: "not_found", message: "x" } }),
    "GET /v1/apps/github/connect-plan": () => json(200, PLAN),
    "GET /v1/workspaces/w1/oauth-clients": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/workspaces/w1/accounts": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/workspaces/w1/agents": () => json(200, { items: [{ id: "a1", name: "Daneel", handle: "daneel", kind: "hosted" }], next_cursor: null }),
    "GET /v1/orgs/o1/app-instances": () => json(200, { items: [], next_cursor: null }),
    ...extra,
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url);
      const c: Call = { method: input.method, path: url.pathname, body: input.method === "GET" ? null : await input.json().catch(() => null) };
      calls.push(c);
      const h = routes[`${c.method} ${c.path}`];
      return h ? h(c) : json(404, { error: { code: "not_found", message: "x" } });
    }),
  );
  return calls;
}

function open(props: Partial<React.ComponentProps<typeof ConnectJourney>> = {}) {
  const onClose = vi.fn();
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <ConnectJourney workspaceId="w1" orgId="o1" agent={{ id: "a1", name: "Daneel" }} appId="github" onClose={onClose} {...props} />
    </QueryClientProvider>,
  );
  return onClose;
}
const post = (calls: Call[]) => calls.find((c) => c.method === "POST" && c.path === "/v1/apps/github/connect");

const openSpy = vi.fn();
beforeEach(() => {
  vi.stubGlobal("BroadcastChannel", FakeChannel);
  vi.stubGlobal("open", openSpy);
});
afterEach(() => {
  openSpy.mockReset();
  FakeChannel.open.clear();
});

describe("the journey for a fixed agent and a given app", () => {
  it("Read only: Give Daneel GitHub → Next → the token page and the permissions → Connect → Done", async () => {
    const calls = stub({ "POST /v1/apps/github/connect": () => json(200, { kind: "connected", connection: CONN, grants: [{ agent_id: "a1" }], missing_scopes: [] }) });
    open();
    await screen.findByRole("dialog", { name: "Give Daneel GitHub" });
    // Access first: Read only is preselected
    expect(await screen.findByRole("radio", { name: /Read only/ })).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    const link = await screen.findByRole("link", { name: "Create a token on GitHub" });
    expect(link).toHaveAttribute("href", "https://github.com/settings/tokens");
    expect(screen.getByText(/For Read only, tick/)).toHaveTextContent("For Read only, tick: read:org");
    fireEvent.change(screen.getByLabelText("Token"), { target: { value: "ghp_x" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByText("Daneel can now read GitHub.")).toBeInTheDocument();
    expect(post(calls)?.body).toMatchObject({
      method_id: "token", fields: { access_token: "ghp_x" }, scopes: ["read:org"], grant: { agent_ids: ["a1"], level: "read" },
    });
    expect(screen.getByRole("link", { name: "Open chat" })).toHaveAttribute("href", "/o/acme/w/main/agents/daneel");
    expect(screen.getByRole("button", { name: "Give Daneel another app" })).toBeInTheDocument();
  });

  it("Ask before changes: the Done sentence, the write permissions and level ask", async () => {
    const calls = stub({ "POST /v1/apps/github/connect": () => json(200, { kind: "connected", connection: CONN, grants: [{ agent_id: "a1" }], missing_scopes: [] }) });
    open();
    fireEvent.click(await screen.findByRole("radio", { name: /Ask before changes/ }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.change(await screen.findByLabelText("Token"), { target: { value: "ghp_x" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByText("Daneel can now read GitHub, and asks you before each change.")).toBeInTheDocument();
    expect(post(calls)?.body).toMatchObject({ scopes: ["repo", "read:org"], grant: { level: "ask" } });
  });

  it("a wrong token says so by the app's name and keeps the form", async () => {
    stub({
      "POST /v1/apps/github/connect": () => json(422, { error: { code: "connect_probe_failed", message: "x", details: { cause: "rejected" } } }),
    });
    open();
    fireEvent.click(await screen.findByRole("button", { name: "Next" }));
    fireEvent.change(await screen.findByLabelText("Token"), { target: { value: "bad" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("GitHub did not accept this token. Check that it is complete and has not expired.");
    expect(screen.getByRole("button", { name: "Connect" })).toBeInTheDocument();
  });

  it("a blocked suggested method shows who can fix it and no form", async () => {
    stub({
      "GET /v1/apps/github/connect-plan": () => json(200, { app_id: "github", available: false, available_who_can: WHO, suggested_method: null, methods: [planMethod("token", "fields", { ready: false, who_can: WHO })] }),
    });
    open();
    fireEvent.click(await screen.findByRole("button", { name: "Next" }));
    expect(await screen.findByText("GitHub cannot be connected yet.")).toBeInTheDocument();
    expect(screen.getAllByText(/Ada Lovelace/).length).toBeGreaterThan(0);
    expect(screen.queryByLabelText("Token")).toBeNull();
    expect(screen.getByRole("button", { name: "Connect" })).toBeDisabled();
  });

  it("an existing account: Use it, choose the access, and only the access is saved", async () => {
    const calls = stub({
      "GET /v1/workspaces/w1/accounts": () => json(200, { items: [ACCOUNT], next_cursor: null }),
      "PUT /v1/agents/a1/grants/connections/c1": () => json(200, {}),
    });
    open();
    expect(await screen.findByRole("radio", { name: /Use GitHub · bob-builder \(connected by Bob\)/ })).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.click(await screen.findByRole("radio", { name: /Ask before changes/ }));
    fireEvent.click(screen.getByRole("button", { name: "Give Daneel access" }));
    expect(await screen.findByText("Daneel can now read GitHub, and asks you before each change.")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "PUT")?.body).toEqual({ level: "ask" });
    expect(post(calls)).toBeUndefined();
  });

  it("grant_failed after a sign-in shows the account as made; Give access retries only the access", async () => {
    openSpy.mockReturnValue({ closed: false, opener: window, location: { replace: vi.fn() }, focus: vi.fn(), close: vi.fn() });
    const calls = stub({
      "POST /v1/apps/github/connect": () => json(200, { kind: "redirect", url: "https://provider.example/a" }),
      "PUT /v1/agents/a1/grants/connections/c9": () => json(200, {}),
    });
    open();
    fireEvent.click(await screen.findByRole("button", { name: "Next" }));
    fireEvent.click(await screen.findByText(/^Advanced/));
    fireEvent.click(await screen.findByRole("radio", { name: /Sign in/ }));
    fireEvent.click(screen.getByRole("button", { name: "Sign in to GitHub" }));
    await screen.findByText("Waiting for you to sign in to GitHub…");
    await act(async () =>
      new FakeChannel("aurora-connect").postMessage({
        type: "aurora:connected", app_id: "github", connection_id: "c9", missing_scopes: [], granted_agent_ids: [], grant_failed: { code: "not_found" },
      }),
    );
    expect(await screen.findByText("GitHub is connected, but Daneel could not be given access.")).toBeInTheDocument();
    const before = calls.filter((c) => c.method === "POST").length;
    fireEvent.click(screen.getByRole("button", { name: "Give access" }));
    expect(await screen.findByText("Daneel can now read GitHub.")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "PUT")?.body).toEqual({ level: "read" });
    expect(calls.filter((c) => c.method === "POST").length).toBe(before);
  });
});

describe("the App step", () => {
  it("a blocked tile shows who can fix it and cannot be opened; another website is the last tile", async () => {
    stub({
      "GET /v1/apps/github/connect-plan": () => json(200, { app_id: "github", available: false, available_who_can: WHO, suggested_method: null, methods: [] }),
    });
    open({ agent: undefined, appId: null });
    const tile = await screen.findByRole("button", { name: "GitHub" });
    expect(screen.getByRole("button", { name: "Another website or API" })).toBeInTheDocument();
    fireEvent.focus(tile);
    expect(await screen.findByText("Needs setup by an admin")).toBeInTheDocument();
    expect(screen.getByText(/Ada Lovelace/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "GitHub" })).toBeNull();
  });

  it("from Apps & accounts in a one-agent workspace the agent is preselected and the button says Give Daneel access", async () => {
    stub();
    open({ agent: undefined, appId: null });
    fireEvent.click(await screen.findByRole("button", { name: "GitHub" }));
    expect(await screen.findByRole("checkbox", { name: "Give Daneel access" })).toBeChecked();
    expect(screen.getByRole("button", { name: "Give Daneel access" })).toBeInTheDocument();
  });

  it("Another website or API saves the account and Daneel's access in one request", async () => {
    const calls = stub({
      "POST /v1/workspaces/w1/secrets": () => json(201, { id: "s1", name: "Billing API", kind: "generic" }),
    });
    open({ agent: { id: "a1", name: "Daneel" }, appId: null });
    fireEvent.click(await screen.findByRole("button", { name: "Another website or API" }));
    fireEvent.change(await screen.findByLabelText("Name"), { target: { value: "Billing API" } });
    fireEvent.change(screen.getByLabelText("Website"), { target: { value: "https://api.billing.example/v1" } });
    fireEvent.change(screen.getByLabelText("Token"), { target: { value: "tok" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByText("Daneel can now use Billing API.")).toBeInTheDocument();
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    expect(calls.find((c) => c.method === "POST")?.body).toMatchObject({
      name: "Billing API", kind: "generic", host_pattern: "api.billing.example", value: "tok", grant: { agent_ids: ["a1"] },
    });
  });
});
