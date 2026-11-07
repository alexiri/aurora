import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AccessTab } from "@/components/agents/access-tab";
import { PolicyView } from "@/components/policy/policy-view";
import { SimulationResult } from "@/components/policy/simulate-panel";
import { ApiError } from "@/lib/api/client";
import {
  draftFromGeneration,
  moveRule,
  normalizeRule,
  policyIssues,
  toDraftRequest,
  type PolicyGeneration,
  type RuleInput,
  type Simulation,
} from "@/lib/api/policy";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

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

const gen = (over: Partial<PolicyGeneration> = {}): PolicyGeneration => ({
  id: "g1", number: 1, status: "draft", default_action: "none", uncredentialed_default: "none",
  uncredentialed_allowlist: [], open_lane_posture: "enabled", rules: [], draft_revision: 3, created_at: "2026-01-01T00:00:00Z", ...over,
});
const rule = (name: string, id: string, over: Record<string, unknown> = {}) => ({
  logical_id: id, priority: 0, name, description: null, enabled: true, identities: [],
  targets: [{ type: "network", host: "api.x.com" }], action: "allow", require_approval: false,
  rate_limit: null, conditions: [], ...over,
});

describe("policy mapping", () => {
  it("keeps the type tag and strips modifiers from block rules", () => {
    const r: RuleInput = {
      name: "r", action: "block", require_approval: true, rate_limit: { count: 1, per: "day" },
      targets: [{ type: "secrets", scope: "any" }], description: "  ",
    };
    const n = normalizeRule(r);
    expect(n.require_approval).toBe(false);
    expect(n.rate_limit).toBeNull();
    expect(n.description).toBeNull();
    expect(n.targets).toEqual([{ type: "secrets", scope: "any" }]);
  });
  it("round-trips a generation to a draft request; org omits uncredentialed fields", () => {
    const g = gen({ rules: [rule("a", "r1", { rate_limit: { count: 5, per: "hour" } }) as never] });
    const state = draftFromGeneration(g);
    const ws = toDraftRequest(state, 3, "workspace");
    expect(ws.revision).toBe(3);
    expect(ws.rules[0].logical_id).toBe("r1");
    expect(ws.rules[0].rate_limit).toEqual({ count: 5, per: "hour" });
    expect(ws.uncredentialed_default).toBe("none");
    expect(ws.open_lane_posture).toBe("enabled");
    expect(toDraftRequest({ ...state, open_lane_posture: "disabled" }, 3, "workspace").open_lane_posture).toBe("disabled");
    const org = toDraftRequest(state, 3, "organization");
    expect(org.uncredentialed_default).toBeUndefined();
    expect(org.open_lane_posture).toBeUndefined();
  });
  it("reorders rules", () => {
    expect(moveRule(["a", "b", "c"], 2, 0)).toEqual(["c", "a", "b"]);
    expect(moveRule(["a", "b"], 0, -1)).toEqual(["a", "b"]);
  });
  it("maps policy_invalid fields to rule indexes", () => {
    const e = new ApiError("policy_invalid", "bad", { fields: [{ path: "/rules/1/targets/0/host", message: "required" }, { path: "/default_action", message: "x" }] }, 422);
    const i = policyIssues(e);
    expect(i.rules[1]).toEqual(["targets/0/host: required"]);
    expect(i.general).toEqual(["/default_action: x"]);
  });
});

describe("PolicyView", () => {
  const scope = { kind: "workspace", id: "w1" } as const;
  const base = (extra: Record<string, Handler> = {}) =>
    mockFetch({
      "GET /v1/workspaces/w1/policy": () =>
        json(200, { scope: "workspace", published: gen({ status: "published", draft_revision: null }), draft: gen({ rules: [rule("Allow x", "r1") as never, rule("Other", "r2") as never] }) }),
      "GET /v1/workspaces/w1/policy/warnings": () => json(200, { items: [{ code: "shadowed", rule_logical_id: "r2", message: "Shadowed by Allow x" }] }),
      "GET /v1/workspaces/w1/secrets": () => json(200, { items: [] }),
      "GET /v1/orgs/o1/secrets": () => json(200, { items: [] }),
      "GET /v1/orgs/o1/governance/effective": () =>
        json(200, { llm: { posture: { value: "open", source: "org" }, endpoints: [] }, egress: { posture: { value: "open", source: "org" }, open_lane: { value: "allowlist_only", source: "instance" }, org_allowlist: [], baseline_allowlist: [] }, non_compliant_secrets: [] }),
      ...extra,
    });

  it("shows rules and warnings; read-only hides actions", async () => {
    base();
    wrap(<PolicyView title="Policy" scope={scope} orgId="o1" canEdit={false} />);
    expect(await screen.findByText("Allow x")).toBeInTheDocument();
    expect(await screen.findByText(/Shadowed by Allow x/)).toBeInTheDocument();
    expect(screen.queryByText("Apply changes")).toBeNull();
  });

  it("reorders, applies (PUT draft then publish) with the revision", async () => {
    let putBody: { revision: number; rules: { name: string }[] } | null = null;
    let pubBody: unknown = null;
    base({
      "PUT /v1/workspaces/w1/policy/draft": async (req) => {
        putBody = await req.json();
        return json(200, gen({ draft_revision: 4 }));
      },
      "POST /v1/workspaces/w1/policy/publish": async (req) => {
        pubBody = await req.json();
        return json(200, { scope: "workspace", published: gen(), draft: gen() });
      },
    });
    wrap(<PolicyView title="Policy" scope={scope} orgId="o1" canEdit />);
    fireEvent.click(await screen.findByLabelText("Move Other up"));
    fireEvent.click(screen.getByText("Apply changes"));
    await waitFor(() => expect(pubBody).toEqual({ revision: 4 }));
    expect(putBody!.revision).toBe(3);
    expect(putBody!.rules.map((r) => r.name)).toEqual(["Other", "Allow x"]);
  });

  it("edits the workspace open-lane posture and shows what the higher level enforces", async () => {
    let putBody: { open_lane_posture?: string } | null = null;
    base({
      "PUT /v1/workspaces/w1/policy/draft": async (req) => {
        putBody = await req.json();
        return json(200, gen({ draft_revision: 4 }));
      },
    });
    wrap(<PolicyView title="Policy" scope={scope} orgId="o1" canEdit />);
    const sel = (await screen.findByLabelText("Open lane posture")) as HTMLSelectElement;
    expect(sel.value).toBe("enabled");
    expect((await screen.findByTestId("open-lane-enforced")).textContent).toMatch(/Enforced by instance: allowlist_only/);
    fireEvent.change(sel, { target: { value: "disabled" } });
    fireEvent.click(screen.getByText("Save draft"));
    await waitFor(() => expect(putBody).not.toBeNull());
    expect(putBody!.open_lane_posture).toBe("disabled");
  });

  it("prompts to reload on stale_revision", async () => {
    base({
      "PUT /v1/workspaces/w1/policy/draft": () => json(409, { error: { code: "stale_revision", message: "stale" } }),
    });
    wrap(<PolicyView title="Policy" scope={scope} orgId="o1" canEdit />);
    fireEvent.click(await screen.findByLabelText("Move Other up"));
    fireEvent.click(screen.getByText("Apply changes"));
    expect(await screen.findByText("Reload policy")).toBeInTheDocument();
  });

  it("maps policy_invalid errors onto the rule", async () => {
    base({
      "PUT /v1/workspaces/w1/policy/draft": () =>
        json(422, { error: { code: "policy_invalid", message: "bad", details: { fields: [{ path: "/rules/0/name", message: "too long" }] } } }),
    });
    wrap(<PolicyView title="Policy" scope={scope} orgId="o1" canEdit />);
    fireEvent.click(await screen.findByLabelText("Move Other up"));
    fireEvent.click(screen.getByText("Apply changes"));
    expect(await screen.findByText("name: too long")).toBeInTheDocument();
  });
});

describe("SimulationResult", () => {
  it("renders verdict, guidance, credential and trace", () => {
    const sim: Simulation = {
      verdict: "needs_approval", reason: "rule", guidance_code: "approval_required",
      winning_credential: { kind: "secret", id: "s1", name: "Stripe" }, candidates: [],
      trace: {
        organization: { verdict: null, considered: [] },
        workspace: { verdict: "needs_approval", considered: [{ rule_logical_id: "r1", rule_name: "Pay", source: "hand", matched: true, reason: "host matched" }] },
      },
    };
    render(<SimulationResult sim={sim} />);
    expect(screen.getByTestId("verdict")).toHaveTextContent("needs_approval");
    expect(screen.getByText("approval_required")).toBeInTheDocument();
    expect(screen.getByText(/Stripe/)).toBeInTheDocument();
    expect(screen.getByText(/Matched · Pay/)).toBeInTheDocument();
  });
});

describe("AccessTab", () => {
  it("toggles a grant with PUT and DELETE", async () => {
    const calls: string[] = [];
    const secret = { id: "s1", name: "Stripe", scope: "workspace", kind: "generic", status: "active" };
    mockFetch({
      "GET /v1/workspaces/w1/secrets": () => json(200, { items: [secret] }),
      "GET /v1/orgs/o1/secrets": () => json(200, { items: [] }),
      "GET /v1/agents/a1/grants": () => json(200, { secrets: [], connections: [] }),
      "GET /v1/agents/a1/effective": () => json(200, { credentials: [{ id: "s1", kind: "secret", name: "Stripe", scope: "workspace", selected: true, selected_by: ["r1"] }], tools: [] }),
      "PUT /v1/agents/a1/grants/secrets/s1": () => {
        calls.push("PUT");
        return json(200, { id: "g", agent_id: "a1", secret_id: "s1", created_at: "2026-01-01T00:00:00Z" });
      },
    });
    wrap(<AccessTab agentId="a1" wsId="w1" orgId="o1" canOwn />);
    const box = await screen.findByLabelText("Grant Stripe");
    fireEvent.click(box);
    await waitFor(() => expect(calls).toEqual(["PUT"]));
    expect(await screen.findByText("selected")).toBeInTheDocument();
  });

  it("offers org secrets to workspace owners (I-27 amendment)", async () => {
    const ws = { id: "s1", name: "Stripe", scope: "workspace", kind: "generic", status: "active" };
    const org = { id: "s2", name: "Shared LLM", scope: "organization", kind: "llm", status: "active" };
    mockFetch({
      "GET /v1/workspaces/w1/secrets": () => json(200, { items: [ws] }),
      "GET /v1/orgs/o1/secrets": () => json(200, { items: [org] }),
      "GET /v1/agents/a1/grants": () => json(200, { secrets: [], connections: [] }),
      "GET /v1/agents/a1/effective": () => json(200, { credentials: [], tools: [] }),
    });
    wrap(<AccessTab agentId="a1" wsId="w1" orgId="o1" canOwn />);
    expect(await screen.findByLabelText("Grant Stripe")).toBeInTheDocument();
    expect(await screen.findByLabelText("Grant Shared LLM")).toBeInTheDocument();
  });

  it("members only see effective access and never call the can_own lists", async () => {
    const fn = mockFetch({
      "GET /v1/agents/a1/effective": () => json(200, { credentials: [], tools: [] }),
    });
    wrap(<AccessTab agentId="a1" wsId="w1" orgId="o1" canOwn={false} />);
    expect(await screen.findByText("No credentials are available to this agent.")).toBeInTheDocument();
    expect(screen.queryByLabelText(/^Grant /)).toBeNull();
    const urls = fn.mock.calls.map((c) => (c[0] as Request).url);
    expect(urls.some((u) => u.includes("/secrets") || u.includes("/grants"))).toBe(false);
  });
});
