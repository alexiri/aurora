// (P5-UX-9, C-932, C-933, C-946) The per-app setup page in its two views, and the links from the lists to it.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const params = vi.hoisted(() => ({ value: {} as Record<string, string> }));
vi.mock("next/navigation", () => ({
  useParams: () => params.value,
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  usePathname: () => "/",
  useSearchParams: () => new URLSearchParams(),
}));

import { OrgAppSetupPage, WorkspaceAppSetupPage } from "@/components/apps/app-setup/app-setup-page";
import { AppsTab } from "@/components/apps/apps-tab";
import { AppLink } from "@/components/apps/app-link";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const calls: { method: string; path: string; body?: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input as Request;
      const url = new URL(req.url, "http://localhost");
      const body = req.method === "POST" || req.method === "PUT" ? await req.clone().json().catch(() => undefined) : undefined;
      calls.push({ method: req.method, path: url.pathname, body });
      const h = routes[`${req.method} ${url.pathname}`];
      return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return calls;
}
const wrap = (ui: React.ReactElement) => {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
};
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  params.value = {};
});

const contacts = { owners: [{ id: "u1", name: "Bob Builder" }], admins: [{ id: "u2", name: "Ada Lovelace" }, { id: "u3", name: "Alan Turing" }] };
const org = { id: "o1", slug: "acme", name: "Users", my_role: "member", contacts };
const workspace = (role: string) => ({ id: "w1", org_id: "o1", slug: "ops", name: "Ops", my_role: role, contacts });
const WHO_ORG_ADMIN = {
  action: "register_oauth_client", role: "org_admin", you_can: false,
  people: [{ id: "u2", name: "Ada Lovelace" }, { id: "u3", name: "Alan Turing" }],
};
const can = (over: Record<string, boolean> = {}) => ({
  register_workspace_client: true, register_org_client: false, register_instance: false, set_availability: false,
  connect_workspace: true, connect_org: false, ...over,
});

const detail = (id: string, name: string, extra: Record<string, unknown> = {}) => ({
  app_id: id, package_id: id, name, description: `${name} for agents.`, available: true, has_dark_icon: false,
  connect: [], credentials: [], hosts: [{ host: `api.${id}.example` }], metadata: [], connection_count: 0, credential_free: false,
  serves_library: false, app_restrictions: { custom_only: false, block_untooled: false },
  oauth_clients: [{
    client_key: id, used_by_methods: [{ id: "oauth", label: `Sign in with ${name}` }], setup_note: null,
    redirect_uri: "https://aurora.test/v1/oauth/callback",
    fields: [{ name: "client_id", label: "Client ID", secret: false }, { name: "client_secret", label: "Client secret", secret: true }],
  }],
  tools: [
    { id: "search", name: "Search issues", group: "read", description: "Find issues by text." },
    { id: "create", name: "Create an issue", group: "write", description: null },
  ],
  ...extra,
});

const method = (over: Record<string, unknown>) => ({
  audience: "person", needs: [], ready: true, recommended: false, scope_presets: { read: [], write: [] }, ...over,
});

function workspaceRoutes(state: { registered: boolean }, posted: unknown[]): Record<string, Handler> {
  return {
    "GET /v1/resolve": () => json(200, { org, workspace: workspace("owner") }),
    "GET /v1/workspaces/w1": () => json(200, workspace("owner")),
    "GET /v1/orgs/o1": () => json(200, org),
    "GET /v1/apps/linear": () => json(200, detail("linear", "Linear")),
    "GET /v1/apps/linear/setup": () =>
      json(200, {
        app_id: "linear", name: "Linear", available: true, suggested_method: "key", instances: [],
        values: { callback_url: "https://aurora.test/v1/oauth/callback" },
        can: can(),
        clients: {
          workspace: state.registered ? [{ id: "c1", client_key: "linear", enabled: true, used_by_methods: [{ id: "oauth", label: "Sign in with Linear" }] }] : [],
          organization: [], instance: { set_up: false },
        },
        methods: [
          method({ id: "key", kind: "fields", label: "API key" }),
          state.registered
            ? method({ id: "oauth", kind: "oauth2", label: "Sign in with Linear", client_key: "linear" })
            : method({ id: "oauth", kind: "oauth2", label: "Sign in with Linear", client_key: "linear", ready: false, needs: ["oauth_client"], who_can: WHO_ORG_ADMIN }),
        ],
      }),
    "GET /v1/apps/linear/guide": () => new Response("1. In Linear, create an OAuth application.", { status: 200, headers: { "content-type": "text/markdown" } }),
    "GET /v1/workspaces/w1/accounts": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/workspaces/w1/oauth-clients": () =>
      json(200, {
        items: state.registered
          ? [{ id: "c1", scope: "workspace", package_id: "linear", client_key: "linear", fields: { client_id: "abc", client_secret: "" }, enabled: true, connection_count: 0, created_at: "", updated_at: "" }]
          : [],
      }),
    "POST /v1/workspaces/w1/oauth-clients": async (req) => {
      posted.push(await req.json());
      state.registered = true;
      return json(201, { id: "c1" });
    },
  };
}

describe("workspace view (C-932)", () => {
  it("shows a sentence and chip per way to connect, offers the workspace's own fix, names who can do the rest, and turns Ready after registering", async () => {
    params.value = { org: "acme", ws: "ops", app: "linear" };
    const state = { registered: false };
    const posted: unknown[] = [];
    mockFetch(workspaceRoutes(state, posted));
    wrap(<WorkspaceAppSetupPage />);

    expect(await screen.findByRole("heading", { level: 1, name: /Linear/ })).toBeInTheDocument();
    expect(screen.getByTestId("setup-headline")).toHaveTextContent("People here can connect Linear with an API key today. Sign in with Linear needs setup first.");
    const ways = screen.getByTestId("setup-ways");
    expect(within(ways).getByText("Ready")).toBeInTheDocument();
    expect(within(ways).getByText("Needs setup")).toBeInTheDocument();
    expect(within(ways).getByText(/needs an OAuth client: none is set up for this workspace, Users or Aurora\./)).toBeInTheDocument();
    // what the viewer cannot do says who can, by name; what they can do is a button
    expect(within(ways).getByText(/Ask Ada Lovelace or Alan Turing/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connect Linear" })).toBeInTheDocument();
    // the callback URL is shown with Copy; the tool descriptions and the guide are on the page
    expect(within(screen.getByTestId("setup-values")).getByRole("button", { name: /Copy/ })).toBeInTheDocument();
    expect(screen.getByText(/Find issues by text\./)).toBeInTheDocument();
    expect(await screen.findByText("In Linear, create an OAuth application.")).toBeInTheDocument();

    fireEvent.click(within(ways).getByRole("button", { name: "Register an OAuth client for this workspace" }));
    const dlg = await screen.findByRole("dialog", { name: "Register an OAuth client" });
    // the app and its client are chosen, named by the way it is used, and the callback URL is shown
    expect(within(dlg).getByLabelText("App")).toHaveValue("linear|linear");
    expect(within(dlg).getByRole("option", { name: "Linear — for Sign in with Linear" })).toBeInTheDocument();
    expect(within(dlg).getByText("https://aurora.test/v1/oauth/callback")).toBeInTheDocument();
    fireEvent.change(within(dlg).getByLabelText("Client ID"), { target: { value: "abc" } });
    fireEvent.change(within(dlg).getByLabelText("Client secret"), { target: { value: "s3cret" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Add" }));

    await waitFor(() => expect(posted).toEqual([{ package_id: "linear", client_key: "linear", fields: { client_id: "abc", client_secret: "s3cret" } }]));
    await waitFor(() => expect(screen.queryByText("Needs setup")).toBeNull());
    expect(screen.getByTestId("setup-headline")).toHaveTextContent("People here can connect Linear with an API key or Sign in with Linear today.");
    expect(within(screen.getByTestId("setup-ways")).getAllByText("Ready")).toHaveLength(2);
  });

  it("a member is told who can set apps up, with no page of controls", async () => {
    params.value = { org: "acme", ws: "ops", app: "linear" };
    mockFetch({
      ...workspaceRoutes({ registered: false }, []),
      "GET /v1/resolve": () => json(200, { org, workspace: workspace("member") }),
      "GET /v1/workspaces/w1": () => json(200, workspace("member")),
    });
    wrap(<WorkspaceAppSetupPage />);
    expect(await screen.findByText(/Only workspace owners can/)).toBeInTheDocument();
    expect(screen.queryByTestId("setup-ways")).toBeNull();
  });
});

describe("organization view (C-932)", () => {
  it("AWS shows the organization's external id with Copy and the organization's own actions", async () => {
    params.value = { org: "acme", app: "aws-keys" };
    mockFetch({
      "GET /v1/resolve": () => json(200, { org: { ...org, my_role: "owner" } }),
      "GET /v1/orgs/o1": () => json(200, { ...org, my_role: "owner" }),
      "GET /v1/apps/aws-keys": () => json(200, detail("aws-keys", "AWS", { oauth_clients: [] })),
      "GET /v1/apps/aws-keys/setup": () =>
        json(200, {
          app_id: "aws-keys", name: "AWS", available: true, instances: [], suggested_method: "keys",
          values: { callback_url: "https://aurora.test/v1/oauth/callback", external_id: "ext-1234" },
          can: can({ register_workspace_client: false, register_org_client: true, set_availability: true, connect_org: true }),
          clients: { workspace: null, organization: [], instance: { set_up: false } },
          methods: [method({ id: "keys", kind: "fields", label: "Access keys" })],
        }),
      "GET /v1/apps/aws-keys/guide": () => json(404, { error: { code: "not_found", message: "nf" } }),
      "GET /v1/orgs/o1/accounts": () => json(200, { items: [], next_cursor: null }),
    });
    wrap(<OrgAppSetupPage />);
    const values = await screen.findByTestId("setup-values");
    expect(within(values).getByText("ext-1234")).toBeInTheDocument();
    expect(within(values).getByRole("button", { name: /Copy/ })).toBeInTheDocument();
    // no OAuth client means no callback URL to copy
    expect(within(values).queryByText("https://aurora.test/v1/oauth/callback")).toBeNull();
    expect(screen.getByRole("link", { name: "Change availability" })).toHaveAttribute("href", "/o/acme/apps?tab=apps#availability");
    expect(screen.getByRole("button", { name: "Connect AWS" })).toBeInTheDocument();
    expect(screen.getByTestId("org-app-setup")).toBeInTheDocument();
  });

  it("GitHub's page shows the organization's clients by the way they are used, and the self-hosted link carries the package", async () => {
    params.value = { org: "acme", app: "gitlab" };
    mockFetch({
      "GET /v1/resolve": () => json(200, { org: { ...org, my_role: "admin" } }),
      "GET /v1/orgs/o1": () => json(200, { ...org, my_role: "admin" }),
      "GET /v1/apps/gitlab": () => json(200, detail("gitlab", "GitLab", { package_id: "gitlab-pkg" })),
      "GET /v1/apps/gitlab/setup": () =>
        json(200, {
          app_id: "gitlab", name: "GitLab", available: true, instances: [], suggested_method: null,
          values: { callback_url: "https://aurora.test/v1/oauth/callback" },
          can: can({ register_workspace_client: false, register_org_client: true, register_instance: true, connect_org: true }),
          clients: { workspace: null, organization: [{ id: "c9", client_key: "gitlab", enabled: true, used_by_methods: [{ id: "oauth", label: "Sign in with GitLab" }] }], instance: { set_up: true } },
          methods: [
            method({ id: "token", kind: "fields", label: "Personal access token", ready: false, needs: ["instance"] }),
            method({ id: "oauth", kind: "oauth2", label: "Sign in with GitLab", client_key: "gitlab" }),
          ],
        }),
      "GET /v1/apps/gitlab/guide": () => json(404, { error: { code: "not_found", message: "nf" } }),
      "GET /v1/orgs/o1/accounts": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/orgs/o1/oauth-clients": () => json(200, { items: [] }),
    });
    wrap(<OrgAppSetupPage />);
    const ways = await screen.findByTestId("setup-ways");
    expect(within(ways).getByText("Needs setup")).toBeInTheDocument();
    expect(within(ways).getByRole("link", { name: "Register a self-hosted app" })).toHaveAttribute("href", "/o/acme/apps?tab=self-hosted&package=gitlab-pkg");
    expect(within(screen.getByTestId("setup-where")).getByText("GitLab — for Sign in with GitLab")).toBeInTheDocument();
    expect(within(screen.getByTestId("setup-where")).getByText("Set up")).toBeInTheDocument();
  });
});

describe("links to the setup page (C-933)", () => {
  it("an app name links to the page at the viewer's level; Administration opens the catalogue entry", () => {
    const app = { app_id: "linear", package_id: "linear-pkg" };
    params.value = { org: "acme", ws: "ops" };
    const { unmount } = wrap(<AppLink app={app}>Linear</AppLink>);
    expect(screen.getByRole("link", { name: "Linear" })).toHaveAttribute("href", "/o/acme/w/ops/apps/linear");
    unmount();
    params.value = { org: "acme" };
    const second = wrap(<AppLink app={app}>Linear</AppLink>);
    expect(screen.getByRole("link", { name: "Linear" })).toHaveAttribute("href", "/o/acme/apps/linear");
    second.unmount();
    params.value = {};
    wrap(<AppLink app={app}>Linear</AppLink>);
    expect(screen.getByRole("link", { name: "Linear" })).toHaveAttribute("href", "/admin/apps/packages/linear-pkg");
  });

  it("the workspace Apps tab's Details goes to the setup page and no dialog opens", async () => {
    params.value = { org: "acme", ws: "ops" };
    mockFetch({
      "GET /v1/apps": () =>
        json(200, { items: [{ app_id: "linear", package_id: "linear", package_version: "1", name: "Linear", description: "Issues", available: true, connection_count: 2, has_dark_icon: false, serves_library: false, credential_free: false, app_restrictions: { custom_only: false, block_untooled: false } }] }),
    });
    wrap(<AppsTab workspaceId="w1" orgId="o1" canOwn />);
    const details = await screen.findByRole("link", { name: "Details of Linear" });
    expect(details).toHaveAttribute("href", "/o/acme/w/ops/apps/linear");
    expect(screen.getByRole("link", { name: "Linear" })).toHaveAttribute("href", "/o/acme/w/ops/apps/linear");
    // Details is a link now (no dialog to open)
    expect(screen.queryByRole("button", { name: "Details of Linear" })).toBeNull();
  });
});
