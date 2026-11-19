// (FIX-154, D-306(c), C-897) Credential-free apps: the agent's Access tab lists them with Give access (saved through
// PUT …/grants/apps/{app}).
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AccountsList } from "@/components/accounts/accounts-list";
import { wrap } from "./support/render";
import { json } from "./support/api";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useSearchParams: () => new URLSearchParams(""),
}));

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

const account = (appId: string) => ({
  ref: { type: "connection", id: CONN }, kind: "app", name: "CERN directory", scope: "workspace", levels: ["read", "ask", "act"],
  app: { app_id: appId, name: "CERN Phonebook", custom_only: false }, health: { state: "unknown" }, agents: [], given: false,
  can: { give_access: true, change_access: true, manage: true },
});
function setupAccess(accounts: unknown[] = []) {
  routes[`GET /v1/workspaces/${WS}/accounts`] = () => json(200, { items: accounts, next_cursor: null });
  routes[`GET /v1/workspaces/${WS}/agents`] = () => json(200, { items: [], next_cursor: null });
  routes["GET /v1/apps"] = () => json(200, { items: [summary()] });
  routes["GET /v1/apps/phonebook"] = () => json(200, detail());
}
const tab = (canOwn = true) => (
  <AccountsList scope={{ kind: "workspace", id: WS }} orgId={ORG} canOwn={canOwn} agent={{ id: AGENT, name: "Scout" }} />
);

describe("Access tab", () => {
  it("lists a credential-free app with no account, with a badge, and Give <Agent> access", async () => {
    setupAccess();
    wrap(tab());
    const list = await screen.findByRole("list", { name: "Apps that need no sign-in" });
    expect(within(list).getByText("CERN Phonebook")).toBeTruthy();
    expect(within(list).getByText("No sign-in needed")).toBeTruthy();
    expect(within(list).getByRole("button", { name: "Give Scout access to CERN Phonebook" })).toBeTruthy();
  });

  it("is not listed once the workspace has an account of the app (it is the ordinary row then)", async () => {
    setupAccess([account("phonebook")]);
    wrap(tab());
    await screen.findByText("Not given to Scout (1)");
    expect(screen.queryByText("No sign-in needed")).toBeNull();
  });

  it("is not listed when it is unavailable or needs a sign-in, or to people who cannot give access", async () => {
    setupAccess();
    routes["GET /v1/apps"] = () => json(200, { items: [summary({ available: false }), summary({ app_id: "gh", name: "GitHub", credential_free: false })] });
    wrap(tab());
    await screen.findByText("Nothing is given yet");
    expect(screen.queryByText("No sign-in needed")).toBeNull();
    cleanup();
    setupAccess();
    wrap(tab(false));
    await screen.findByText("Nothing is given yet");
    expect(screen.queryByText("No sign-in needed")).toBeNull();
  });

  it("Give access opens the grant form with Full preselected and saves through PUT …/grants/apps/{app}", async () => {
    setupAccess();
    routes[`PUT /v1/agents/${AGENT}/grants/apps/phonebook`] = (_r, body) =>
      json(200, {
        connection: conn("phonebook"),
        created: true,
        grant: { id: "g1", agent_id: AGENT, connection_id: CONN, created_at: now, allow_tools: [], ask_tools: [], ask_timeout_secs: null, app_restrictions: { custom_only: false, block_untooled: false }, ...(body as object) },
      });
    wrap(tab());
    fireEvent.click(await screen.findByRole("button", { name: "Give Scout access to CERN Phonebook" }));
    const dlg = await screen.findByRole("dialog");
    const full = (await within(dlg).findByRole("radio", { name: "Full access" })) as HTMLInputElement;
    expect(full.checked).toBe(true);
    fireEvent.click(within(dlg).getByRole("button", { name: "Save access" }));
    await waitFor(() => expect(count(`PUT /v1/agents/${AGENT}/grants/apps/phonebook`)).toBe(1));
    expect(calls.find((c) => c.key.startsWith("PUT"))!.body).toMatchObject({ mode: "full" });
    // the accounts list is refreshed
    await waitFor(() => expect(count(`GET /v1/workspaces/${WS}/accounts`)).toBeGreaterThan(1));
  });
});

