// (FIX-154, D-306(c), C-897) Credential-free apps: the agent's Access tab lists them with Give access (saved through
// PUT …/grants/apps/{app}); the connect dialog has no form and connects and grants in one action.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConnectionGrants } from "@/components/agents/connection-grants";
import { ConnectDialog } from "@/components/apps/connect-dialog";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useSearchParams: () => new URLSearchParams(""),
}));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request, body: unknown) => Response | Promise<Response>;
let calls: { key: string; body: unknown }[];
let routes: Record<string, Handler>;
const count = (key: string) => calls.filter((c) => c.key === key).length;

beforeEach(() => {
  calls = [];
  routes = {};
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

const now = "2026-01-01T00:00:00Z";
const AGENT = "a0000000-0000-4000-8000-000000000011";
const WS = "a0000000-0000-4000-8000-000000000002";
const ORG = "a0000000-0000-4000-8000-000000000001";
const CONN = "c0000000-0000-4000-8000-000000000050";

const tools = [
  { id: "search", name: "Search the directory", group: "read", host: "x.ldap.aurora.internal", path: "/search", methods: ["POST"], connect_methods: [], scopes: [], wildcard: false },
];
const summary = (over: Record<string, unknown> = {}) => ({
  app_id: "phonebook", name: "CERN Phonebook", package_id: "cern", package_version: "1.0.0", available: true, connection_count: 0,
  description: "People lookup.", has_dark_icon: false, credential_free: true, app_restrictions: { custom_only: false, block_untooled: false }, ...over,
});
const detail = (over: Record<string, unknown> = {}) => ({
  ...summary(), connect: [{ id: "anonymous", kind: "fields", fields: [], scopes: [], probe_scopes: [], access_levels: [], default_scopes: [] }],
  connect_note: { text: "Nothing to enter: the directory is read anonymously.", url: null }, credentials: [], hosts: [], metadata: [],
  oauth_clients: [], tools, ...over,
});
const conn = (appId: string) => ({
  app_id: appId, app_version: "1.0.0", created_at: now, id: CONN, injecting: [], label: "CERN directory", metadata: {}, missing_scopes: [], org_id: ORG,
  package_id: "cern", scope: "workspace", scopes: [], status: "active", updated_at: now, workspace_id: WS,
});

function setupAccess(connections: unknown[] = []) {
  routes[`GET /v1/workspaces/${WS}/connections`] = () => json(200, { items: connections });
  routes[`GET /v1/orgs/${ORG}/connections`] = () => json(200, { items: [] });
  routes[`GET /v1/agents/${AGENT}/grants`] = () => json(200, { secrets: [], connections: [], converted_by_app: [] });
  routes["GET /v1/apps"] = () => json(200, { items: [summary()] });
  routes["GET /v1/apps/phonebook"] = () => json(200, detail());
}

describe("Access tab", () => {
  it("lists a credential-free app with no connection, with a badge, instead of the empty state", async () => {
    setupAccess();
    wrap(<ConnectionGrants agentId={AGENT} wsId={WS} orgId={ORG} />);
    const list = await screen.findByRole("list", { name: "Connection grants" });
    expect(within(list).getByText("CERN Phonebook")).toBeTruthy();
    expect(within(list).getByText("No sign-in needed")).toBeTruthy();
    expect(screen.queryByText("No connections in scope")).toBeNull();
    expect(screen.getByText(/Apps that need no sign-in can be given directly/)).toBeTruthy();
  });

  it("is not listed once the scope has a connection of the app (it is the ordinary row then)", async () => {
    setupAccess([conn("phonebook")]);
    wrap(<ConnectionGrants agentId={AGENT} wsId={WS} orgId={ORG} />);
    const list = await screen.findByRole("list", { name: "Connection grants" });
    expect(within(list).queryByText("No sign-in needed")).toBeNull();
    expect(within(list).getByText("CERN directory")).toBeTruthy();
  });

  it("is not listed when it is unavailable or needs a sign-in", async () => {
    setupAccess();
    routes["GET /v1/apps"] = () => json(200, { items: [summary({ available: false }), summary({ app_id: "gh", name: "GitHub", credential_free: false })] });
    wrap(<ConnectionGrants agentId={AGENT} wsId={WS} orgId={ORG} />);
    expect(await screen.findByText("No connections in scope")).toBeTruthy();
  });

  it("Give access opens the grant form with Full preselected and saves through PUT …/grants/apps/{app}", async () => {
    setupAccess();
    routes[`PUT /v1/agents/${AGENT}/grants/apps/phonebook`] = (_r, body) =>
      json(200, {
        connection: conn("phonebook"),
        created: true,
        grant: { id: "g1", agent_id: AGENT, connection_id: CONN, created_at: now, allow_tools: [], ask_tools: [], ask_timeout_secs: null, app_restrictions: { custom_only: false, block_untooled: false }, ...(body as object) },
      });
    wrap(<ConnectionGrants agentId={AGENT} wsId={WS} orgId={ORG} />);
    fireEvent.click(await screen.findByRole("button", { name: "Give access to CERN Phonebook" }));
    const dlg = await screen.findByRole("dialog");
    const full = (await within(dlg).findByRole("radio", { name: "Full access" })) as HTMLInputElement;
    expect(full.checked).toBe(true);
    fireEvent.click(within(dlg).getByRole("button", { name: "Save access" }));
    await waitFor(() => expect(count(`PUT /v1/agents/${AGENT}/grants/apps/phonebook`)).toBe(1));
    expect(calls.find((c) => c.key.startsWith("PUT"))!.body).toMatchObject({ mode: "full" });
    // the grants, connections and apps lists are refreshed
    await waitFor(() => expect(count(`GET /v1/agents/${AGENT}/grants`)).toBeGreaterThan(1));
  });
});

describe("Connect dialog", () => {
  const open = () => wrap(<ConnectDialog appId="phonebook" workspaceId={WS} orgId={ORG} onClose={() => {}} />);
  function setupConnect() {
    routes["GET /v1/apps"] = () => json(200, { items: [summary()] });
    routes["GET /v1/apps/phonebook"] = () => json(200, detail());
    routes[`GET /v1/workspaces/${WS}/agents`] = () =>
      json(200, { items: [{ id: AGENT, handle: "scout", name: "Scout", kind: "hosted", frozen: null }], next_cursor: null });
    routes["POST /v1/apps/phonebook/connect"] = () =>
      json(200, { status: "connected", connection: conn("phonebook"), missing_scopes: [] });
  }

  it("has no form: the note, an agent picker and one action; with no agent chosen it only connects", async () => {
    setupConnect();
    open();
    await screen.findByText(/Nothing to enter/);
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.getByText("Give agents access")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await screen.findByText(/CERN Phonebook is connected/);
    expect(count("POST /v1/apps/phonebook/connect")).toBe(1);
    expect(calls.some((c) => c.key.startsWith("PUT"))).toBe(false);
  });

  it("with an agent chosen it gives access through the app grant route (which creates the connection)", async () => {
    setupConnect();
    routes[`PUT /v1/agents/${AGENT}/grants/apps/phonebook`] = () =>
      json(200, {
        connection: conn("phonebook"), created: true,
        grant: { id: "g1", agent_id: AGENT, connection_id: CONN, created_at: now, mode: "full", allow_tools: [], ask_tools: [], ask_timeout_secs: null, app_restrictions: { custom_only: false, block_untooled: false } },
      });
    open();
    fireEvent.click(await screen.findByRole("checkbox", { name: /Scout/ }));
    fireEvent.click(screen.getByRole("button", { name: "Connect and give access" }));
    await screen.findByText(/CERN Phonebook is connected and 1 agent has access/);
    expect(count(`PUT /v1/agents/${AGENT}/grants/apps/phonebook`)).toBe(1);
    expect(count("POST /v1/apps/phonebook/connect")).toBe(0);
  });

  it("a failed grant says so and keeps the connection", async () => {
    setupConnect();
    routes[`PUT /v1/agents/${AGENT}/grants/apps/phonebook`] = () => json(403, { error: { code: "forbidden", message: "No." } });
    open();
    fireEvent.click(await screen.findByRole("checkbox", { name: /Scout/ }));
    fireEvent.click(screen.getByRole("button", { name: "Connect and give access" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Could not give access to Scout");
    expect(alert.textContent).toContain("Access tab");
  });
});
