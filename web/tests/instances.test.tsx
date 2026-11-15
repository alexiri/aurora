import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ConnectDialog } from "@/components/apps/connect-dialog";
import { ConnectionsTab } from "@/components/apps/connections-tab";
import { InstancesView } from "@/components/apps/instances-view";
import { OAuthClientsPanel } from "@/components/apps/oauth-clients-panel";

// The connect flow is a dialog now (D-272).
const ConnectView = () => <ConnectDialog appId="desk" workspaceId="w1" orgId="o1" onClose={() => {}} />;

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main", app: "desk" }),
  useSearchParams: () => new URLSearchParams(),
}));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const APP = {
  app_id: "desk",
  package_id: "selfhost",
  package_version: "1.0.0",
  name: "Desk",
  description: "d",
  category: "dev",
  available: true,
  connection_count: 0,
  has_dark_icon: false,
  api_docs: null,
  connect_note: null,
  label_hint: null,
  connect: [
    { id: "token", kind: "fields", fields: [{ name: "api_token", label: "Token", type: "secret", store: "credentials", required: true }], scopes: [], default_scopes: [], access_levels: [], client_key: null, callback: null },
    { id: "oauth", kind: "oauth2", fields: [], scopes: [], default_scopes: [], access_levels: [], client_key: "oa", callback: "query" },
  ],
  tools: [],
  hosts: [],
  agent_instructions: null,
  credentials: [],
  metadata: [],
  oauth_clients: [{ client_key: "oa", fields: [{ name: "client_id", label: "Application ID", secret: false }, { name: "client_secret", label: "Secret", secret: true }], setup_note: null, redirect_uri: "https://a.test/v1/oauth/callback" }],
  instances: { label: "Desk instance", default_host: "saas.desk.example" },
};
const PLAIN = { ...APP, app_id: "plain", name: "Plain", instances: null, oauth_clients: [], package_id: "plainpkg" };

const inst = (over: Record<string, unknown>) => {
  const r = {
    id: "i1",
    org_id: "o1",
    package_id: "selfhost",
    app_id: "desk",
    host: "desk.acme.example",
    base_path: null as string | null,
    label: "Acme desk",
    status: "active",
    created_by: null,
    created_at: "",
    updated_at: "",
    connections: 2,
    builtin: false,
    ...over,
  };
  // (C-731) Registrations are named by `https://<host><base_path>`.
  return { base_url: `https://${r.host}${r.base_path ?? ""}`, ...r };
};

interface Call {
  method: string;
  path: string;
  body: unknown;
}

/** Route table over the stubbed fetch; records every call. */
function stub(routes: Record<string, (c: Call) => Response>) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url);
      const c: Call = {
        method: input.method,
        path: url.pathname,
        body: input.method === "GET" ? null : await input.json().catch(() => null),
      };
      calls.push(c);
      const h = routes[`${c.method} ${c.path}`];
      return h ? h(c) : json(404, { error: { code: "not_found", message: "x" } });
    }),
  );
  return calls;
}

const catalogue = {
  "GET /v1/apps": () => json(200, { items: [APP, PLAIN] }),
  "GET /v1/apps/desk": () => json(200, APP),
  "GET /v1/apps/plain": () => json(200, PLAIN),
};

describe("instances admin section", () => {
  it("lists instances with their scope and connections; instance-wide ones are read-only in an organization", async () => {
    stub({
      ...catalogue,
      "GET /v1/orgs/o1/app-instances": () =>
        json(200, { items: [inst({}), inst({ id: "i2", org_id: null, host: "desk.wide.example", label: "Wide", connections: 0 })], next_cursor: null }),
    });
    wrap(<InstancesView scope={{ kind: "organization", id: "o1" }} canManage />);
    const table = await screen.findByRole("table", { name: "Registered self-hosted apps" });
    expect(within(table).getByText("https://desk.acme.example")).toBeInTheDocument();
    expect(within(table).getByText("Organization")).toBeInTheDocument();
    expect(within(table).getByText("Instance-wide")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete https://desk.acme.example" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete https://desk.wide.example" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Disable https://desk.wide.example" })).toBeNull();
  });

  it("a package's built-in instance is badged and read-only, even for an admin (FIX-134)", async () => {
    stub({
      ...catalogue,
      "GET /v1/orgs/o1/app-instances": () =>
        json(200, { items: [inst({ id: "r-us", org_id: null, host: "us.desk.example", label: "US region", builtin: true, connections: 1 })], next_cursor: null }),
    });
    wrap(<InstancesView scope={{ kind: "organization", id: "o1" }} canManage />);
    const table = await screen.findByRole("table", { name: "Registered self-hosted apps" });
    expect(within(table).getByText("https://us.desk.example")).toBeInTheDocument();
    expect(within(table).getByText("Built in")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Delete|Disable|Enable|instance-wide/ })).toBeNull();
  });

  it("registers an instance (label defaults to the host) and shows the server's refusal", async () => {
    let n = 0;
    const calls = stub({
      ...catalogue,
      "GET /v1/orgs/o1/app-instances": () => json(200, { items: [], next_cursor: null }),
      "POST /v1/orgs/o1/app-instances": () => {
        n += 1;
        return n === 1
          ? json(422, { error: { code: "instance_host_conflict", message: "taken", details: { fields: [{ path: "/host", message: "The host is already claimed." }] } } })
          : json(201, inst({}));
      },
    });
    wrap(<InstancesView scope={{ kind: "organization", id: "o1" }} canManage />);
    fireEvent.click(await screen.findByRole("button", { name: "Register a self-hosted app" }));
    fireEvent.change(await screen.findByLabelText("App"), { target: { value: "desk" } });
    fireEvent.change(screen.getByLabelText("Host"), { target: { value: " desk.acme.example " } });
    fireEvent.click(screen.getByRole("button", { name: "Register" }));
    expect((await screen.findAllByRole("alert")).map((a) => a.textContent).join(" ")).toMatch(/already claimed/);
    fireEvent.click(screen.getByRole("button", { name: "Register" }));
    await waitFor(() => expect(calls.filter((c) => c.method === "POST")).toHaveLength(2));
    expect(calls.filter((c) => c.method === "POST")[1].body).toEqual({ app_id: "desk", host: "desk.acme.example", label: "desk.acme.example" });
  });

  it("a failed host check says what happened, in the host field (FIX-144)", async () => {
    stub({
      ...catalogue,
      "GET /v1/orgs/o1/app-instances": () => json(200, { items: [], next_cursor: null }),
      "POST /v1/orgs/o1/app-instances": () =>
        json(422, {
          error: {
            code: "instance_probe_failed",
            message: "x",
            details: {
              fields: [{ path: "/host", message: "x" }],
              reason: "unexpected_status",
              status: 404,
              url: "https://its.cern.ch/api/health",
              expected: [200],
            },
          },
        }),
    });
    wrap(<InstancesView scope={{ kind: "organization", id: "o1" }} canManage />);
    fireEvent.click(await screen.findByRole("button", { name: "Register a self-hosted app" }));
    fireEvent.change(await screen.findByLabelText("App"), { target: { value: "desk" } });
    fireEvent.change(screen.getByLabelText("Host"), { target: { value: "its.cern.ch" } });
    fireEvent.click(screen.getByRole("button", { name: "Register" }));
    expect(await screen.findByText(/its\.cern\.ch answered 404 Not Found at \/api\/health\./)).toBeTruthy();
    expect(screen.queryByText(/could not be reached from the gateway/)).toBeNull();
  });

  it("a pasted URL fills Host and Path, and the path is sent as base_path (FIX-145)", async () => {
    const calls = stub({
      ...catalogue,
      "GET /v1/orgs/o1/app-instances": () => json(200, { items: [], next_cursor: null }),
      "POST /v1/orgs/o1/app-instances": () => json(201, inst({ host: "its.cern.ch", base_path: "/jira" })),
    });
    wrap(<InstancesView scope={{ kind: "organization", id: "o1" }} canManage />);
    fireEvent.click(await screen.findByRole("button", { name: "Register a self-hosted app" }));
    fireEvent.change(await screen.findByLabelText("App"), { target: { value: "desk" } });
    expect(screen.getByText(/Only if the app is served under a path, for example \/jira for https:\/\/its\.cern\.ch\/jira/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Host"), { target: { value: "https://its.cern.ch/jira/?x=1" } });
    expect((screen.getByLabelText("Host") as HTMLInputElement).value).toBe("its.cern.ch");
    expect((screen.getByLabelText("Path") as HTMLInputElement).value).toBe("/jira");
    fireEvent.click(screen.getByRole("button", { name: "Register" }));
    await waitFor(() => expect(calls.filter((c) => c.method === "POST")).toHaveLength(1));
    expect(calls.filter((c) => c.method === "POST")[0].body).toEqual({ app_id: "desk", host: "its.cern.ch", base_path: "/jira", label: "its.cern.ch" });
  });

  it("a URL with a port stays as typed so the server explains, and a bad path shows on the Path field", async () => {
    stub({
      ...catalogue,
      "GET /v1/orgs/o1/app-instances": () => json(200, { items: [], next_cursor: null }),
      "POST /v1/orgs/o1/app-instances": () =>
        json(422, { error: { code: "instance_base_path_invalid", message: "x", details: { fields: [{ path: "/base_path", message: "A path segment may not be . or .." }] } } }),
    });
    wrap(<InstancesView scope={{ kind: "organization", id: "o1" }} canManage />);
    fireEvent.click(await screen.findByRole("button", { name: "Register a self-hosted app" }));
    fireEvent.change(await screen.findByLabelText("App"), { target: { value: "desk" } });
    fireEvent.change(screen.getByLabelText("Host"), { target: { value: "https://its.cern.ch:8443/jira" } });
    expect((screen.getByLabelText("Host") as HTMLInputElement).value).toBe("https://its.cern.ch:8443/jira");
    expect((screen.getByLabelText("Path") as HTMLInputElement).value).toBe("");
    fireEvent.change(screen.getByLabelText("Host"), { target: { value: "its.cern.ch" } });
    fireEvent.change(screen.getByLabelText("Path"), { target: { value: "/a/.." } });
    fireEvent.click(screen.getByRole("button", { name: "Register" }));
    expect(await screen.findByText("A path segment may not be . or ..")).toBeInTheDocument();
  });

  it("a registration under a path is named by its base URL in the list and the confirmations (FIX-145)", async () => {
    stub({
      ...catalogue,
      "GET /v1/orgs/o1/app-instances": () => json(200, { items: [inst({ host: "its.cern.ch", base_path: "/jira" })], next_cursor: null }),
    });
    wrap(<InstancesView scope={{ kind: "organization", id: "o1" }} canManage />);
    expect(await screen.findByText("https://its.cern.ch/jira")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Delete https://its.cern.ch/jira" }));
    expect(await screen.findByText("Delete https://its.cern.ch/jira?")).toBeInTheDocument();
  });

  it("deleting asks for confirmation and says what happens", async () => {
    const calls = stub({
      ...catalogue,
      "GET /v1/orgs/o1/app-instances": () => json(200, { items: [inst({})], next_cursor: null }),
      "DELETE /v1/app-instances/i1": () => json(204, null),
    });
    wrap(<InstancesView scope={{ kind: "organization", id: "o1" }} canManage />);
    fireEvent.click(await screen.findByRole("button", { name: "Delete https://desk.acme.example" }));
    const dlg = await screen.findByRole("dialog");
    expect(dlg.textContent).toMatch(/2 connections will be disabled/);
    expect(calls.some((c) => c.method === "DELETE")).toBe(false);
    fireEvent.click(within(dlg).getByRole("button", { name: "Delete self-hosted app" }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE" && c.path === "/v1/app-instances/i1")).toBe(true));
  });

  it("without any instance-bound app it says so and offers no registration", async () => {
    stub({
      "GET /v1/apps": () => json(200, { items: [PLAIN] }),
      "GET /v1/apps/plain": () => json(200, PLAIN),
      "GET /v1/admin/app-instances": () => json(200, { items: [], next_cursor: null }),
    });
    wrap(<InstancesView scope={{ kind: "instance" }} canManage />);
    await screen.findByText("No app supports self-hosting");
    expect(screen.queryByRole("button", { name: "Register a self-hosted app" })).toBeNull();
  });
});

describe("connect dialog instance picker", () => {
  const routes = (extra: Record<string, (c: Call) => Response> = {}) => ({
    ...catalogue,
    "GET /v1/resolve": () => json(200, { org: { id: "o1", name: "Acme", slug: "acme" }, workspace: { id: "w1", name: "Main", slug: "main" } }),
    "GET /v1/apps/desk/guide": () => json(404, { error: { code: "not_found", message: "x" } }),
    "GET /v1/orgs/o1/app-instances": () => json(200, { items: [inst({}), inst({ id: "i3", host: "off.acme.example", status: "disabled" })], next_cursor: null }),
    "GET /v1/workspaces/w1/oauth-clients": () =>
      json(200, {
        items: [
          { id: "c-default", scope: "workspace", package_id: "selfhost", client_key: "oa", fields: { client_id: "d" }, enabled: true, instance_id: null, connection_count: 0, created_at: "", updated_at: "" },
          { id: "c-inst", scope: "workspace", package_id: "selfhost", client_key: "oa", fields: { client_id: "i" }, enabled: true, instance_id: "i1", connection_count: 0, created_at: "", updated_at: "" },
        ],
        next_cursor: null,
      }),
    ...extra,
  });

  it("offers the default host and active instances, and sends the chosen instance", async () => {
    const calls = stub(
      routes({
        "POST /v1/apps/desk/connect": () => json(200, { kind: "connected", connection: { id: "k1" }, missing_scopes: [] }),
      }),
    );
    wrap(<ConnectView />);
    const picker = await screen.findByLabelText("Desk instance");
    await within(picker).findByRole("option", { name: "Acme desk (https://desk.acme.example)" });
    const options = within(picker).getAllByRole("option").map((o) => o.textContent);
    // the default host first; the disabled instance is not offered
    expect(options).toEqual(["saas.desk.example (default)", "Acme desk (https://desk.acme.example)"]);
    fireEvent.change(picker, { target: { value: "i1" } });
    fireEvent.change(screen.getByLabelText("Token"), { target: { value: "t" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/apps/desk/connect")).toBe(true));
    expect(calls.find((c) => c.path === "/v1/apps/desk/connect")?.body).toMatchObject({ instance_id: "i1", method_id: "token" });
  });

  // FIX-134 (D-243): the package's preset instances (Sentry's regions) come before the registered servers.
  it("offers the package's built-in instances (regions) before registered servers and sends the chosen one", async () => {
    const calls = stub(
      routes({
        "GET /v1/orgs/o1/app-instances": () =>
          json(200, {
            items: [
              inst({ id: "i1" }),
              inst({ id: "r-us", org_id: null, host: "us.desk.example", label: "US region", builtin: true, connections: 0 }),
              inst({ id: "r-eu", org_id: null, host: "eu.desk.example", label: "EU region (DE)", builtin: true, connections: 0 }),
            ],
            next_cursor: null,
          }),
        "POST /v1/apps/desk/connect": () => json(200, { kind: "connected", connection: { id: "k1" }, missing_scopes: [] }),
      }),
    );
    wrap(<ConnectView />);
    const picker = await screen.findByLabelText("Desk instance");
    await within(picker).findByRole("option", { name: "US region (us.desk.example)" });
    expect(within(picker).getAllByRole("option").map((o) => o.textContent)).toEqual([
      "saas.desk.example (default)",
      "US region (us.desk.example)",
      "EU region (DE) (eu.desk.example)",
      "Acme desk (https://desk.acme.example)",
    ]);
    expect(within(picker).getByRole("group", { name: "Registered servers" })).toBeInTheDocument();
    fireEvent.change(picker, { target: { value: "r-us" } });
    fireEvent.change(screen.getByLabelText("Token"), { target: { value: "t" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/apps/desk/connect")).toBe(true));
    expect(calls.find((c) => c.path === "/v1/apps/desk/connect")?.body).toMatchObject({ instance_id: "r-us" });
  });

  it("omits instance_id for the default host", async () => {
    const calls = stub(
      routes({
        "POST /v1/apps/desk/connect": () => json(200, { kind: "connected", connection: { id: "k1" }, missing_scopes: [] }),
      }),
    );
    wrap(<ConnectView />);
    await screen.findByLabelText("Desk instance");
    fireEvent.change(screen.getByLabelText("Token"), { target: { value: "t" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/apps/desk/connect")).toBe(true));
    expect(calls.find((c) => c.path === "/v1/apps/desk/connect")?.body).not.toHaveProperty("instance_id");
  });

  it("OAuth lists only the clients of the chosen instance and warns when there is none", async () => {
    stub(routes());
    wrap(<ConnectView />);
    const picker = await screen.findByLabelText("Desk instance");
    fireEvent.click(await screen.findByRole("radio", { name: /Sign in/ }));
    // default host: the unbound client only
    let client = await screen.findByLabelText("OAuth client");
    expect(within(client).getAllByRole("option").map((o) => o.textContent)).toEqual(["Automatic", "workspace client (d)"]);
    fireEvent.change(picker, { target: { value: "i1" } });
    client = await screen.findByLabelText("OAuth client");
    expect(within(client).getAllByRole("option").map((o) => o.textContent)).toEqual(["Automatic", "workspace client (i)"]);
    expect(screen.queryByText(/No OAuth application is registered for this instance/)).toBeNull();
  });
});

describe("instance on connections and OAuth clients", () => {
  it("shows the instance host on a connection and no enable button once its instance is deleted", async () => {
    const base = { org_id: "o1", workspace_id: "w1", scope: "workspace", app_id: "desk", package_id: "selfhost", app_version: "1", scopes: [], missing_scopes: [], metadata: {}, tenant_host: null, oauth_client_id: null, credentials_expires_at: null, connected_by: null, last_used_at: null, created_at: "", updated_at: "", injecting: [] };
    stub({
      "GET /v1/workspaces/w1/connections": () =>
        json(200, {
          items: [
            { ...base, id: "k1", label: "Acme GitLab", status: "active", status_reason: null, instance: { id: "i1", host: "desk.acme.example", base_path: "/jira", base_url: "https://desk.acme.example/jira", label: "Acme desk" } },
            { ...base, id: "k2", label: "Gone", status: "disabled", status_reason: "instance_deleted", instance: null },
          ],
          next_cursor: null,
        }),
    });
    wrap(<ConnectionsTab scope={{ kind: "workspace", id: "w1" }} canOwn />);
    // the connection's instance line names the registration by its base URL (D-231)
    expect(await screen.findByText("https://desk.acme.example/jira")).toBeInTheDocument();
    expect(screen.getByText(/self-hosted app of this connection was deleted/)).toBeInTheDocument();
    // k1 can be disabled, k2 (instance deleted) cannot be re-enabled
    expect(screen.getAllByRole("button", { name: /^(Disable|Enable)$/ })).toHaveLength(1);
  });

  it("an OAuth client row names its instance, and the create form binds a client to one", async () => {
    const calls = stub({
      ...catalogue,
      "GET /v1/orgs/o1/oauth-clients": () =>
        json(200, { items: [{ id: "c1", scope: "organization", package_id: "selfhost", client_key: "oa", fields: { client_id: "i" }, enabled: true, instance_id: "i1", connection_count: 0, created_at: "", updated_at: "" }], next_cursor: null }),
      "GET /v1/orgs/o1/app-instances": () => json(200, { items: [inst({})], next_cursor: null }),
      "POST /v1/orgs/o1/oauth-clients": () => json(201, { id: "c2" }),
    });
    wrap(<OAuthClientsPanel scope={{ kind: "organization", id: "o1" }} canOwn />);
    expect(await screen.findByText("https://desk.acme.example")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Add an OAuth client"));
    const app = await screen.findByLabelText("App");
    await within(app).findByRole("option", { name: "Desk (oa)" });
    fireEvent.change(app, { target: { value: "selfhost|oa" } });
    const picker = await screen.findByLabelText("Self-hosted app");
    await within(picker).findByRole("option", { name: "Acme desk (https://desk.acme.example)" });
    fireEvent.change(picker, { target: { value: "i1" } });
    fireEvent.change(screen.getByLabelText("Application ID"), { target: { value: "cid" } });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    expect(calls.find((c) => c.method === "POST")?.body).toMatchObject({ package_id: "selfhost", client_key: "oa", instance_id: "i1" });
  });
});
