import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ByoSettings } from "@/components/agents/byo-settings";
import { ConnectTab } from "@/components/agents/connect-tab";
import { CreateAgentDialog } from "@/components/agents/create-agent-dialog";
import { AgentsList } from "@/components/agents-list";
import { GovernanceView } from "@/components/governance/governance-view";
import { PolicyView } from "@/components/policy/policy-view";
import { GeneralTab } from "@/components/settings/general-tab";
import {
  byoOptionsFrom,
  formFromDocument,
  formFromEffective,
  formToDocument,
  type EffectiveGovernance,
} from "@/lib/api/governance";
import { allAgentsText, appliesToAllAgents, type RuleInput } from "@/lib/api/policy";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${url.pathname}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const eff = (byo?: Partial<EffectiveGovernance["byo"]>): EffectiveGovernance => ({
  llm: { posture: { value: "open", source: "org" }, endpoints: [] },
  egress: {
    posture: { value: "open", source: "org" },
    open_lane: { value: "enabled", source: "org" },
    org_allowlist: [],
    baseline_allowlist: [],
  },
  byo: {
    posture: { value: "owners_only", source: "instance" },
    source_cidrs: { value: null, source: "instance" },
    require_context_credentials: { value: false, source: "instance" },
    ...byo,
  },
  agent_tokens: { max_age_days: { value: null, source: "instance" } },
  non_compliant_secrets: [],
});
const instance = (over: Record<string, unknown> = {}) => ({
  llm: { allowed_key_scopes: [], minimum_posture: "open", require_key_origin: false, known_llm_hosts: [], endpoints: [] },
  egress: { minimum_posture: "open", open_lane_minimum: "enabled", baseline_allowlist: [] },
  byo: { enabled: true, minimum_posture: "owners_only", default_source_cidrs: [], require_context_credentials: false, ...over },
});
const emptyDoc = {
  llm: { posture: "open", endpoints: {} },
  egress: { posture: "open", open_lane: "enabled", allowlist: [] },
  byo: { posture: null, source_cidrs: [], require_context_credentials: false },
};

describe("BYO governance form", () => {
  it("offers only postures at or above the instance floor and maps the document", () => {
    expect(byoOptionsFrom("enabled")).toEqual(["enabled", "owners_only", "disabled"]);
    expect(byoOptionsFrom("owners_only")).toEqual(["owners_only", "disabled"]);
    expect(byoOptionsFrom("disabled")).toEqual(["disabled"]);
    const f = formFromDocument(
      { ...emptyDoc, byo: { posture: "disabled", source_cidrs: ["10.0.0.0/8", "192.0.2.0/24"], require_context_credentials: true } } as never,
      eff(),
      instance({ default_source_cidrs: ["10.0.0.0/8"] }) as never,
    );
    expect(f.byo.floor).toBe("owners_only");
    expect(f.byo.posture).toBe("disabled");
    expect(f.byo.sourceCidrs).toBe("10.0.0.0/8\n192.0.2.0/24");
    expect(f.byo.instanceCidrs).toEqual(["10.0.0.0/8"]);
    expect(formToDocument(f).byo).toEqual({
      posture: "disabled",
      source_cidrs: ["10.0.0.0/8", "192.0.2.0/24"],
      require_context_credentials: true,
    });
    // Defaults inherit: nothing is pinned.
    const d = formFromDocument(emptyDoc as never, eff(), instance() as never);
    expect(formToDocument(d).byo).toEqual({ posture: null, source_cidrs: [], require_context_credentials: false });
  });
  it("locks require-context when the instance switched it on; members see effective values", () => {
    const e = eff({ require_context_credentials: { value: true, source: "instance" }, source_cidrs: { value: ["1.2.3.0/24"], source: "org" } });
    expect(formFromDocument(emptyDoc as never, e, instance() as never).byo.requireContextLocked).toBe(true);
    const m = formFromEffective(e);
    expect(m.byo.requireContext).toBe(true);
    expect(m.byo.sourceCidrs).toBe("1.2.3.0/24");
  });
});

const routes = (extra: Record<string, Handler> = {}, inst = instance()) => ({
  "GET /v1/orgs/o1/governance/effective": () => json(200, eff()),
  "GET /v1/orgs/o1/governance": () => json(200, { document: emptyDoc, revision: 2 }),
  "GET /v1/governance/instance": () => json(200, inst),
  "GET /v1/orgs/o1/workspaces": () => json(200, { items: [], next_cursor: null }),
  ...extra,
});

describe("BYO tab of the governance page", () => {
  it("saves posture, source list and require-context in the document", async () => {
    let body: { document: { byo: unknown } } | null = null;
    mockFetch(
      routes(
        {
          "PUT /v1/orgs/o1/governance": async (req) => {
            body = await req.json();
            return json(200, { document: emptyDoc, revision: 3 });
          },
        },
        instance({ default_source_cidrs: ["10.0.0.0/8"] }),
      ),
    );
    wrap(<GovernanceView orgId="o1" orgSlug="acme" canEdit />);
    fireEvent.click(await screen.findByRole("tab", { name: "BYO agents" }));
    const sel = screen.getByLabelText("Who may create BYO agents") as HTMLSelectElement;
    expect([...sel.options].map((o) => o.value)).toEqual(["", "owners_only", "disabled"]);
    expect(screen.getByText(/inside the instance default: 10.0.0.0\/8/)).toBeTruthy();
    fireEvent.change(sel, { target: { value: "disabled" } });
    fireEvent.change(screen.getByLabelText("Default source CIDR list"), { target: { value: "10.1.0.0/16\n10.2.0.0/16" } });
    fireEvent.click(screen.getByLabelText(/Require context credentials/));
    fireEvent.click(screen.getByRole("button", { name: "Save governance" }));
    await waitFor(() => expect(body).not.toBeNull());
    expect(body!.document.byo).toEqual({
      posture: "disabled",
      source_cidrs: ["10.1.0.0/16", "10.2.0.0/16"],
      require_context_credentials: true,
    });
  });
  it("shows server field errors and the instance master switch notice, read-only for members", async () => {
    mockFetch(
      routes(
        {
          "PUT /v1/orgs/o1/governance": () =>
            json(422, {
              error: {
                code: "byo_cidr_outside_default",
                message: "bad",
                details: { fields: [{ path: "byo.source_cidrs[0]", message: "outside the instance default source list" }] },
              },
            }),
        },
        instance({ enabled: false }),
      ),
    );
    const { unmount } = wrap(<GovernanceView orgId="o1" orgSlug="acme" canEdit />);
    fireEvent.click(await screen.findByRole("tab", { name: "BYO agents" }));
    expect(screen.getByText(/disabled by the instance operator/)).toBeTruthy();
    expect((screen.getByLabelText("Who may create BYO agents") as HTMLSelectElement).disabled).toBe(true);
    unmount();
    wrap(<GovernanceView orgId="o1" orgSlug="acme" canEdit={false} />);
    fireEvent.click(await screen.findByRole("tab", { name: "BYO agents" }));
    expect((screen.getByLabelText("Default source CIDR list") as HTMLTextAreaElement).disabled).toBe(true);
    expect(screen.queryByRole("button", { name: "Save governance" })).toBeNull();
  });
});

const workspace = (over: Record<string, unknown> = {}) =>
  ({
    id: "w1", org_id: "o1", slug: "main", name: "Main", timezone: "UTC", memory_language: "english",
    byo_posture: null, effective_byo_posture: { value: "owners_only", level: "instance" },
    my_role: "owner", created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z", ...over,
  }) as never;

describe("workspace BYO posture", () => {
  it("offers only postures not below the inherited one and patches it only when changed", async () => {
    const bodies: Record<string, unknown>[] = [];
    mockFetch({
      "GET /v1/orgs/o1/governance/effective": () => json(200, eff()),
      "PATCH /v1/workspaces/w1": async (req) => {
        bodies.push(await req.json());
        return json(200, workspace({ byo_posture: "disabled" }));
      },
    });
    wrap(<GeneralTab workspace={workspace()} canOwn />);
    await screen.findByText(/Effective BYO posture/);
    const sel = (await screen.findByLabelText("BYO agent posture")) as HTMLSelectElement;
    await waitFor(() => expect([...sel.options].map((o) => o.value)).toEqual(["", "owners_only", "disabled"]));
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(bodies.length).toBe(1));
    expect("byo_posture" in bodies[0]).toBe(false);
    fireEvent.change(sel, { target: { value: "disabled" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(bodies.length).toBe(2));
    expect(bodies[1].byo_posture).toBe("disabled");
  });
});

const byoAgent = (over: Record<string, unknown> = {}) =>
  ({
    id: "a1", org_id: "o1", workspace_id: "w1", handle: "bot", name: "Bot", kind: "byo", is_default: false,
    proactive_messaging: "ask", brief: "", created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
    byo: {
      posture: "owners_only", posture_level: "instance", source_cidrs: null, require_context: false,
      effective_source_cidrs: ["10.0.0.0/8"], effective_source_level: "instance",
      effective_require_context: false, effective_require_context_level: null,
      ...((over.byo as object) ?? {}),
    },
    ...Object.fromEntries(Object.entries(over).filter(([k]) => k !== "byo")),
  }) as never;

describe("agent access controls", () => {
  it("shows effective values and saves the agent's own list (null = inherit)", async () => {
    const bodies: unknown[] = [];
    mockFetch({
      "PUT /v1/agents/a1/byo": async (req) => {
        bodies.push(await req.json());
        return json(200, byoAgent());
      },
    });
    wrap(<ByoSettings agent={byoAgent()} wsId="w1" canOwn />);
    expect(screen.getByTestId("effective-cidrs").textContent).toContain("10.0.0.0/8");
    expect(screen.getByTestId("effective-cidrs").textContent).toContain("from instance");
    fireEvent.click(screen.getByRole("button", { name: "Save access controls" }));
    await waitFor(() => expect(bodies.length).toBe(1));
    expect(bodies[0]).toEqual({ source_cidrs: null, require_context: false });
    fireEvent.change(screen.getByLabelText("Source CIDR list for this agent"), { target: { value: "10.1.0.0/16, 10.2.0.0/16" } });
    fireEvent.click(screen.getByLabelText(/Require context credentials/));
    fireEvent.click(screen.getByRole("button", { name: "Save access controls" }));
    await waitFor(() => expect(bodies.length).toBe(2));
    expect(bodies[1]).toEqual({ source_cidrs: ["10.1.0.0/16", "10.2.0.0/16"], require_context: true });
  });
  it("members cannot edit; a disabled posture is marked with its level; server errors show", async () => {
    mockFetch({
      "PUT /v1/agents/a1/byo": () =>
        json(422, { error: { code: "byo_cidr_outside_default", message: "outside the default list", details: { fields: [{ path: "/source_cidrs/0", message: "outside" }] } } }),
    });
    const { unmount } = wrap(<ByoSettings agent={byoAgent({ byo: { posture: "disabled", posture_level: "workspace" } })} wsId="w1" canOwn={false} />);
    expect(screen.getByText(/BYO disabled by workspace/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Save access controls" })).toBeNull();
    expect(screen.getByLabelText("Source CIDR list for this agent").matches(":disabled")).toBe(true);
    unmount();
    wrap(<ByoSettings agent={byoAgent()} wsId="w1" canOwn />);
    fireEvent.change(screen.getByLabelText("Source CIDR list for this agent"), { target: { value: "11.0.0.0/8" } });
    fireEvent.click(screen.getByRole("button", { name: "Save access controls" }));
    expect(await screen.findByRole("alert")).toBeTruthy();
  });
  it("shows the caller's address as a hint next to the source list, with the NAT caveat (C-195)", async () => {
    mockFetch({
      "GET /v1/me": () =>
        json(200, { user: { id: "u1", email: "a@x.io" }, default_org_id: null, approval_email: true, client_address: "203.0.113.7", memberships: [] }),
    });
    wrap(<ByoSettings agent={byoAgent()} wsId="w1" canOwn />);
    const hint = await screen.findByTestId("caller-address");
    expect(hint.textContent).toContain("203.0.113.7");
    expect(hint.textContent).toMatch(/gateway may see a different address/);
    expect(hint.textContent).toMatch(/NAT/);
  });
  it("omits the address hint when the API reports none", async () => {
    const fn = mockFetch({
      "GET /v1/me": () => json(200, { user: { id: "u1", email: "a@x.io" }, default_org_id: null, approval_email: true, client_address: null, memberships: [] }),
    });
    wrap(<ByoSettings agent={byoAgent()} wsId="w1" canOwn />);
    await waitFor(() => expect(fn).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.queryByTestId("caller-address")).toBeNull();
  });
  it("the connect tab shows the context-credential note and the agent list marks disabled BYO agents", async () => {
    mockFetch({
      "GET /v1/agents/a1/container-config": () =>
        json(200, {
          proxy_url: "https://x:<CONTEXT_TOKEN>@gw:4102", env: {}, ca_pem: "pem", ca_path: "/aurora/ca.pem",
          proxy_ca_path: "/aurora/ca.pem", proxy_tls: null, llm_env: {}, stubs: [],
          byo: { posture: "owners_only", source_cidrs: null, require_context: true },
          notes: ["This agent must use a context credential."],
        }),
      "GET /v1/gateway/ca.pem": () => new Response("pem", { status: 200 }),
    });
    const { unmount } = wrap(<ConnectTab agent={byoAgent({ byo: { effective_require_context: true, effective_require_context_level: "agent" } })} wsId="w1" canOwn />);
    expect(await screen.findByText("This agent must use a context credential.")).toBeTruthy();
    expect(screen.getAllByText(/CONTEXT_TOKEN/).length).toBeGreaterThan(0);
    unmount();
    wrap(<AgentsList agents={[byoAgent({ byo: { posture: "disabled", posture_level: "org" } })]} />);
    expect(screen.getByText("BYO disabled by org")).toBeTruthy();
  });
});

const rule = (over: Partial<RuleInput> & { name: string }): RuleInput =>
  ({ action: "allow", targets: [{ type: "secret", id: "s1" }], identities: [], enabled: true, ...over }) as RuleInput;

describe("rules that apply to all agents", () => {
  it("matches the server rule: enabled allow, empty identities, credential targets", () => {
    expect(appliesToAllAgents(rule({ name: "a" }))).toBe(true);
    expect(appliesToAllAgents(rule({ name: "a", targets: [{ type: "secrets", scope: "any" }] }))).toBe(true);
    expect(appliesToAllAgents(rule({ name: "a", targets: [{ type: "connection", id: "c" }] }))).toBe(true);
    expect(appliesToAllAgents(rule({ name: "a", targets: [{ type: "app_connections", app_id: "x", scope: "any" }] }))).toBe(true);
    expect(appliesToAllAgents(rule({ name: "a", identities: [{ kind: "agent", id: "a1" }] }))).toBe(false);
    expect(appliesToAllAgents(rule({ name: "a", action: "block" }))).toBe(false);
    expect(appliesToAllAgents(rule({ name: "a", enabled: false }))).toBe(false);
    expect(appliesToAllAgents(rule({ name: "a", targets: [{ type: "network", host: "x" }] }))).toBe(false);
    expect(allAgentsText(true)).toContain("members creating BYO agents");
    expect(allAgentsText(false)).not.toContain("BYO");
  });

  const gen = (rules: unknown[]) => ({
    id: "g1", number: 2, status: "draft", default_action: "none", uncredentialed_default: "none",
    uncredentialed_allowlist: [], open_lane_posture: "enabled", draft_revision: 3, created_at: "2026-01-01T00:00:00Z",
    rules: rules.map((r, i) => ({ logical_id: `r${i}`, priority: i, description: null, rate_limit: null, conditions: [], require_approval: false, ...(r as object) })),
  });
  it("flags the rule inline and lists it in a confirmation step before publishing", async () => {
    let published = 0;
    const broad = { name: "broad", enabled: true, identities: [], targets: [{ type: "secret", id: "s1" }], action: "allow" };
    const scoped = { name: "scoped", enabled: true, identities: [{ kind: "agent", id: "a1" }], targets: [{ type: "secret", id: "s1" }], action: "allow" };
    mockFetch({
      "GET /v1/workspaces/w1/policy": () => json(200, { draft: gen([broad, scoped]), published: gen([]) }),
      "GET /v1/workspaces/w1/policy/warnings": () => json(200, { items: [] }),
      "GET /v1/orgs/o1/governance/effective": () => json(200, eff({ posture: { value: "enabled", source: "instance" } })),
      "GET /v1/workspaces/w1/secrets": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/orgs/o1/secrets": () => json(200, { items: [], next_cursor: null }),
      "PUT /v1/workspaces/w1/policy/draft": () => json(200, gen([broad, scoped])),
      "POST /v1/workspaces/w1/policy/publish": () => {
        published += 1;
        return json(200, gen([broad, scoped]));
      },
    });
    wrap(<PolicyView title="Policy" scope={{ kind: "workspace", id: "w1" }} canEdit orgId="o1" />);
    const notes = await screen.findAllByTestId("all-agents-note");
    expect(notes).toHaveLength(1);
    await waitFor(() => expect(notes[0].textContent).toContain("members creating BYO agents"));
    fireEvent.click(screen.getByRole("button", { name: "Apply changes" }));
    const dlg = await screen.findByRole("alertdialog", { name: "Confirm broad rules" });
    expect(dlg.textContent).toContain("broad");
    expect(dlg.textContent).not.toContain("scoped");
    expect(published).toBe(0);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog", { name: "Confirm broad rules" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Apply changes" }));
    fireEvent.click(await screen.findByRole("button", { name: "Apply anyway" }));
    await waitFor(() => expect(published).toBe(1));
  });
});

describe("labelled runs", () => {
  it("the approval card shows the context label of the run", async () => {
    const { ApprovalCard } = await import("@/components/approvals/approvals-view");
    const approval = {
      id: "ap1", org_id: "o1", workspace_id: "w1", agent: { id: "a1", name: "Bot", handle: "bot" },
      method: "POST", host: "api.example.com", path: "/x", rule_name: "r",
      summary: { title: "POST api.example.com", fields: [], source: "generic" },
      context_label: "aurora run: make deploy", status: "pending",
      deadline: new Date(Date.now() + 60_000).toISOString(), created_at: "2026-01-01T00:00:00Z",
    };
    render(
      <ul>
        <ApprovalCard approval={approval as never} now={Date.now()} onDecide={() => {}} pending={false} />
      </ul>,
    );
    expect(screen.getByTestId("context-label").textContent).toBe("Run: aurora run: make deploy");
  });
});

describe("BYO posture in the agent UI", () => {
  const instanceWith = (hosted: boolean) => ({ features: { hosted } });
  it("offers BYO only as the posture allows and says owners only otherwise", async () => {
    mockFetch({
      "GET /v1/instance": () => json(200, instanceWith(true)),
      "GET /v1/workspaces/w1": () => json(200, workspace({ my_role: "member", effective_byo_posture: { value: "owners_only", level: "instance" } })),
    });
    const { unmount } = wrap(<CreateAgentDialog wsId="w1" />);
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    await screen.findByLabelText("Kind");
    await waitFor(() => expect(screen.getByRole("option", { name: /Bring your own/ }).hasAttribute("disabled")).toBe(true));
    expect(screen.getByText(/workspace owners only/)).toBeTruthy();
    unmount();
    mockFetch({
      "GET /v1/instance": () => json(200, instanceWith(true)),
      "GET /v1/workspaces/w1": () => json(200, workspace({ my_role: "owner", effective_byo_posture: { value: "owners_only", level: "instance" } })),
    });
    wrap(<CreateAgentDialog wsId="w1" />);
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    await screen.findByLabelText("Kind");
    expect(screen.getByRole("option", { name: /Bring your own/ }).hasAttribute("disabled")).toBe(false);
  });
  it("says why BYO is unavailable when disabled and no hosted alternative exists", async () => {
    mockFetch({
      "GET /v1/instance": () => json(200, instanceWith(false)),
      "GET /v1/workspaces/w1": () => json(200, workspace({ effective_byo_posture: { value: "disabled", level: "org" } })),
    });
    wrap(<CreateAgentDialog wsId="w1" />);
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    expect(await screen.findByText(/disabled by the org settings/)).toBeTruthy();
    expect((screen.getByRole("button", { name: "Create agent" }) as HTMLButtonElement).disabled).toBe(true);
  });
  it("hides regeneration from non-owners under owners_only and when disabled", async () => {
    const cfg = {
      proxy_url: "https://x:<AGENT_TOKEN>@gw:4102", env: {}, ca_pem: "pem", ca_path: "/aurora/ca.pem",
      proxy_ca_path: "/aurora/ca.pem", proxy_tls: null, llm_env: {}, stubs: [], byo: null, notes: [],
    };
    mockFetch({
      "GET /v1/agents/a1/container-config": () => json(200, cfg),
      "GET /v1/gateway/ca.pem": () => new Response("pem", { status: 200 }),
    });
    const { unmount } = wrap(<ConnectTab agent={byoAgent()} wsId="w1" canOwn={false} />);
    expect(screen.queryByRole("button", { name: "Regenerate token" })).toBeNull();
    expect(screen.getByText(/only workspace owners can regenerate/)).toBeTruthy();
    unmount();
    const { unmount: u2 } = wrap(<ConnectTab agent={byoAgent()} wsId="w1" canOwn />);
    expect(screen.getByRole("button", { name: "Regenerate token" })).toBeTruthy();
    u2();
    wrap(<ConnectTab agent={byoAgent({ byo: { posture: "disabled", posture_level: "org" } })} wsId="w1" canOwn />);
    expect(screen.queryByRole("button", { name: "Regenerate token" })).toBeNull();
  });
});
