import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { OrgAiModels, OrgByo } from "@/components/governance/org-governance";
import { ApiError } from "@/lib/api/client";
import type { Org } from "@/lib/api/queries";
import {
  countBlockedHosts,
  formFromDocument,
  formFromEffective,
  formToDocument,
  governanceIssues,
  issuesUnder,
  type EffectiveGovernance,
} from "@/lib/api/governance";
import { wrap } from "./support/render";
import { type Handler, json, mockFetch } from "./support/api";

const eff = (over: Partial<EffectiveGovernance> = {}): EffectiveGovernance => ({
  llm: {
    posture: { value: "open", source: "org" },
    endpoints: [
      {
        id: "anthropic", provider: "anthropic", display_name: "Anthropic",
        permitted: { value: true, source: "instance" },
        key_scopes: { value: ["organization"], source: "instance" },
        allowed_models: { value: ["m1"], source: "org" },
        origin_rule: null,
      },
    ],
  },
  egress: { posture: { value: "credentialed_only", source: "instance" }, open_lane: { value: "enabled", source: "org" }, org_allowlist: [{ host: "pypi.org" }], baseline_allowlist: [{ host: "base.io", path: "/**" }] },
  byo: {
    posture: { value: "owners_only", source: "instance" },
    source_cidrs: { value: null, source: "instance" },
    require_context_credentials: { value: false, source: "instance" },
  },
  agent_tokens: { max_age_days: { value: null, source: "instance" } },
  non_compliant_secrets: [{ id: "s1", name: "my-key", workspace_id: null, reason: "mismatch" }],
  ...over,
});
const doc = {
  byo: { posture: null, source_cidrs: [], require_context_credentials: false },
  agent_tokens: { max_age_days: null },
  llm: { posture: "open", endpoints: { anthropic: { allowed: true, key_scopes: ["organization"], allowed_models: ["m1"], origin: { allowed_organization_ids: ["org-1"], allowed_workspace_ids: [], allowed_team_ids: [] } } } },
  egress: { posture: "open", open_lane: "allowlist_only", allowlist: [{ host: "pypi.org" }, { host: "x.io", path: "/a/**" }] },
};

describe("governance mapping", () => {
  it("round-trips document <-> form and marks instance locks", () => {
    const f = formFromDocument(doc as never, eff());
    expect(f.endpoints[0].locked).toEqual({ permitted: true, key_scopes: true, allowed_models: false });
    expect(f.egressPostureLocked).toBe(true);
    expect(f.openLane).toBe("allowlist_only");
    expect(f.openLaneLocked).toBe(false);
    expect(f.endpoints[0].org_ids).toBe("org-1");
    expect(formToDocument(f)).toEqual(doc);
  });
  it("endpoints absent from the document are not permitted; lists parse from text", () => {
    const f = formFromDocument({ llm: { posture: "open", endpoints: {} } } as never, eff());
    expect(f.endpoints[0].allowed).toBe(false);
    f.endpoints[0].allowed_models = "a, b\nc";
    expect(formToDocument(f).llm.endpoints.anthropic.allowed_models).toEqual(["a", "b", "c"]);
  });
  it("open lane defaults to the effective value and is locked by the instance", () => {
    const e = eff({ egress: { ...eff().egress, open_lane: { value: "disabled", source: "instance" } } });
    const f = formFromDocument({ egress: { posture: "open", allowlist: [] } } as never, e);
    expect(f.openLane).toBe("disabled");
    expect(f.openLaneLocked).toBe(true);
    expect(formFromEffective(e).openLane).toBe("disabled");
  });
  it("member form shows effective values", () => {
    expect(formFromEffective(eff()).endpoints[0].allowed_models).toBe("m1");
  });
  it("maps field errors by JSON path", () => {
    const err = new ApiError("governance_invalid", "bad", {
      fields: [
        { path: "/llm/endpoints/anthropic/allowed_models/0", message: "not in instance list" },
        { path: "egress.allowlist[2].host", message: "bad host" },
        { message: "general" },
      ],
    }, 422);
    const i = governanceIssues(err);
    expect(issuesUnder(i, "/llm/endpoints/anthropic/allowed_models")).toEqual(["0: not in instance list"]);
    expect(issuesUnder(i, "/egress/allowlist/2")).toEqual(["host: bad host"]);
    expect(i.general).toEqual(["general"]);
  });
  it("counts blocked hosts", () => {
    expect(countBlockedHosts([{ host: "a" }, { host: "b" }, { host: "b" }])[0]).toEqual({ host: "b", count: 2 });
  });
});

const routes = (extra: Record<string, Handler> = {}) => ({
  "GET /v1/orgs/o1/governance/effective": () => json(200, eff()),
  "GET /v1/orgs/o1/governance": () => json(200, { document: doc, revision: 4 }),
  "GET /v1/governance/instance": () => json(200, { llm: { allowed_key_scopes: [], minimum_posture: "open", require_key_origin: false, known_llm_hosts: [], endpoints: [] }, egress: { minimum_posture: "open", open_lane_minimum: "enabled", baseline_allowlist: [] }, byo: { enabled: true, minimum_posture: "owners_only", default_source_cidrs: [], require_context_credentials: false } }),
  "GET /v1/orgs/o1/workspaces": () => json(200, { items: [{ id: "w1", slug: "main", name: "Main" }], next_cursor: null }),
  "GET /v1/workspaces/w1/activity": () => json(200, { items: [{ host: "blocked.dev" }], next_cursor: null }),
  ...extra,
});

const ORG = { id: "o1", slug: "acme", name: "Acme", my_role: "owner" } as unknown as Org;

describe("OrgByo (P5-IA-3, C-967)", () => {
  it("shows Administration's BYO limits as locked rows above the organization's own controls", async () => {
    const e = eff({ agent_tokens: { max_age_days: { value: 90, source: "instance" } } });
    mockFetch(routes({ "GET /v1/orgs/o1/governance/effective": () => json(200, e), "GET /v1/orgs/o1/agent-tokens": () => json(200, { items: [], next_cursor: null }) }));
    wrap(<OrgByo org={ORG} />);
    const above = await screen.findByRole("list", { name: "Limits set above this level" });
    const rows = within(above).getAllByRole("listitem").map((r) => r.textContent ?? "");
    expect(rows.some((r) => /Who may run agents outside Aurora/.test(r) && /Only workspace owners/.test(r) && /Set for you by an Aurora administrator/.test(r))).toBe(true);
    expect(rows.some((r) => /Longest a token lasts/.test(r) && /Up to 90 days/.test(r))).toBe(true);
    expect(screen.getByLabelText("Maximum token age (days)")).toBeTruthy();
    expect(screen.queryByLabelText("Allowed models")).toBeNull();
  });

  it("has no limit rows when Administration sets none", async () => {
    const e = eff({
      byo: { posture: { value: "enabled", source: "org" }, source_cidrs: { value: null, source: "org" }, require_context_credentials: { value: false, source: "org" } },
      agent_tokens: { max_age_days: { value: null, source: "org" } },
    });
    mockFetch(routes({ "GET /v1/orgs/o1/governance/effective": () => json(200, e), "GET /v1/orgs/o1/agent-tokens": () => json(200, { items: [], next_cursor: null }) }));
    wrap(<OrgByo org={ORG} />);
    await screen.findByLabelText("Who may create BYO agents");
    expect(screen.queryByRole("list", { name: "Limits set above this level" })).toBeNull();
  });
});

describe("OrgAiModels (P5-IA-3, C-966)", () => {
  it("members see read-only controls and locked labels, and no Save", async () => {
    // The organization's accounts call is 403 for a member: the key comes from the workspace's list (scope "organization").
    mockFetch(routes({
      "GET /v1/orgs/o1/accounts": () => json(403, { error: { code: "forbidden", message: "no" } }),
      "GET /v1/workspaces/w1/accounts": () =>
        json(200, { items: [{ kind: "ai_key", scope: "organization", name: "Users AI gateway", ref: { type: "secret", id: "k1" }, agents: [], levels: [] }], next_cursor: null }),
    }));
    wrap(<OrgAiModels org={{ ...ORG, my_role: "member" }} />);
    // (P5-POLISH-9, D-390) A member reads a summary, never a disabled form.
    expect(await screen.findByTestId("member-ai-models")).toBeTruthy();
    expect(await screen.findByText("Users AI gateway")).toBeTruthy();
    expect(screen.queryByText("The organization has no key of its own.")).toBeNull();
    expect(screen.getByTestId("who-changes")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(screen.queryByLabelText("Allowed models")).toBeNull();
  });

  it("a member's key row says its default model, and nothing when it is unknown (P5-APIGAP-3, C-1048)", async () => {
    mockFetch(routes({
      "GET /v1/workspaces/w1/accounts": () =>
        json(200, { items: [
          { kind: "ai_key", scope: "organization", name: "Users AI gateway", default_model: "glm-5.3", ref: { type: "secret", id: "k1" }, agents: [], levels: [] },
          { kind: "ai_key", scope: "organization", name: "Spare key", default_model: null, ref: { type: "secret", id: "k2" }, agents: [], levels: [] },
        ], next_cursor: null }),
    }));
    wrap(<OrgAiModels org={{ ...ORG, my_role: "member" }} />);
    expect((await screen.findByTestId("key-default-model")).textContent).toBe(" · default glm-5.3");
    expect(screen.getAllByTestId("key-default-model")).toHaveLength(1);
  });

  it("has one page: no tabs, no Egress, no Apps, no Compliance", async () => {
    mockFetch(routes());
    wrap(<OrgAiModels org={ORG} />);
    await screen.findByText("Anthropic");
    expect(screen.queryAllByRole("tab")).toHaveLength(0);
    expect(screen.queryByLabelText(/Browser connections/)).toBeNull();
  });

  it("shows Administration's limits as locked rows above the organization's rules, with who can change them", async () => {
    mockFetch(routes());
    wrap(<OrgAiModels org={ORG} />);
    const above = await screen.findByRole("list", { name: "Limits set above this level" });
    const rows = within(above).getAllByRole("listitem").map((r) => r.textContent ?? "");
    // The instance decides the endpoint's permission and key scopes; the posture and allowed models are the organization's.
    expect(rows.some((r) => /AI endpoints organizations may use/.test(r) && /Anthropic allowed/.test(r))).toBe(true);
    expect(rows.some((r) => /Who may add Anthropic keys/.test(r) && /keys for the whole organization/.test(r))).toBe(true);
    expect(rows.every((r) => /Set for you by an Aurora administrator/.test(r) && /An Aurora administrator can set this up|administrator/.test(r))).toBe(true);
    expect(rows.some((r) => /Which AI keys agents may use/.test(r))).toBe(false);
    // (P5-REFINE-20, org-7) The page opens with the summary; what is set above comes last, in a collapsed disclosure.
    const summary = screen.getByTestId("models-summary");
    expect(summary.compareDocumentPosition(above) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const disclosure = screen.getByTestId("models-above") as HTMLDetailsElement;
    expect(disclosure.open).toBe(false);
    expect(disclosure.textContent).toContain("Set above you (");
    expect(disclosure.contains(above)).toBe(true);
    expect(screen.queryByText("Rules for Acme")).toBeNull();
  });

  it("(org-7) opens with the summary sentence and plain rows, endpoints and Advanced collapsed, origin as names", async () => {
    // An admin reads the organization's own list; the first workspace (here one the admin is not in: 404) is not needed.
    mockFetch(routes({
      "GET /v1/workspaces/w1/accounts": () => json(404, { error: { code: "not_found", message: "nf" } }),
      "GET /v1/orgs/o1/accounts": () =>
        json(200, { items: [{ kind: "ai_key", scope: "organization", name: "Company key", ref: { type: "secret", id: "k1" }, agents: [], levels: [], starter: true }], next_cursor: null }),
    }));
    wrap(<OrgAiModels org={ORG} />);
    await waitFor(() => expect(screen.getByTestId("models-summary").textContent).toBe("Agents in Acme use Company key. People may not add their own."));
    expect(screen.getByText("Key agents use")).toBeTruthy();
    expect(screen.queryByText("No company key yet")).toBeNull();
    // (axe definition-list, dlitem) Every row is one div directly under the dl holding only dt and dd, the switch and the
    // team picker inside a dd.
    const dl = screen.getByText("Key agents use").closest("dl")!;
    for (const row of Array.from(dl.children)) {
      expect(row.tagName).toBe("DIV");
      expect(Array.from(row.children).every((c) => c.tagName === "DT" || c.tagName === "DD")).toBe(true);
    }
    expect(within(dl.querySelectorAll("dd")[2]!).getByRole("switch", { name: "People may add their own key" })).toBeTruthy();
    expect(screen.getByText("Everyone in Acme")).toBeTruthy();
    expect(screen.getByText("Endpoints (1)")).toBeTruthy();
    expect((screen.getByTestId("models-endpoints") as HTMLDetailsElement).open).toBe(false);
    expect((screen.getByTestId("models-advanced") as HTMLDetailsElement).open).toBe(false);
    // No typed ids: the origin settings are pick lists, and the old jargon is gone.
    expect(screen.queryByText(/Origin: organization ids/)).toBeNull();
    expect(screen.queryByText("LLM posture")).toBeNull();
    expect(screen.getByRole("list", { name: "Organizations" })).toBeTruthy();
  });

  it("names the posture as a limit when the instance sets it", async () => {
    const e = eff({ llm: { ...eff().llm, posture: { value: "sanctioned_only", source: "instance" } } });
    mockFetch(routes({ "GET /v1/orgs/o1/governance/effective": () => json(200, e) }));
    wrap(<OrgAiModels org={ORG} />);
    const above = await screen.findByRole("list", { name: "Limits set above this level" });
    expect(within(above).getByText("Only keys an administrator approved")).toBeTruthy();
  });

  it("admins save with revision (Save is off until something changed); stale revision prompts reload", async () => {
    let body: { revision: number } | null = null;
    mockFetch(routes({
      "PUT /v1/orgs/o1/governance": async (req) => {
        body = await req.json();
        return json(409, { error: { code: "stale_revision", message: "stale" } });
      },
    }));
    wrap(<OrgAiModels org={ORG} />);
    await screen.findByLabelText("Allowed models");
    // (the shared save bar) There is no Save until something changed; then the bar offers Discard and Save.
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Discard changes" })).toBeNull();
    fireEvent.change(screen.getByLabelText("Allowed models"), { target: { value: "m1\nm2" } });
    const save = (await screen.findByRole("button", { name: "Save" })) as HTMLButtonElement;
    expect(save.disabled).toBe(false);
    fireEvent.click(save);
    expect(await screen.findByRole("button", { name: "Reload latest" })).toBeTruthy();
    expect(body!.revision).toBe(4);
  });

  it("an admin narrows the allowed models and saves them in the document; Discard puts them back", async () => {
    let body: { document: { llm: { endpoints: { anthropic: { allowed_models: string[] } } } } } | null = null;
    let revision = 4;
    mockFetch(routes({
      "GET /v1/orgs/o1/governance": () => json(200, { document: doc, revision }),
      "PUT /v1/orgs/o1/governance": async (req) => {
        body = await req.json();
        revision = 5;
        return json(200, { document: doc, revision });
      },
    }));
    wrap(<OrgAiModels org={ORG} />);
    const models =(await screen.findByLabelText("Allowed models")) as HTMLTextAreaElement;
    fireEvent.change(models, { target: { value: "mx" } });
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect((screen.getByLabelText("Allowed models") as HTMLTextAreaElement).value).toBe("m1");
    fireEvent.change(screen.getByLabelText("Allowed models"), { target: { value: "m2" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(body).not.toBeNull());
    expect(body!.document.llm.endpoints.anthropic.allowed_models).toEqual(["m2"]);
    // The save brings a new revision, which starts a fresh form; the confirmation survives that.
    expect(await screen.findByText(/Saved\. Existing keys were re-checked/)).toBeTruthy();
  });

  it("says how many AI keys no longer follow the rules and links to the Accounts lists filtered to AI keys", async () => {
    const e = eff({
      non_compliant_secrets: [
        { id: "s1", name: "my-key", workspace_id: null, reason: "mismatch" },
        { id: "s2", name: "ws-key", workspace_id: "w1", reason: "not_permitted" },
      ],
    });
    mockFetch(routes({ "GET /v1/orgs/o1/governance/effective": () => json(200, e) }));
    wrap(<OrgAiModels org={ORG} />);
    const notice = await screen.findByTestId("keys-notice");
    expect(notice.textContent).toMatch(/2 AI keys no longer follow these rules/);
    expect(within(notice).getByRole("link", { name: /Acme's Accounts/ }).getAttribute("href")).toBe("/o/acme/apps?tab=accounts&kind=ai-key");
    await waitFor(() => expect(within(notice).getByRole("link", { name: /Main's Accounts/ })).toBeTruthy());
    expect(within(notice).getByRole("link", { name: /Main's Accounts/ }).getAttribute("href")).toMatch(/\/o\/acme\/w\/main\/.*kind=ai-key/);
  });

  it("says one key in the singular, and nothing is wrong when none", async () => {
    mockFetch(routes());
    wrap(<OrgAiModels org={ORG} />);
    expect((await screen.findByTestId("keys-notice")).textContent).toMatch(/1 AI key no longer follows these rules/);
    cleanup();
    mockFetch(routes({ "GET /v1/orgs/o1/governance/effective": () => json(200, eff({ non_compliant_secrets: [] })) }));
    wrap(<OrgAiModels org={ORG} />);
    // (org-7) No green box that explains nothing: the notice is said only when a key does not follow.
    await screen.findByTestId("models-summary");
    expect(screen.queryByText("All AI keys follow these rules.")).toBeNull();
    expect(screen.queryByTestId("keys-notice")).toBeNull();
  });
});
