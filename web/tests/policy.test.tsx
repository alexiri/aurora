import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { wrap } from "./support/render";
import { type Handler, json, mockFetch } from "./support/api";

vi.mock("next/navigation", () => ({ useSearchParams: () => new URLSearchParams(), useParams: () => ({ org: "acme", ws: "ops" }) }));

import { AccessTab } from "@/components/agents/access-tab";
import { InternetLevel } from "@/components/internet/internet-level";
import { SimulatePanel, SimulationResult } from "@/components/policy/simulate-panel";
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

const EFF = {
  llm: { posture: { value: "open", source: "org" }, endpoints: [] },
  egress: { posture: { value: "open", source: "org" }, open_lane: { value: "allowlist_only", source: "instance" }, org_allowlist: [], baseline_allowlist: [] },
  byo: { posture: { value: "owners_only", source: "instance" } },
  non_compliant_secrets: [],
} as never;
const WHO = { role: "workspace_owner", action: "change_this", people: [], can: false } as never;

/** The Internet access page body of a level, with the effective limits given. */
function PolicyView({ scope, canEdit, orgId, orgSlug = "acme" }: { title?: string; scope: { kind: "workspace" | "organization"; id: string }; canEdit: boolean; orgId: string; orgSlug?: string }) {
  return (
    <InternetLevel level={scope.kind === "workspace" ? "workspace" : "org"} scope={scope} orgId={orgId} orgSlug={orgSlug} wsSlug="main" canEdit={canEdit} eff={EFF} whoCanHere={WHO} whoCanOrg={WHO} />
  );
}

describe("Internet access page (InternetLevel)", () => {
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

  it("shows rules and warnings to an owner", async () => {
    base();
    wrap(<PolicyView title="Policy" scope={scope} orgId="o1" canEdit />);
    expect(await screen.findByText("Allow x")).toBeInTheDocument();
    expect(await screen.findByText(/Shadowed by Allow x/)).toBeInTheDocument();
    expect(screen.getByText("Rules people wrote")).toBeInTheDocument();
  });

  it("a member sees the limits from above and who to ask: no rules, no Common changes, no Tighten…, no policy reads", async () => {
    const fn = base();
    wrap(<PolicyView title="Policy" scope={scope} orgId="o1" canEdit={false} />);
    expect(await screen.findByText("Browser connections (not inspected)")).toBeInTheDocument();
    expect(screen.getByText("Only for listed websites")).toBeInTheDocument();
    expect(screen.getByText("Set by Administration")).toBeInTheDocument();
    for (const gone of ["Allow x", "Common changes", "Add rule", "Tighten…", "Apply changes"]) expect(screen.queryByText(gone)).toBeNull();
    const urls = fn.mock.calls.map((c) => (c[0] as Request).url);
    expect(urls.some((u) => u.includes("/policy"))).toBe(false);
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
    // The move is written to the draft as it is made, not at Apply.
    await waitFor(() => expect(putBody).not.toBeNull());
    expect(pubBody).toBeNull();
    fireEvent.click(await screen.findByText("Apply changes"));
    await waitFor(() => expect(pubBody).toEqual({ revision: 4 }));
    expect(putBody!.revision).toBe(3);
    expect(putBody!.rules.map((r) => r.name)).toEqual(["Other", "Allow x"]);
  });

  it("rolls back with the draft revision the history list showed (FIX-109)", async () => {
    let rollbackBody: unknown = null;
    base({
      "GET /v1/workspaces/w1/policy/generations": () =>
        json(200, {
          items: [
            { id: "g3", number: 3, status: "draft", rule_count: 0, draft_revision: 7, created_at: "2026-01-03T00:00:00Z" },
            { id: "g2", number: 2, status: "published", rule_count: 1, draft_revision: null, created_at: "2026-01-02T00:00:00Z" },
          ],
          next_cursor: null,
        }),
      "POST /v1/workspaces/w1/policy/rollback": async (req) => {
        rollbackBody = await req.json();
        return json(200, { scope: "workspace", published: gen(), draft: gen() });
      },
    });
    wrap(<PolicyView title="Policy" scope={scope} orgId="o1" canEdit />);
    fireEvent.click(await screen.findByText("History of changes"));
    fireEvent.click(await screen.findByLabelText("Roll back to generation 2"));
    await waitFor(() => expect(rollbackBody).toEqual({ generation: 2, expect_revision: 7 }));
  });

  it("keeps the four controls behind Tighten… and writes an edit to the draft at once", async () => {
    let putBody: { open_lane_posture?: string } | null = null;
    base({
      "PUT /v1/workspaces/w1/policy/draft": async (req) => {
        putBody = await req.json();
        return json(200, gen({ draft_revision: 4 }));
      },
    });
    wrap(<PolicyView title="Policy" scope={scope} orgId="o1" canEdit />);
    // What Administration set is a limit from above, in words; the controls are closed until Tighten… is opened.
    expect((await screen.findAllByText("Set by Administration")).length).toBeGreaterThan(0);
    expect(screen.queryByLabelText("Browser connections (not inspected)")).toBeNull();
    expect(screen.queryByLabelText("Requests without an account")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Tighten…" }));
    const sel = (await screen.findByLabelText("Browser connections (not inspected)")) as HTMLSelectElement;
    expect(sel.value).toBe("enabled");
    expect(screen.getByLabelText("Requests without an account")).toBeInTheDocument();
    expect(screen.getByLabelText("Default rule")).toBeInTheDocument();
    fireEvent.change(sel, { target: { value: "disabled" } });
    await waitFor(() => expect(putBody).not.toBeNull());
    expect(putBody!.open_lane_posture).toBe("disabled");
    expect(screen.queryByText("Save draft")).toBeNull();
  });

  it("the Default Rule control carries the LLM-hosts note with a link to AI models (RFE-023)", async () => {
    base({
      "GET /v1/orgs/o1/policy": () =>
        json(200, { scope: "organization", published: gen({ status: "published", draft_revision: null }), draft: gen() }),
      "GET /v1/orgs/o1/policy/warnings": () => json(200, { items: [] }),
    });
    for (const s of [scope, { kind: "organization", id: "o1" } as const]) {
      wrap(<PolicyView title="Policy" scope={s} orgId="o1" orgSlug="acme" canEdit />);
      if (s.kind === "workspace") fireEvent.click(await screen.findByRole("button", { name: "Tighten…" }));
      const note = await screen.findByTestId("default-rule-llm-note");
      expect(note.textContent).toMatch(/Does not affect AI endpoints\. See AI models/);
      expect(note.getAttribute("title")).toMatch(/explicit block rule that names an AI host still applies/);
      expect(screen.getByRole("link", { name: "AI models" })).toHaveAttribute("href", "/o/acme/ai-models");
      expect(screen.getByLabelText("Default rule")).toHaveAttribute("aria-describedby", "default-rule-llm-note");
      cleanup();
    }
  });

  it("prompts to reload on stale_revision", async () => {
    base({
      "PUT /v1/workspaces/w1/policy/draft": () => json(409, { error: { code: "stale_revision", message: "stale" } }),
    });
    wrap(<PolicyView title="Policy" scope={scope} orgId="o1" canEdit />);
    fireEvent.click(await screen.findByLabelText("Move Other up"));
    expect(await screen.findByRole("button", { name: "Reload" })).toBeInTheDocument();
  });

  it("maps policy_invalid errors onto the rule", async () => {
    base({
      "PUT /v1/workspaces/w1/policy/draft": () =>
        json(422, { error: { code: "policy_invalid", message: "bad", details: { fields: [{ path: "/rules/0/name", message: "too long" }] } } }),
    });
    wrap(<PolicyView title="Policy" scope={scope} orgId="o1" canEdit />);
    fireEvent.click(await screen.findByLabelText("Move Other up"));
    expect(await screen.findByText("name: too long")).toBeInTheDocument();
  });
});

describe("SimulationResult", () => {
  it("renders verdict, guidance, credential and trace", () => {
    const sim: Simulation = {
      verdict: "needs_approval", reason: "rule", guidance_code: "approval_required",
      approval_parks: false, approval_timeout_clamped: false,
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
  // (P5-UX-4) The Access tab is the accounts list read for this agent; its detailed tests are in accounts.test.tsx.
  const account = (over: Record<string, unknown>) => ({
    ref: { type: "secret", id: "s1" },
    kind: "website",
    name: "Stripe",
    scope: "workspace",
    levels: [],
    health: { state: "not_used" },
    agents: [],
    can: { give_access: true, change_access: true, manage: true },
    ...over,
  });

  it("shows the accounts the agent can use first and the others collapsed", async () => {
    mockFetch({
      "GET /v1/workspaces/w1/accounts": () =>
        json(200, {
          items: [
            account({
              given: true,
              agents: [{ agent: { id: "a1", name: "Daneel", handle: "daneel" }, workspace: { id: "w1", name: "Ops" }, level: "use" }],
            }),
            account({ ref: { type: "secret", id: "s2" }, name: "Wiki", given: false }),
          ],
          next_cursor: null,
        }),
    });
    wrap(<AccessTab agentId="a1" agentName="Daneel" wsId="w1" orgId="o1" canOwn />);
    expect(await screen.findByLabelText("Accounts Daneel can use")).toBeInTheDocument();
    expect(screen.getByText("Not given to Daneel (1)")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Give Daneel an app" })).toBeInTheDocument();
  });

  it("members see the same list as text and no controls", async () => {
    mockFetch({
      "GET /v1/workspaces/w1/accounts": () =>
        json(200, {
          items: [
            account({
              given: true,
              can: { give_access: false, change_access: false, manage: false },
              agents: [{ agent: { id: "a1", name: "Daneel", handle: "daneel" }, workspace: { id: "w1", name: "Ops" }, level: "use" }],
            }),
          ],
          next_cursor: null,
        }),
    });
    wrap(<AccessTab agentId="a1" agentName="Daneel" wsId="w1" orgId="o1" canOwn={false} />);
    expect(await screen.findByText("Stripe")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /More for/ })).toBeNull();
    expect(screen.queryByText("Add")).toBeNull();
  });
});

describe("simulate speaker (RFE-005)", () => {
  const agent = { id: "a1", name: "Bot", handle: "bot" } as never;
  const users = [
    { id: "u1", label: "Alice (a@x.io)" },
    { id: "u2", label: "Bob (b@x.io)" },
  ];
  const sim = { verdict: "allow", reason: null, guidance_code: null, winning_credential: null, trace: { organization: { verdict: null, considered: [] }, workspace: { verdict: null, considered: [] } } };

  async function run(speakerLabel?: string) {
    let sent: Record<string, unknown> | null = null;
    mockFetch({
      "POST /v1/workspaces/w1/policy/simulate": async (req) => {
        sent = (await req.json()) as Record<string, unknown>;
        return json(200, sim);
      },
    });
    wrap(<SimulatePanel wsId="w1" agents={[agent]} users={users} currentUserId="u1" />);
    fireEvent.change(screen.getByLabelText("Agent"), { target: { value: "a1" } });
    fireEvent.change(screen.getByLabelText("URL"), { target: { value: "https://api.x.com/v1" } });
    if (speakerLabel) fireEvent.change(screen.getByLabelText("Speaker"), { target: { value: speakerLabel } });
    fireEvent.click(screen.getByRole("button", { name: "Check" }));
    await waitFor(() => expect(sent).not.toBeNull());
    return sent as unknown as Record<string, unknown>;
  }

  it("defaults the speaker to the signed-in user", async () => {
    expect((await run())["speaker_user_id"]).toBe("u1");
  });

  it("sends null for 'No speaker' and another member's id when picked", async () => {
    expect((await run("none"))["speaker_user_id"]).toBeNull();
    cleanup();
    expect((await run("u2"))["speaker_user_id"]).toBe("u2");
  });
});
