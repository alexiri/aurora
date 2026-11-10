import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AccessTab } from "@/components/agents/access-tab";
import { ConnectionGrants } from "@/components/agents/connection-grants";
import { appIsCustomOnly, appRestrictions } from "@/lib/api/apps";
import {
  customBody,
  grantState,
  groupState,
  initialDecisions,
  inertToolIds,
  mapGrantError,
  parseTimeout,
  setGroup,
  timeoutProblem,
  type Tool,
} from "@/lib/grant-editor";
import { ApiError } from "@/lib/api/client";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request, body: unknown) => Response | Promise<Response>;
interface Call { key: string; body: unknown }
let calls: Call[];
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

// ---- data: real built-in packages (plugins/builtin/github, indico) and their tool ids ----------------------------------
const now = "2026-01-01T00:00:00Z";
const AGENT = "a0000000-0000-4000-8000-000000000011";
const WS = "a0000000-0000-4000-8000-000000000002";
const ORG = "a0000000-0000-4000-8000-000000000001";
const C_GH = "c0000000-0000-4000-8000-000000000040";
const C_JIRA = "c0000000-0000-4000-8000-000000000041";
const C_IND = "c0000000-0000-4000-8000-000000000042";

const tool = (id: string, name: string, group: "read" | "write", over: Partial<Tool> = {}): Tool => ({
  id, name, group, host: "api.github.com", path: `/${id}`, methods: group === "read" ? ["GET"] : ["POST"], wildcard: false, ...over,
});
const ghTools: Tool[] = [
  tool("get_repo", "Get a repository", "read", { path: "/repos/*/*" }),
  tool("list_issues", "List issues", "read", { path: "/repos/*/*/issues" }),
  tool("list_pulls", "List pull requests", "read", { path: "/repos/*/*/pulls" }),
  tool("create_issue", "Create an issue", "write", { path: "/repos/*/*/issues" }),
  tool("merge_pull", "Merge a pull request", "write", { path: "/repos/*/*/pulls/*/merge", methods: ["PUT"] }),
];
const ghWithUmbrella: Tool[] = [tool("all_read", "All read operations", "read", { wildcard: true, path: "/*" }), ...ghTools];
const indicoTools: Tool[] = [
  tool("get_current_user", "Get the connected Indico user", "read", { host: "indico.example.org", path: "/api/user/" }),
  tool("get_event", "Read an event", "read", { host: "indico.example.org", path: "/export/event/*" }),
];

const conn = (id: string, appId: string, label: string, scope = "workspace") => ({
  app_id: appId, app_version: "1.0.0", created_at: now, id, injecting: [], label, metadata: {}, missing_scopes: [], org_id: ORG,
  package_id: appId, scope, scopes: [], status: "active", updated_at: now, workspace_id: scope === "workspace" ? WS : null,
});
const grant = (connectionId: string, over: Record<string, unknown> = {}) => ({
  id: "g-" + connectionId, agent_id: AGENT, connection_id: connectionId, created_at: now, mode: "custom", allow_tools: [], ask_tools: [], ask_timeout_secs: null, ...over,
});
const appDetail = (appId: string, name: string, tools: Tool[], extra: Record<string, unknown> = {}) => ({
  app_id: appId, name, package_id: appId, package_version: "1.0.0", available: true, connection_count: 1, description: "", has_dark_icon: false,
  connect: [], credentials: [], hosts: [], metadata: [], oauth_clients: [], tools, ...extra,
});
const appSummary = (appId: string, name: string) => ({ app_id: appId, name, package_id: appId, package_version: "1.0.0", available: true, connection_count: 1, description: "", has_dark_icon: false });

interface Setup {
  grants?: unknown[];
  appTools?: Tool[];
  appExtra?: Record<string, unknown>;
  put?: Handler;
  /** `converted_by_app` of the grants response (C-413). */
  converted?: unknown[];
  /** `app_restrictions` of the catalogue entry of github. */
  catalogueRestrictions?: { custom_only: boolean; block_untooled: boolean };
}
function setup(s: Setup = {}) {
  const grants = s.grants ?? [grant(C_JIRA, { mode: "full" }), grant(C_IND, { allow_tools: ["get_event"], ask_tools: ["get_current_user"], ask_timeout_secs: 600 })];
  routes[`GET /v1/workspaces/${WS}/connections`] = () => json(200, { items: [conn(C_GH, "github", "acme-bot"), conn(C_JIRA, "atlassian", "Jira Cloud"), conn(C_IND, "indico", "CERN Indico")] });
  routes[`GET /v1/orgs/${ORG}/connections`] = () => json(200, { items: [conn("c0000000-0000-4000-8000-000000000043", "github", "org-bot", "organization")] });
  routes[`GET /v1/agents/${AGENT}/grants`] = () => json(200, { secrets: [], connections: grants, converted_by_app: s.converted ?? [] });
  routes["GET /v1/apps"] = () =>
    json(200, {
      items: [
        { ...appSummary("github", "GitHub"), app_restrictions: s.catalogueRestrictions ?? { custom_only: false, block_untooled: false } },
        appSummary("atlassian", "Atlassian"),
        appSummary("indico", "Indico"),
      ],
    });
  routes["GET /v1/apps/github"] = () => json(200, appDetail("github", "GitHub", s.appTools ?? ghTools, s.appExtra));
  routes["GET /v1/apps/atlassian"] = () => json(200, appDetail("atlassian", "Atlassian", ghTools));
  routes["GET /v1/apps/indico"] = () => json(200, appDetail("indico", "Indico", indicoTools));
  routes[`GET /v1/agents/${AGENT}/effective`] = () => json(200, { credentials: [], tools: [] });
  routes[`GET /v1/agents/${AGENT}/model-readiness`] = () => json(200, { harness: "claude-code", override_dropped: false, default: null, keys: [] });
  routes[`PUT /v1/agents/${AGENT}/grants/connections/${C_GH}`] = s.put ?? ((_r, body) => json(200, { ...grant(C_GH), ...(body as object) }));
  routes[`DELETE /v1/agents/${AGENT}/grants/connections/${C_JIRA}`] = () => json(204, null);
}

const view = () => wrap(<ConnectionGrants agentId={AGENT} wsId={WS} orgId={ORG} />);
async function openEditor(name: string) {
  fireEvent.click(await screen.findByRole("button", { name }));
  return screen.findByRole("dialog");
}
const radio = (group: string, label: string) => within(screen.getByRole("radiogroup", { name: group })).getByRole("radio", { name: label }) as HTMLInputElement;
const choose = (group: string, label: string) => fireEvent.click(radio(group, label));

describe("connection list (owners)", () => {
  it("shows none, full and custom states for every connection in scope, with Edit and Remove", async () => {
    setup();
    view();
    const list = await screen.findByRole("list", { name: "Connection grants" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(4);
    expect(within(rows[0]).getByText("acme-bot")).toBeTruthy();
    expect(within(rows[0]).getByTestId("grant-state").textContent).toBe("No access");
    expect(within(rows[0]).getByRole("button", { name: "Give access to acme-bot" })).toBeTruthy();
    expect(within(rows[0]).queryByRole("button", { name: /Remove/ })).toBeNull();
    expect(within(rows[1]).getByTestId("grant-state").textContent).toBe("Full access");
    expect(within(rows[1]).getByRole("button", { name: "Edit access to Jira Cloud" })).toBeTruthy();
    expect(within(rows[1]).getByRole("button", { name: "Remove access to Jira Cloud" })).toBeTruthy();
    expect(within(rows[2]).getByTestId("grant-state").textContent).toBe("Custom: 1 allowed, 1 ask");
    expect(within(rows[3]).getByText("org-bot")).toBeTruthy();
  });

  it("is read-only for non-owners: no Edit or Remove, decisions from the effective view", async () => {
    setup();
    routes[`GET /v1/agents/${AGENT}/effective`] = () =>
      json(200, {
        credentials: [],
        tools: [
          { app_id: "github", connection_id: C_GH, tool_id: "get_repo", decision: "allow", inert: false },
          { app_id: "github", connection_id: C_GH, tool_id: "merge_pull", decision: "needs_approval", inert: false },
          { app_id: "github", connection_id: C_GH, tool_id: "create_issue", decision: "block", inert: false },
        ],
      });
    wrap(<AccessTab agentId={AGENT} wsId={WS} orgId={ORG} canOwn={false} />);
    const list = await screen.findByRole("list", { name: "Connection access (read only)" });
    expect(within(list).getByText("1 allowed, 1 ask, 1 blocked")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Edit access|Give access|Remove access/ })).toBeNull();
    expect(count(`GET /v1/agents/${AGENT}/grants`)).toBe(0);
  });
});

describe("editor", () => {
  it("full → custom starts with every tool on Block, groups Read and Write; Save sends allow and ask with the timeout and refreshes the views", async () => {
    setup();
    view();
    await screen.findByRole("list", { name: "Connection grants" });
    await openEditor("Give access to acme-bot");
    await screen.findByRole("radiogroup", { name: "Access mode" });
    expect(radio("Access mode", "Full access").checked).toBe(true);
    fireEvent.click(radio("Access mode", "Custom"));
    expect(screen.getByRole("region", { name: "Read tools" })).toBeTruthy();
    expect(screen.getByRole("region", { name: "Write tools" })).toBeTruthy();
    for (const t of ghTools) expect(radio(`Access for ${t.name}`, "Block").checked).toBe(true);
    expect(screen.queryByLabelText("Approval timeout (seconds)")).toBeNull();

    choose("Access for List issues", "Allow");
    choose("Access for Merge a pull request", "Ask");
    fireEvent.change(await screen.findByLabelText("Approval timeout (seconds)"), { target: { value: "900" } });
    const before = { grants: count(`GET /v1/agents/${AGENT}/grants`), eff: count(`GET /v1/agents/${AGENT}/effective`), ready: count(`GET /v1/agents/${AGENT}/model-readiness`) };
    fireEvent.click(screen.getByRole("button", { name: "Save access" }));
    await waitFor(() => expect(count(`PUT /v1/agents/${AGENT}/grants/connections/${C_GH}`)).toBe(1));
    expect(calls.find((c) => c.key.startsWith("PUT"))!.body).toEqual({ mode: "custom", allow_tools: ["list_issues"], ask_tools: ["merge_pull"], ask_timeout_secs: 900 });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // Cache invalidation: grants and effective refetch (mounted), Model readiness is invalidated (not mounted here, so no fetch needed).
    await waitFor(() => expect(count(`GET /v1/agents/${AGENT}/grants`)).toBeGreaterThan(before.grants));
    expect(count(`GET /v1/agents/${AGENT}/effective`)).toBeGreaterThanOrEqual(before.eff);
    expect(before.ready).toBe(0);
  });

  it("on the Access tab a save refreshes Model readiness and the effective view too", async () => {
    setup();
    routes[`GET /v1/workspaces/${WS}/secrets`] = () => json(200, { items: [] });
    routes[`GET /v1/orgs/${ORG}/secrets`] = () => json(200, { items: [] });
    wrap(<AccessTab agentId={AGENT} wsId={WS} orgId={ORG} canOwn hosted base="/o/acme/w/main" />);
    await openEditor("Give access to acme-bot");
    await waitFor(() => expect(count(`GET /v1/agents/${AGENT}/model-readiness`)).toBe(1));
    const eff = count(`GET /v1/agents/${AGENT}/effective`);
    fireEvent.click(await screen.findByRole("button", { name: "Save access" }));
    await waitFor(() => expect(count(`GET /v1/agents/${AGENT}/model-readiness`)).toBe(2));
    await waitFor(() => expect(count(`GET /v1/agents/${AGENT}/effective`)).toBeGreaterThan(eff));
  });

  it("saves Full access as { mode: full }", async () => {
    setup();
    view();
    await openEditor("Give access to acme-bot");
    fireEvent.click(await screen.findByRole("button", { name: "Save access" }));
    await waitFor(() => expect(calls.find((c) => c.key.startsWith("PUT"))?.body).toEqual({ mode: "full" }));
  });

  it("the group setter sets every row of one group and leaves the other", async () => {
    setup();
    view();
    await openEditor("Give access to acme-bot");
    fireEvent.click(await screen.findByRole("radio", { name: "Custom" }));
    expect(groupState(initialDecisions(ghTools, null), ghTools, "read")).toBe("block");
    choose("Set all read tools", "Allow");
    for (const n of ["Get a repository", "List issues", "List pull requests"]) expect(radio(`Access for ${n}`, "Allow").checked).toBe(true);
    expect(radio("Access for Create an issue", "Block").checked).toBe(true);
    expect(radio("Set all read tools", "Allow").checked).toBe(true);
    choose("Access for List issues", "Ask");
    expect(radio("Set all read tools", "Allow").checked).toBe(false);
    expect(radio("Set all read tools", "Ask").checked).toBe(false);
    choose("Set all write tools", "Ask");
    expect(radio("Access for Merge a pull request", "Ask").checked).toBe(true);
  });

  it("offers an umbrella only where the catalogue declares one (Read here, not Write)", async () => {
    setup({ appTools: ghWithUmbrella });
    view();
    await openEditor("Give access to acme-bot");
    fireEvent.click(await screen.findByRole("radio", { name: "Custom" }));
    const read = screen.getByRole("region", { name: "Read tools" });
    expect(within(read).getByText("Umbrella")).toBeTruthy();
    expect(within(read).getAllByRole("listitem")[0].textContent).toContain("All read operations");
    expect(within(screen.getByRole("region", { name: "Write tools" })).queryByText("Umbrella")).toBeNull();
    // Chosen like any tool, and sent by its id.
    choose("Access for All read operations", "Allow");
    fireEvent.click(screen.getByRole("button", { name: "Save access" }));
    await waitFor(() => expect(calls.find((c) => c.key.startsWith("PUT"))?.body).toMatchObject({ allow_tools: ["all_read"], ask_tools: [] }));
  });

  it("marks inert tools (named by the grant, gone from the catalogue) and explains them", async () => {
    setup({ grants: [grant(C_GH, { allow_tools: ["get_repo", "list_labels"], ask_tools: [] })] });
    view();
    await openEditor("Edit access to acme-bot");
    const inert = await screen.findByRole("region", { name: "Inert tools" });
    expect(within(inert).getByText("list_labels")).toBeTruthy();
    expect(within(inert).getByText("Inert")).toBeTruthy();
    expect(within(inert).getByText(/An app update removed these tools/)).toBeTruthy();
    expect(radio("Access for list_labels", "Allow").checked).toBe(true);
    // Setting it to Block drops it from the body.
    choose("Access for list_labels", "Block");
    fireEvent.click(screen.getByRole("button", { name: "Save access" }));
    await waitFor(() => expect(calls.find((c) => c.key.startsWith("PUT"))?.body).toEqual({ mode: "custom", allow_tools: ["get_repo"], ask_tools: [] }));
  });

  it("hides Full access for a custom_only app (app_restrictions on the grant) and starts with every tool on Block", async () => {
    setup({ grants: [grant(C_GH, { mode: "full", app_restrictions: { custom_only: true, block_untooled: true } })] });
    view();
    await openEditor("Edit access to acme-bot");
    expect(await screen.findByText("This app only supports custom access: choose the tools the agent may use.")).toBeTruthy();
    expect(screen.queryByRole("radio", { name: "Full access" })).toBeNull();
    for (const t of ghTools) expect(radio(`Access for ${t.name}`, "Block").checked).toBe(true);
    expect(screen.getByText("Requests that are not in the tool list are always blocked.")).toBeTruthy();
    expect(screen.getByText("Custom access only")).toBeTruthy();
  });

  it("shows the Custom access only badge from the catalogue's app_restrictions", async () => {
    setup({ catalogueRestrictions: { custom_only: true, block_untooled: false } });
    view();
    const list = await screen.findByRole("list", { name: "Connection grants" });
    await waitFor(() => expect(within(within(list).getAllByRole("listitem")[0]).getByText("Custom access only")).toBeTruthy());
  });

  it("tells the owner when an app update converted a grant from full to custom", async () => {
    setup({ converted: [{ connection_id: C_GH, app_id: "github", converted_at: now }] });
    view();
    const note = await screen.findByRole("status", { name: "Access converted" });
    expect(note.textContent).toContain("Access for GitHub was converted from full to custom by an app update");
  });

  it("learns custom_only from a 422 full_grant_forbidden and switches to the custom form", async () => {
    setup({ put: () => json(422, { error: { code: "full_grant_forbidden", message: "Full access is not allowed for this app.", details: { app_id: "github" } } }) });
    view();
    await openEditor("Give access to acme-bot");
    fireEvent.click(await screen.findByRole("button", { name: "Save access" }));
    expect(await screen.findByText("This app only supports custom access: choose the tools the agent may use.")).toBeTruthy();
    expect(screen.queryByRole("radio", { name: "Full access" })).toBeNull();
    expect(radio("Access for List issues", "Block").checked).toBe(true);
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("shows the block_untooled note from the app detail", async () => {
    setup({ appExtra: { app_restrictions: { custom_only: false, block_untooled: true } } });
    view();
    await openEditor("Give access to acme-bot");
    expect(await screen.findByText("Requests that are not in the tool list are always blocked.")).toBeTruthy();
    expect(screen.getByRole("radio", { name: "Full access" })).toBeTruthy();
  });

  it("maps empty_custom to a message and keeps the dialog open", async () => {
    setup({ put: () => json(422, { error: { code: "grant_invalid", message: "A custom grant needs at least one tool.", details: { reason: "empty_custom" } } }) });
    view();
    await openEditor("Give access to acme-bot");
    fireEvent.click(await screen.findByRole("radio", { name: "Custom" }));
    fireEvent.click(screen.getByRole("button", { name: "Save access" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Allow or ask at least one tool");
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("maps unknown_tool to the offending rows", async () => {
    setup({
      grants: [grant(C_GH, { allow_tools: ["get_repo", "list_labels"] })],
      put: () => json(422, { error: { code: "grant_invalid", message: "A tool is not in the app's catalogue.", details: { reason: "unknown_tool" } } }),
    });
    view();
    await openEditor("Edit access to acme-bot");
    fireEvent.click(await screen.findByRole("button", { name: "Save access" }));
    const inert = await screen.findByRole("region", { name: "Inert tools" });
    expect(await within(inert).findByText("This tool is not in the app's catalogue any more.")).toBeTruthy();
    expect(screen.getByText(/Set them to Block and save again/)).toBeTruthy();
  });

  it("maps an out-of-range ask timeout to the field, and checks the bounds before sending", async () => {
    setup({
      put: () => json(422, { error: { code: "approval_timeout_out_of_range", message: "Too long.", details: { fields: [{ path: "/ask_timeout_secs", message: "At most 3600 seconds on this instance." }] } } }),
    });
    view();
    await openEditor("Give access to acme-bot");
    fireEvent.click(await screen.findByRole("radio", { name: "Custom" }));
    choose("Access for Merge a pull request", "Ask");
    const field = await screen.findByLabelText("Approval timeout (seconds)");
    fireEvent.change(field, { target: { value: "30" } });
    expect(screen.getByText(/Enter a whole number of seconds from 60 to 14400/)).toBeTruthy();
    expect((screen.getByRole("button", { name: "Save access" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(field, { target: { value: "14000" } });
    fireEvent.click(screen.getByRole("button", { name: "Save access" }));
    expect(await screen.findByText("At most 3600 seconds on this instance.")).toBeTruthy();
    expect(field.getAttribute("aria-invalid")).toBe("true");
  });
});

describe("remove", () => {
  it("asks, calls DELETE and refreshes the grants", async () => {
    setup();
    view();
    fireEvent.click(await screen.findByRole("button", { name: "Remove access to Jira Cloud" }));
    const dlg = await screen.findByRole("dialog");
    const before = count(`GET /v1/agents/${AGENT}/grants`);
    fireEvent.click(within(dlg).getByRole("button", { name: "Remove access" }));
    await waitFor(() => expect(count(`DELETE /v1/agents/${AGENT}/grants/connections/${C_JIRA}`)).toBe(1));
    await waitFor(() => expect(count(`GET /v1/agents/${AGENT}/grants`)).toBeGreaterThan(before));
  });

  it("reports a failed removal", async () => {
    setup();
    routes[`DELETE /v1/agents/${AGENT}/grants/connections/${C_JIRA}`] = () => json(403, { error: { code: "forbidden", message: "no" } });
    view();
    fireEvent.click(await screen.findByRole("button", { name: "Remove access to Jira Cloud" }));
    fireEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Remove access" }));
    expect((await screen.findByText(/Could not remove access/)).textContent).toContain("permission");
  });
});

describe("pure rules", () => {
  it("builds bodies, states and timeouts", () => {
    expect(grantState(null).label).toBe("No access");
    expect(grantState({ mode: "full", allow_tools: [], ask_tools: [] }).kind).toBe("full");
    expect(parseTimeout("")).toBeNull();
    expect(parseTimeout("90")).toBe(90);
    expect(parseTimeout("1.5")).toBe("invalid");
    expect(timeoutProblem("59")).toBeTruthy();
    expect(timeoutProblem("14401")).toBeTruthy();
    expect(timeoutProblem("60")).toBeUndefined();
    // The timeout is dropped when nothing is on Ask (the API refuses it otherwise).
    expect(customBody({ get_repo: "allow" }, "120")).toEqual({ mode: "custom", allow_tools: ["get_repo"], ask_tools: [] });
    expect(inertToolIds(ghTools, { allow_tools: ["get_repo", "gone"], ask_tools: ["gone2"] })).toEqual(["gone", "gone2"]);
    expect(setGroup(initialDecisions(ghTools, null), ghTools, "write", "allow").merge_pull).toBe("allow");
    expect(mapGrantError(new ApiError("grant_invalid", "x", { reason: "tool_in_both" }), ghTools, {}).general).toMatch(/both/);
    expect(mapGrantError(new Error("offline"), ghTools, {}).general).toMatch(/Could not save/);
  });

  it("reads app_restrictions from a grant, the grants response or the app detail", () => {
    expect(appRestrictions(null, { app_restrictions: { custom_only: true, block_untooled: false } })).toEqual({ custom_only: true, block_untooled: false });
    expect(appRestrictions({ app_restrictions: { custom_only: false, block_untooled: true } })).toEqual({ custom_only: false, block_untooled: true });
    expect(appRestrictions(undefined, {})).toEqual({ custom_only: false, block_untooled: false });
    // Restrictions only tighten: a flag from any source applies.
    expect(
      appRestrictions({ app_restrictions: { custom_only: true, block_untooled: false } }, { app_restrictions: { custom_only: false, block_untooled: true } }),
    ).toEqual({ custom_only: true, block_untooled: true });
  });

  it("appIsCustomOnly reads the real app_restrictions field", () => {
    expect(appIsCustomOnly({ app_restrictions: { custom_only: true, block_untooled: false } })).toBe(true);
    expect(appIsCustomOnly({ app_restrictions: { custom_only: false, block_untooled: true } })).toBe(false);
    expect(appIsCustomOnly(undefined)).toBe(false);
  });
});
