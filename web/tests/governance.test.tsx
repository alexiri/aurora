import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { GovernanceView } from "@/components/governance/governance-view";
import { ApiError } from "@/lib/api/client";
import {
  countBlockedHosts,
  formFromDocument,
  formFromEffective,
  formToDocument,
  governanceIssues,
  issuesUnder,
  type EffectiveGovernance,
} from "@/lib/api/governance";

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
  non_compliant_secrets: [{ id: "s1", name: "my-key", workspace_id: null, reason: "mismatch" }],
  ...over,
});
const doc = {
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
  "GET /v1/governance/instance": () => json(200, { llm: { allowed_key_scopes: [], minimum_posture: "open", require_key_origin: false, known_llm_hosts: [], endpoints: [] }, egress: { minimum_posture: "open", open_lane_minimum: "enabled", baseline_allowlist: [] } }),
  "GET /v1/orgs/o1/workspaces": () => json(200, { items: [{ id: "w1", slug: "main", name: "Main" }], next_cursor: null }),
  "GET /v1/workspaces/w1/activity": () => json(200, { items: [{ host: "blocked.dev" }], next_cursor: null }),
  ...extra,
});

describe("GovernanceView", () => {
  it("members see read-only controls and locked labels", async () => {
    mockFetch(routes());
    wrap(<GovernanceView orgId="o1" orgSlug="acme" canEdit={false} />);
    expect(await screen.findByText("Anthropic")).toBeTruthy();
    expect(screen.getAllByText("enforced by instance").length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: "Save governance" })).toBeNull();
    expect((screen.getByLabelText("Allowed models") as HTMLTextAreaElement).disabled).toBe(true);
  });

  it("egress tab edits the browser-tunnel posture and saves it in the document", async () => {
    let body: { document: { egress: { open_lane: string } } } | null = null;
    mockFetch(routes({
      "PUT /v1/orgs/o1/governance": async (req) => {
        body = await req.json();
        return json(200, { document: doc, revision: 5 });
      },
    }));
    wrap(<GovernanceView orgId="o1" orgSlug="acme" canEdit />);
    fireEvent.click(await screen.findByRole("tab", { name: "Egress" }));
    const sel = screen.getByLabelText(/Browser tunnels/) as HTMLSelectElement;
    expect(sel.value).toBe("allowlist_only");
    fireEvent.change(sel, { target: { value: "disabled" } });
    fireEvent.click(screen.getByRole("button", { name: "Save governance" }));
    await waitFor(() => expect(body).not.toBeNull());
    expect(body!.document.egress.open_lane).toBe("disabled");
  });

  it("admins save with revision; stale revision prompts reload", async () => {
    let body: { revision: number } | null = null;
    mockFetch(routes({
      "PUT /v1/orgs/o1/governance": async (req) => {
        body = await req.json();
        return json(409, { error: { code: "stale_revision", message: "stale" } });
      },
    }));
    wrap(<GovernanceView orgId="o1" orgSlug="acme" canEdit />);
    fireEvent.click(await screen.findByRole("button", { name: "Save governance" }));
    expect(await screen.findByRole("button", { name: "Reload latest" })).toBeTruthy();
    expect(body!.revision).toBe(4);
  });

  it("shows compliance list with a fix link and blocked-host helper", async () => {
    mockFetch(routes());
    wrap(<GovernanceView orgId="o1" orgSlug="acme" canEdit />);
    fireEvent.click(await screen.findByRole("tab", { name: /Compliance/ }));
    expect(screen.getByRole("link", { name: "Fix" }).getAttribute("href")).toBe("/o/acme/credentials");
    fireEvent.click(screen.getByRole("tab", { name: "Egress" }));
    await waitFor(() => expect(screen.getByText("blocked.dev")).toBeTruthy());
  });
});
