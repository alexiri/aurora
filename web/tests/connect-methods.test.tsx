import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ConnectView } from "@/components/apps/connect-view";
import { toolsForMethod, type Tool } from "@/lib/grant-editor";

// FIX-123 (D-195(a), D-214): one app per service; the ways to authenticate are connect methods, and the connect
// window asks "How do you want to connect?" with a description and a Recommended badge for each.

let search = "";
vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main", app: "github" }),
  useSearchParams: () => new URLSearchParams(search),
}));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  search = "";
  vi.unstubAllGlobals();
});

interface Call {
  method: string;
  path: string;
  body: unknown;
}

function stub(routes: Record<string, (c: Call) => Response>) {
  const calls: Call[] = [];
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

const method = (over: Record<string, unknown>) => ({
  kind: "fields",
  label: null,
  description: null,
  recommended: false,
  fields: [],
  scopes: [],
  default_scopes: [],
  access_levels: [],
  client_key: null,
  callback: null,
  ...over,
});

const field = (name: string, label: string) => ({ name, label, type: "secret", store: "credentials", required: true });

const APP = {
  app_id: "github",
  package_id: "github",
  package_version: "2.0.0",
  name: "GitHub",
  description: "d",
  category: "development",
  available: true,
  connection_count: 0,
  has_dark_icon: false,
  api_docs: null,
  connect_note: null,
  label_hint: null,
  connect: [
    method({
      id: "github_app",
      kind: "hook",
      label: "GitHub App",
      description: "An app installed on the repositories you choose. Recommended for teams.",
      recommended: true,
      fields: [field("private_key", "Private key")],
    }),
    method({ id: "token", label: "Personal access token", description: "Acts as you.", fields: [field("access_token", "Personal access token field")] }),
    method({ id: "oauth", kind: "oauth2", label: "OAuth", description: "Sign in with GitHub.", client_key: "github", callback: "query" }),
  ],
  tools: [],
  hosts: [],
  agent_instructions: null,
  credentials: [],
  metadata: [],
  oauth_clients: [],
  instances: null,
};

const routes = (extra: Record<string, (c: Call) => Response> = {}) => ({
  "GET /v1/resolve": () => json(200, { org: { id: "o1", name: "Acme", slug: "acme" }, workspace: { id: "w1", name: "Main", slug: "main" } }),
  "GET /v1/apps/github": () => json(200, APP),
  "GET /v1/apps/github/guide": () => json(404, { error: { code: "not_found", message: "x" } }),
  "GET /v1/workspaces/w1/oauth-clients": () => json(200, { items: [], next_cursor: null }),
  ...extra,
});

describe("connect window: How do you want to connect?", () => {
  it("shows each method with its description and a single Recommended badge, and preselects the recommended one", async () => {
    stub(routes());
    wrap(<ConnectView />);
    const group = await screen.findByRole("radiogroup", { name: "How do you want to connect?" });
    const radios = within(group).getAllByRole("radio");
    expect(radios.map((r) => r.closest("label")?.textContent)).toEqual([
      "GitHub AppRecommendedAn app installed on the repositories you choose. Recommended for teams.",
      "Personal access tokenActs as you.",
      "OAuthSign in with GitHub.",
    ]);
    expect(within(group).getAllByText("Recommended")).toHaveLength(1);
    expect(radios.map((r) => (r as HTMLInputElement).checked)).toEqual([true, false, false]);
    // the recommended method's own form is the one shown
    expect(screen.getByLabelText("Private key")).toBeInTheDocument();
    expect(screen.queryByLabelText("Personal access token field")).toBeNull();
  });

  it("switching the method shows that method's form and sends its id", async () => {
    const calls = stub(
      routes({ "POST /v1/apps/github/connect": () => json(200, { kind: "connected", connection: { id: "k1" }, missing_scopes: [] }) }),
    );
    wrap(<ConnectView />);
    fireEvent.click(await screen.findByRole("radio", { name: /Personal access token/ }));
    fireEvent.change(await screen.findByLabelText("Personal access token field"), { target: { value: "ghp_x" } });
    expect(screen.queryByLabelText("Private key")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/apps/github/connect")).toBe(true));
    expect(calls.find((c) => c.path === "/v1/apps/github/connect")?.body).toMatchObject({ method_id: "token", fields: { access_token: "ghp_x" } });
  });

  it("a method without a label falls back to what kind of method it is", async () => {
    stub(routes({ "GET /v1/apps/github": () => json(200, { ...APP, connect: [method({ id: "a", kind: "oauth2", client_key: "x" }), method({ id: "b" })] }) }));
    wrap(<ConnectView />);
    const group = await screen.findByRole("radiogroup", { name: "How do you want to connect?" });
    expect(within(group).getAllByRole("radio").map((r) => r.closest("label")?.textContent)).toEqual(["Sign in", "Access token or keys"]);
  });

  it("a single-method app shows no chooser", async () => {
    stub(routes({ "GET /v1/apps/github": () => json(200, { ...APP, connect: [APP.connect[1]] }) }));
    wrap(<ConnectView />);
    await screen.findByLabelText("Personal access token field");
    expect(screen.queryByRole("radiogroup")).toBeNull();
  });

  it("a reconnect opens on the method the connection was made with", async () => {
    search = "reconnect=c1";
    stub(
      routes({
        "GET /v1/workspaces/w1/connections": () =>
          json(200, { items: [{ id: "c1", app_id: "github", connect_method: "token" }], next_cursor: null }),
      }),
    );
    wrap(<ConnectView />);
    expect(await screen.findByLabelText("Personal access token field")).toBeInTheDocument();
    const radios = screen.getAllByRole("radio") as HTMLInputElement[];
    expect(radios.map((r) => r.checked)).toEqual([false, true, false]);
  });
});

describe("tools restricted to a connect method", () => {
  const tool = (id: string, connect_methods: string[]): Tool => ({
    id,
    name: id,
    group: "read",
    host: "api.github.com",
    path: `/${id}`,
    methods: ["GET"],
    connect_methods,
    wildcard: false,
  });
  const all = [tool("get_repo", []), tool("get_authenticated_user", ["token", "oauth"]), tool("list_installation_repos", ["github_app"])];

  it("are hidden from the pickers of connections made with other methods", () => {
    expect(toolsForMethod(all, "github_app").map((t) => t.id)).toEqual(["get_repo", "list_installation_repos"]);
    expect(toolsForMethod(all, "oauth").map((t) => t.id)).toEqual(["get_repo", "get_authenticated_user"]);
  });

  it("hide nothing for a connection with no recorded method", () => {
    expect(toolsForMethod(all, null)).toHaveLength(3);
    expect(toolsForMethod(all, undefined)).toHaveLength(3);
  });
});
