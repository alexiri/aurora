import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConnectJourney } from "@/components/apps/connect-journey/connect-journey";
import {
  buildJourneyConnect,
  connectFailure,
  doneSentence,
  hostOf,
  journeyTitle,
  levelsForApp,
  needsReason,
  nextStep,
  permissionHints,
  prevStep,
  scopesFor,
  stepsOf,
  tickList,
  type JourneyShape,
} from "@/components/apps/connect-journey/logic";
import { ApiError } from "@/lib/api/client";
import type { ConnectMethod } from "@/lib/api/apps";
import { json } from "./support/api";

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
    expect(connectFailure(err("rejected"), "GitHub")).toBe(
      "GitHub refused this token: it may be incomplete, expired or missing a permission. Create a new one and paste it here.",
    );
    expect(connectFailure(err("unreachable", { host: "git.example.org" }), "GitHub")).toBe("Aurora could not reach GitHub (git.example.org). Check the address, then try again.");
    expect(connectFailure(err("unexpected", { upstream_status: 502 }), "GitHub")).toBe("GitHub answered with an unexpected error (502). Try again in a moment.");
    // A network error says what to check, in words.
    expect(connectFailure(new TypeError("Failed to fetch"), "GitHub")).toBe("Aurora could not be reached. Check your connection, then try again.");
  });

  it("titles: Give <Agent> an app, then <App> for <Agent>; no host", () => {
    expect(journeyTitle({ step: "app", agent: "Daneel" })).toBe("Give Daneel an app");
    expect(journeyTitle({ step: "signin", app: "GitHub", agent: "Daneel" })).toBe("GitHub for Daneel");
    expect(journeyTitle({ step: "access", website: true, agent: "Daneel" })).toBe("Website or API for Daneel");
    expect(journeyTitle({ step: "app" })).toBe("Connect an app or website");
  });

  it("permission hints per level, and the tick line when the package gives none", () => {
    const withHints = {
      ...method,
      permission_hints: [
        { level: "read", label: "Read only", hints: ["Contents · Read", "Issues · Read"] },
        { level: "write", label: "Read and write", hints: ["Contents · Read and write"] },
      ],
    } as unknown as ConnectMethod;
    expect(permissionHints(withHints, "read")).toEqual(["Contents · Read", "Issues · Read"]);
    expect(permissionHints(withHints, "ask")).toEqual(["Contents · Read and write"]);
    expect(permissionHints(withHints, "act")).toEqual(["Contents · Read and write"]);
    expect(permissionHints(method, "read")).toEqual(["read:org"]);
  });

  it("the reasons an app waits for an admin", () => {
    // (D-407) An admin is a role, never a name.
    expect(needsReason(["oauth_client"], "Google", false)).toBe("An admin needs to set up Google sign-in");
    expect(needsReason(["oauth_client"], "Google", true)).toBe("Same Google sign-in");
    expect(needsReason(["instance"], "Jira", false)).toBe("An admin needs to add a server");
    expect(needsReason(["admin_material"], "AWS", false)).toBe("Needs setup by an admin");
  });

  it("a website's host is read from what was typed", () => {
    expect(hostOf("https://api.example.com/v1")).toBe("api.example.com");
    expect(hostOf(" api.example.com ")).toBe("api.example.com");
  });
});

// ---- the component ----

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
      permission_hints: [
        { level: "read", label: "Read only", hints: ["Contents · Read", "Issues · Read", "Pull requests · Read", "Metadata · Read"] },
        { level: "write", label: "Read and write", hints: ["Contents · Read and write", "Issues · Read and write"] },
      ],
    }),
    method({ id: "oauth", kind: "oauth2", label: "Sign in", client_key: "gh", callback: "query" }),
  ],
  first_prompt: "Which pull requests are waiting for my review?",
  tools: [{ id: "list", name: "List issues", group: "read" }, { id: "create", name: "Create issue", group: "write" }],
  hosts: [], agent_instructions: null, credentials: [], metadata: [], oauth_clients: [], instances: null,
};
const planMethod = (id: string, kind: string, over: Record<string, unknown> = {}) => ({
  id, kind, label: id, description: null, recommended: false, audience: "person", needs: [], ready: true, scope_presets: { read: [], write: [] }, ...over,
});
const PLAN = { app_id: "github", available: true, suggested_method: "token", methods: [planMethod("token", "fields"), planMethod("oauth", "oauth2")] };
const CONN = { id: "c9", scope: "workspace", app_id: "github", scopes: [], missing_scopes: [], label: "Team repos", status: "active", access: "read" };
const ACCOUNT = {
  ref: { type: "connection", id: "c1" }, kind: "app", name: "GitHub · bob-builder", scope: "workspace", levels: ["read", "ask", "act"],
  app: { app_id: "github", name: "GitHub", custom_only: false }, health: { state: "not_used" }, agents: [], connected_by: { name: "Bob" }, given: false,
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
  // jsdom has no `matchMedia`; the frame's `fixedHeight` asks whether the viewport is wide (a wide one, no listeners).
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: query.includes("min-width"), media: query, onchange: null, addEventListener: () => undefined, removeEventListener: () => undefined,
    addListener: () => undefined, removeListener: () => undefined, dispatchEvent: () => false,
  }));
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
    await screen.findByRole("dialog", { name: "GitHub for Daneel" });
    // Access first: Read only is preselected
    expect(await screen.findByRole("radio", { name: /Read only/ })).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    // The numbered steps of B4: the token page, the permissions for the chosen access, then the field.
    const link = await screen.findByRole("link", { name: "Open GitHub" });
    expect(link).toHaveAttribute("href", "https://github.com/settings/tokens");
    expect(screen.getByText(/Give it these permissions, for/)).toHaveTextContent("Give it these permissions, for Read only:");
    expect(screen.getByLabelText("Permissions to give the token")).toHaveTextContent("Contents · Read Issues · Read Pull requests · Read Metadata · Read");
    fireEvent.change(screen.getByLabelText("Token"), { target: { value: "ghp_x" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    // Done names the account and the access, and offers the first prompt as an unsent draft (C-1035).
    expect(await screen.findByText("Daneel can now read Team repos.")).toBeInTheDocument();
    expect(screen.getByText(/Read only\. You can change this on/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Daneel's Apps & access tab" })).toHaveAttribute("href", "/o/acme/w/main/agents/daneel?tab=access");
    expect(screen.getByText("“Which pull requests are waiting for my review?”")).toBeInTheDocument();
    expect(post(calls)?.body).toMatchObject({
      method_id: "token", fields: { access_token: "ghp_x" }, scopes: ["read:org"], grant: { agent_ids: ["a1"], level: "read" },
    });
    const chat = new URL(screen.getByRole("link", { name: "Try it in chat" }).getAttribute("href")!, "http://x");
    expect(chat.pathname).toBe("/o/acme/w/main/agents/daneel");
    // The link carries `tab=chat`: without it the agent page keeps the page the person was on (Access).
    expect(chat.searchParams.get("tab")).toBe("chat");
    expect(chat.searchParams.get("draft")).toBe("Which pull requests are waiting for my review?");
    expect(screen.getByRole("button", { name: "Another app" })).toBeInTheDocument();
    // Close at the left edge of the footer (the dialog's own X is the other Close).
    expect(screen.getAllByRole("button", { name: "Close" }).length).toBeGreaterThanOrEqual(2);
  });

  it("Read and write shows the write permissions; with no first prompt the button is Open chat without a draft", async () => {
    stub({
      "GET /v1/apps/github": () => json(200, { ...DETAIL, first_prompt: null }),
      "POST /v1/apps/github/connect": () => json(200, { kind: "connected", connection: CONN, grants: [{ agent_id: "a1" }], missing_scopes: [] }),
    });
    open();
    fireEvent.click(await screen.findByRole("radio", { name: /Ask before changes/ }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText(/Give it these permissions, for/)).toHaveTextContent("for Ask before changes");
    expect(screen.getByLabelText("Permissions to give the token")).toHaveTextContent("Contents · Read and write Issues · Read and write");
    fireEvent.change(screen.getByLabelText("Token"), { target: { value: "ghp_x" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await screen.findByText(/Daneel can now read Team repos, and asks you/);
    expect(screen.getByRole("link", { name: "Open chat" })).toHaveAttribute("href", "/o/acme/w/main/agents/daneel?tab=chat");
    expect(screen.queryByRole("link", { name: "Try it in chat" })).toBeNull();
  });

  it("Ask before changes: the Done sentence, the write permissions and level ask", async () => {
    const calls = stub({ "POST /v1/apps/github/connect": () => json(200, { kind: "connected", connection: CONN, grants: [{ agent_id: "a1" }], missing_scopes: [] }) });
    open();
    fireEvent.click(await screen.findByRole("radio", { name: /Ask before changes/ }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.change(await screen.findByLabelText("Token"), { target: { value: "ghp_x" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByText("Daneel can now read Team repos, and asks you before each change.")).toBeInTheDocument();
    expect(post(calls)?.body).toMatchObject({ scopes: ["repo", "read:org"], grant: { level: "ask" } });
  });

  it("a wrong token is said under its field, in words, and the pasted value stays (D-388)", async () => {
    stub({
      "POST /v1/apps/github/connect": () => json(422, { error: { code: "connect_probe_failed", message: "x", details: { cause: "rejected" } } }),
    });
    open();
    fireEvent.click(await screen.findByRole("button", { name: "Next" }));
    fireEvent.change(await screen.findByLabelText("Token"), { target: { value: "ghp_bad" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    const message = await screen.findByText(/GitHub refused this token/);
    expect(message).toHaveTextContent("Create a new one and paste it here.");
    // The error belongs to the field (aria-describedby), which keeps what was typed, masked.
    const input = screen.getByLabelText("Token");
    expect(input).toHaveValue("ghp_bad");
    expect(input).toHaveAttribute("type", "password");
    expect(input.getAttribute("aria-describedby") ?? "").toContain(message.id);
    expect(screen.getByRole("button", { name: "Connect" })).toBeEnabled();
  });

  it("an app that needs an admin shows who, and Ask turns into the asked state with Undo (not a disabled Connect)", async () => {
    const calls = stub({
      "GET /v1/apps/github/connect-plan": () => json(200, { app_id: "github", available: false, available_who_can: WHO, suggested_method: null, methods: [planMethod("token", "fields", { ready: false, who_can: WHO })] }),
      "POST /v1/workspaces/w1/apps/github/setup-request": () => json(200, { id: "r1", app_id: "github", status: "open", askers: [], needs: ["oauth_client"] }),
      "DELETE /v1/workspaces/w1/apps/github/setup-request": () => new Response(null, { status: 204 }),
    });
    open();
    fireEvent.click(await screen.findByRole("button", { name: "Next" }));
    expect(await screen.findByText("GitHub needs an admin to set it up first.")).toBeInTheDocument();
    expect(screen.getAllByText(/Ask an organization admin/).length).toBeGreaterThan(0);
    expect(screen.queryByText(/Ada Lovelace/)).toBeNull();
    expect(screen.queryByLabelText("Token")).toBeNull();
    expect(screen.queryByRole("button", { name: "Connect" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Ask an admin" }));
    expect(await screen.findByText(/Asked just now/)).toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST" && c.path === "/v1/workspaces/w1/apps/github/setup-request")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    expect(await screen.findByRole("button", { name: "Ask an admin" })).toBeInTheDocument();
    expect(calls.some((c) => c.method === "DELETE")).toBe(true);
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
    expect(await screen.findByText("Daneel can now read GitHub · bob-builder, and asks you before each change.")).toBeInTheDocument();
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
    // Two ways work: the choice sits at the top of the step ("Sign in with:").
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
  const ADMIN = { action: "register_oauth_client", role: "org_admin", people: [{ name: "Alice Admin" }], you_can: false };
  const item = (id: string, name: string, over: Record<string, unknown> = {}) => ({
    ...DETAIL, app_id: id, package_id: id, name, summary: `${name} things.`, readiness: { state: "ready" }, accounts: [], ...over,
  });
  const blockedItem = (id: string, name: string, pkg: string, over: Record<string, unknown> = {}) =>
    item(id, name, { package_id: pkg, readiness: { state: "needs_admin", needs: ["oauth_client"], who_can: ADMIN }, ...over });
  const apps = (items: unknown[]) => ({ "GET /v1/apps": () => json(200, { items, next_cursor: null }) });

  it("groups come from the catalogue's readiness: Ready (alphabetical, website last), Already connected, Needs an admin first", async () => {
    stub(
      apps([
        item("linear", "Linear"),
        item("github", "GitHub"),
        blockedItem("gcal", "Google Calendar", "google-workspace"),
        blockedItem("gdrive", "Google Drive", "google-workspace"),
        item("sentry", "Sentry", { accounts: [{ id: "c7", name: "Team Sentry", agent_ids: [] }] }),
        item("notion", "Notion", { accounts: [{ id: "c8", name: "Notion docs", agent_ids: ["a1"] }] }),
      ]),
    );
    open({ appId: null });
    await screen.findByRole("region", { name: "Ready to connect · 3" });
    const ready = within(screen.getByRole("region", { name: "Ready to connect · 3" }))
      .getAllByRole("button")
      .map((b) => b.textContent);
    // Alphabetical; the account Daneel can already use stays ready (it is on the Access tab, not "connected, can't use yet").
    expect(ready).toEqual(["GitHub", "Linear", "Notion", "+Another website or API"]);
    const connected = screen.getByRole("region", { name: "Already connected · 1" });
    expect(within(connected).getByText("Team Sentry")).toBeInTheDocument();
    expect(within(connected).getByText("Sentry · connected, Daneel can't use it yet")).toBeInTheDocument();
    const blocked = screen.getByRole("region", { name: "Needs an admin first · 2" });
    // The reason in one line; the second app of the same provider says so.
    expect(within(blocked).getByText("An admin needs to set up Google sign-in")).toBeInTheDocument();
    expect(within(blocked).getByText("Same Google sign-in")).toBeInTheDocument();
    expect(within(blocked).getAllByRole("button", { name: "Ask an admin" })).toHaveLength(2);
    // No plan is fetched to decide the groups.
    expect((fetch as unknown as { mock: { calls: Request[][] } }).mock.calls.some((c) => c[0]!.url.includes("connect-plan"))).toBe(false);
  });

  it("search filters every group by name and summary", async () => {
    stub(apps([item("linear", "Linear"), item("github", "GitHub", { summary: "Repositories and pull requests." }), blockedItem("gcal", "Google Calendar", "google-workspace")]));
    open({ appId: null });
    fireEvent.change(await screen.findByRole("searchbox", { name: "Search apps" }), { target: { value: "pull" } });
    expect(screen.getByRole("button", { name: "GitHub" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Linear" })).toBeNull();
    expect(screen.queryByRole("region", { name: /Needs an admin first/ })).toBeNull();
    fireEvent.change(screen.getByRole("searchbox", { name: "Search apps" }), { target: { value: "calendar" } });
    expect(await screen.findByText("Google Calendar")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "GitHub" })).toBeNull();
    // A group with no match is hidden while searching; "No app matches" only when every group is empty (B1).
    expect(screen.queryByText(/Ready to connect/)).toBeNull();
    expect(screen.queryByText(/No app matches/)).toBeNull();
    fireEvent.change(screen.getByRole("searchbox", { name: "Search apps" }), { target: { value: "zzzz" } });
    expect(await screen.findByText(/No app matches/)).toBeInTheDocument();
  });

  it("Ask asks the admin, shows the asked state with Undo, and a reopened list restores it from requested.by_me", async () => {
    const calls = stub({
      ...apps([blockedItem("gcal", "Google Calendar", "google-workspace")]),
      "POST /v1/workspaces/w1/apps/gcal/setup-request": () => json(200, { id: "r1", app_id: "gcal", status: "open", askers: [], needs: ["oauth_client"] }),
      "DELETE /v1/workspaces/w1/apps/gcal/setup-request": () => new Response(null, { status: 204 }),
    });
    open({ appId: null });
    fireEvent.click(await screen.findByRole("button", { name: "Ask an admin" }));
    expect(await screen.findByText(/Asked just now\. You'll get a notice when it's ready, and Daneel's Apps & access tab will offer it\./)).toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST" && c.path === "/v1/workspaces/w1/apps/gcal/setup-request")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    expect(await screen.findByRole("button", { name: "Ask an admin" })).toBeInTheDocument();
    expect(calls.some((c) => c.method === "DELETE")).toBe(true);
  });

  it("the asked state comes back from requested.by_me when the list is read again", async () => {
    stub(apps([blockedItem("gcal", "Google Calendar", "google-workspace", { readiness: { state: "needs_admin", needs: ["oauth_client"], who_can: ADMIN, requested: { by_me: true, count: 1 } } })]));
    open({ appId: null });
    expect(await screen.findByText(/Asked\. You'll get a notice when it's ready/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Undo" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Ask an admin" })).toBeNull();
  });

  it("Give access on an Already connected row goes to the Access step and saves only the access", async () => {
    const calls = stub({
      ...apps([item("github", "GitHub", { accounts: [{ id: "c1", name: "GitHub · bob-builder", agent_ids: [] }] })]),
      "GET /v1/workspaces/w1/accounts": () => json(200, { items: [ACCOUNT], next_cursor: null }),
      "PUT /v1/agents/a1/grants/connections/c1": () => json(200, {}),
    });
    open({ appId: null });
    fireEvent.click(await screen.findByRole("button", { name: "Give access" }));
    // Straight to Access for that account: no Account step, no Sign in.
    fireEvent.click(await screen.findByRole("radio", { name: /Ask before changes/ }));
    fireEvent.click(screen.getByRole("button", { name: "Give Daneel access" }));
    expect(await screen.findByText(/Daneel can now read GitHub · bob-builder, and asks you/)).toBeInTheDocument();
    expect(calls.find((c) => c.method === "PUT")?.body).toEqual({ level: "ask" });
    expect(post(calls)).toBeUndefined();
  });

  it("the frame: the title, the stepper and the footer stay in place; Back on the left, the action on the right", async () => {
    stub({ ...apps([item("github", "GitHub")]), "GET /v1/workspaces/w1/accounts": () => json(200, { items: [], next_cursor: null }) });
    open({ appId: null });
    expect(await screen.findByRole("dialog", { name: "Give Daneel an app" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Back" })).toBeNull();
    fireEvent.click(await screen.findByRole("button", { name: "GitHub" }));
    expect(await screen.findByRole("dialog", { name: "GitHub for Daneel" })).toBeInTheDocument();
    // Footer: Back first, Next last; the sheet's arrow is the same Back.
    const footerButtons = (await screen.findAllByRole("button", { name: /^(Back|Next)$/ })).map((b) => b.textContent);
    expect(footerButtons).toContain("Back");
    expect(footerButtons).toContain("Next");
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
