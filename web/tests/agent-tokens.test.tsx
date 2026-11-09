import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ConnectTab } from "@/components/agents/connect-tab";
import { AgentRevealDialog } from "@/components/agents/reveal";
import { AgentsList } from "@/components/agents-list";
import { GovernanceView } from "@/components/governance/governance-view";
import {
  expiryText,
  lastUsedText,
  mergeTokens,
  tokenStates,
  type AgentTokenEntry,
} from "@/lib/api/agent-tokens";
import {
  formFromDocument,
  formToDocument,
  maxAgeDaysProblem,
  parseMaxAgeDays,
  type EffectiveGovernance,
} from "@/lib/api/governance";

// (phase 3, RFE-013, SEC-53) Dashboard: the governance field, the Connect tab's expiry info and
// the agent-list badges (stale, expiring, expired; owners only).

vi.mock("next/navigation", () => ({ useRouter: () => ({ replace: vi.fn() }) }));

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
function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const NOW = Date.parse("2026-06-01T00:00:00Z");
const DAY = 86_400_000;
const iso = (offsetDays: number) => new Date(NOW + offsetDays * DAY).toISOString();

describe("token states and texts", () => {
  it("expired beats expiring; stale is independent; healthy and absent show nothing", () => {
    expect(tokenStates({ expires_at: iso(-1), stale: false, expiring: true }, NOW)).toEqual(["expired"]);
    expect(tokenStates({ expires_at: iso(5), stale: false, expiring: true }, NOW)).toEqual(["expiring"]);
    expect(tokenStates({ expires_at: iso(5), stale: true, expiring: true }, NOW)).toEqual(["expiring", "stale"]);
    expect(tokenStates({ expires_at: iso(-3), stale: true, expiring: true }, NOW)).toEqual(["expired", "stale"]);
    expect(tokenStates({ expires_at: null, stale: true, expiring: false }, NOW)).toEqual(["stale"]);
    expect(tokenStates({ expires_at: iso(40), stale: false, expiring: false }, NOW)).toEqual([]);
    expect(tokenStates(null, NOW)).toEqual([]);
    expect(tokenStates(undefined, NOW)).toEqual([]);
  });
  it("words the expiry and the last use", () => {
    expect(expiryText(null, NOW)).toBe("Never expires");
    expect(expiryText(iso(12), NOW)).toBe("Expires in 12 days");
    expect(expiryText(iso(1.5), NOW)).toBe("Expires in 2 days");
    expect(expiryText(new Date(NOW + 3600_000).toISOString(), NOW)).toBe("Expires within a day");
    expect(expiryText(iso(-3), NOW)).toBe("Expired 3 days ago");
    expect(expiryText(new Date(NOW - 3600_000).toISOString(), NOW)).toBe("Expired today");
    expect(lastUsedText(null)).toBe("Never used");
    expect(lastUsedText(iso(-1))).toMatch(/^Last used /);
  });
  it("merges the stale and expiring lists by agent, soonest expiry first", () => {
    const e = (id: string, h: string, exp: string | null): AgentTokenEntry =>
      ({
        agent: { id, handle: h, name: h },
        workspace: { id: "w", slug: "w", name: "W" },
        display_prefix: "aug_x",
        created_at: iso(-50),
        expires_at: exp,
        last_used_at: null,
        stale: true,
        expiring: exp !== null,
      }) as AgentTokenEntry;
    const m = mergeTokens([e("1", "b", iso(10)), e("2", "a", iso(-1)), e("1", "b", iso(10)), e("3", "c", null)]);
    expect(m.map((x) => x.agent.handle)).toEqual(["a", "b", "c"]);
  });
});

describe("maximum age field", () => {
  it("parses whole days within bounds, empty as none, anything else as invalid", () => {
    expect(parseMaxAgeDays("")).toBeNull();
    expect(parseMaxAgeDays(" 30 ")).toBe(30);
    expect(parseMaxAgeDays("1")).toBe(1);
    expect(parseMaxAgeDays("3650")).toBe(3650);
    for (const bad of ["0", "3651", "-1", "1.5", "abc", "12345"]) expect(parseMaxAgeDays(bad)).toBeUndefined();
  });
  it("only tightens the instance value", () => {
    expect(maxAgeDaysProblem("", 30)).toBeUndefined();
    expect(maxAgeDaysProblem("30", 30)).toBeUndefined();
    expect(maxAgeDaysProblem("31", 30)).toMatch(/instance limit is 30/);
    expect(maxAgeDaysProblem("99", null)).toBeUndefined();
    expect(maxAgeDaysProblem("x", null)).toMatch(/whole number/);
  });
});

const eff = (days: number | null, source = "org"): EffectiveGovernance => ({
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
  },
  agent_tokens: { max_age_days: { value: days, source } },
  non_compliant_secrets: [],
});
const instance = (days: number | null) => ({
  llm: { allowed_key_scopes: [], minimum_posture: "open", require_key_origin: false, known_llm_hosts: [], endpoints: [] },
  egress: { minimum_posture: "open", open_lane_minimum: "enabled", baseline_allowlist: [] },
  byo: { enabled: true, minimum_posture: "owners_only", default_source_cidrs: [], require_context_credentials: false },
  agent_tokens: { max_age_days: days },
});
const emptyDoc = {
  llm: { posture: "open", endpoints: {} },
  egress: { posture: "open", open_lane: "enabled", allowlist: [] },
  byo: { posture: null, source_cidrs: [], require_context_credentials: false },
  agent_tokens: { max_age_days: null },
};

describe("governance document mapping", () => {
  it("round-trips the org value and knows the instance value", () => {
    const f = formFromDocument({ ...emptyDoc, agent_tokens: { max_age_days: 14 } } as never, eff(14), instance(30) as never);
    expect(f.byo.maxAgeDays).toBe("14");
    expect(f.byo.instanceMaxAgeDays).toBe(30);
    expect(formToDocument(f).agent_tokens).toEqual({ max_age_days: 14 });
    const none = formFromDocument(emptyDoc as never, eff(null, "instance"), instance(null) as never);
    expect(none.byo.maxAgeDays).toBe("");
    expect(none.byo.instanceMaxAgeDays).toBeNull();
    expect(formToDocument(none).agent_tokens).toEqual({ max_age_days: null });
  });
});

const routes = (over: Record<string, Handler> = {}, days: number | null = null, instDays: number | null = null) => ({
  "GET /v1/orgs/o1/governance/effective": () => json(200, eff(days, instDays != null && days === instDays ? "instance" : "org")),
  "GET /v1/orgs/o1/governance": () => json(200, { document: emptyDoc, revision: 4 }),
  "GET /v1/governance/instance": () => json(200, instance(instDays)),
  "GET /v1/orgs/o1/workspaces": () => json(200, { items: [], next_cursor: null }),
  "GET /v1/orgs/o1/agent-tokens": () => json(200, { items: [], next_cursor: null }),
  ...over,
});

describe("governance page: BYO tab", () => {
  it("shows the effective maximum with its source and saves the org value", async () => {
    let body: { document: { agent_tokens: unknown } } | null = null;
    mockFetch(
      routes(
        {
          "PUT /v1/orgs/o1/governance": async (req) => {
            body = await req.json();
            return json(200, { document: emptyDoc, revision: 5 });
          },
        },
        30,
        30,
      ),
    );
    wrap(<GovernanceView orgId="o1" orgSlug="acme" canEdit />);
    fireEvent.click(await screen.findByRole("tab", { name: "BYO agents" }));
    expect(screen.getByTestId("effective-max-age").textContent).toBe("30 days");
    expect(screen.getByTestId("effective-max-age").parentElement?.textContent).toMatch(/set by instance/);
    expect(screen.getByText(/enforced by instance: 30 days/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Maximum token age (days)"), { target: { value: "14" } });
    fireEvent.click(screen.getByRole("button", { name: "Save governance" }));
    await waitFor(() => expect(body).not.toBeNull());
    expect(body!.document.agent_tokens).toEqual({ max_age_days: 14 });
  });

  it("refuses a value above the instance limit or out of range before saving", async () => {
    const put = vi.fn(() => json(200, { document: emptyDoc, revision: 5 }));
    mockFetch(routes({ "PUT /v1/orgs/o1/governance": put }, null, 30));
    wrap(<GovernanceView orgId="o1" orgSlug="acme" canEdit />);
    fireEvent.click(await screen.findByRole("tab", { name: "BYO agents" }));
    const input = screen.getByLabelText("Maximum token age (days)");
    const save = screen.getByRole("button", { name: "Save governance" }) as HTMLButtonElement;
    fireEvent.change(input, { target: { value: "31" } });
    expect(screen.getByRole("alert").textContent).toMatch(/instance limit is 30/);
    expect(save.disabled).toBe(true);
    fireEvent.change(input, { target: { value: "0" } });
    expect(save.disabled).toBe(true);
    fireEvent.change(input, { target: { value: "" } });
    expect(save.disabled).toBe(false);
    expect(put).not.toHaveBeenCalled();
  });

  it("shows the server's own refusal on the field", async () => {
    mockFetch(
      routes({
        "PUT /v1/orgs/o1/governance": () =>
          json(422, {
            error: {
              code: "governance_below_instance_minimum",
              message: "not valid",
              details: { fields: [{ path: "agent_tokens.max_age_days", message: "longer than the instance maximum age" }] },
            },
          }),
      }),
    );
    wrap(<GovernanceView orgId="o1" orgSlug="acme" canEdit />);
    fireEvent.click(await screen.findByRole("tab", { name: "BYO agents" }));
    fireEvent.change(screen.getByLabelText("Maximum token age (days)"), { target: { value: "50" } });
    fireEvent.click(screen.getByRole("button", { name: "Save governance" }));
    expect(await screen.findByText("longer than the instance maximum age")).toBeTruthy();
  });

  it("lists the tokens that need attention for admins and not for members", async () => {
    const entry = (id: string, exp: string | null, stale: boolean) => ({
      agent: { id, handle: `bot-${id}`, name: `Bot ${id}` },
      workspace: { id: "w1", slug: "main", name: "Main" },
      display_prefix: "aug_ab12",
      created_at: iso(-90),
      expires_at: exp,
      last_used_at: null,
      stale,
      expiring: exp !== null,
    });
    mockFetch(
      routes({
        "GET /v1/orgs/o1/agent-tokens": (req) => {
          const q = new URL(req.url).searchParams;
          return json(200, {
            items: q.get("stale") ? [entry("2", null, true)] : [entry("1", new Date(Date.now() - DAY).toISOString(), false)],
            next_cursor: null,
          });
        },
      }),
    );
    wrap(<GovernanceView orgId="o1" orgSlug="acme" canEdit />);
    fireEvent.click(await screen.findByRole("tab", { name: "BYO agents" }));
    const list = await screen.findByRole("list", { name: "BYO tokens that are unused or expiring" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toMatch(/Bot 1/);
    expect(rows[0].textContent).toMatch(/Token expired/);
    expect(rows[1].textContent).toMatch(/Token unused/);
    cleanup();

    const fetchMock = mockFetch({
      "GET /v1/orgs/o1/governance/effective": () => json(200, eff(null, "instance")),
    });
    wrap(<GovernanceView orgId="o1" orgSlug="acme" canEdit={false} />);
    fireEvent.click(await screen.findByRole("tab", { name: "BYO agents" }));
    expect(screen.queryByTestId("token-attention")).toBeNull();
    expect(fetchMock.mock.calls.some(([r]) => String((r as Request).url).includes("agent-tokens"))).toBe(false);
  });
});

const agent = (token: Record<string, unknown> | null, kind = "byo") => ({
  id: "a1", org_id: "o1", workspace_id: "w1", handle: "bot", name: "Bot", kind,
  is_default: false, harness: null, model_provider: null, model_endpoint_id: null, model_id: null,
  effort: null, proactive_messaging: "ask", brief: "", avatar_url: null, effective_frozen: false, frozen: null,
  created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
  byo: kind === "byo"
    ? {
        posture: "enabled", posture_level: "instance", source_cidrs: null, require_context: false,
        effective_source_cidrs: null, effective_source_level: null, effective_require_context: false,
        effective_require_context_level: null, token,
      }
    : null,
});
const tokenView = (over: Record<string, unknown> = {}) => ({
  display_prefix: "aug_ab12", created_at: iso(-60), expires_at: null, last_used_at: null, stale: false, expiring: false, ...over,
});

describe("agent list badges", () => {
  it("shows expired, expiring and stale badges to owners only", () => {
    const agents = [
      { ...agent(tokenView({ expires_at: new Date(Date.now() - DAY).toISOString(), expiring: true })), id: "1", handle: "gone", name: "Gone" },
      { ...agent(tokenView({ expires_at: new Date(Date.now() + 5 * DAY).toISOString(), expiring: true })), id: "2", handle: "soon", name: "Soon" },
      { ...agent(tokenView({ stale: true })), id: "3", handle: "idle", name: "Idle" },
      { ...agent(tokenView()), id: "4", handle: "fine", name: "Fine" },
      { ...agent(null, "hosted"), id: "5", handle: "host", name: "Host" },
    ] as never;
    const { unmount } = render(<AgentsList agents={agents} showTokenBadges />);
    const row = (name: string) => screen.getByText(name).closest("li") as HTMLElement;
    expect(row("Gone").textContent).toMatch(/Token expired/);
    expect(row("Gone").textContent).not.toMatch(/Token expiring/);
    expect(row("Soon").textContent).toMatch(/Token expiring/);
    expect(row("Idle").textContent).toMatch(/Token unused/);
    expect(row("Fine").textContent).not.toMatch(/Token/);
    expect(row("Host").textContent).not.toMatch(/Token/);
    unmount();
    render(<AgentsList agents={agents} />);
    expect(document.body.textContent).not.toMatch(/Token (expired|expiring|unused)/);
  });
});

describe("Connect tab", () => {
  const config = {
    proxy_url: "https://<AGENT_TOKEN>@gw:1", env: {}, ca_pem: "", ca_path: "/ca.pem", proxy_ca_path: "/ca.pem",
    proxy_tls: null, llm_env: {}, stubs: [], notes: [], byo: null,
  };
  const base = {
    "GET /v1/agents/a1/container-config": () => json(200, config),
    "GET /v1/gateway/ca.pem": () => new Response("pem"),
  };

  it("shows the prefix, expiry, last use and badges; never a token", async () => {
    mockFetch(base);
    wrap(
      <ConnectTab
        agent={agent(tokenView({ expires_at: new Date(Date.now() + 5 * DAY).toISOString(), expiring: true, stale: true })) as never}
        wsId="w1"
        canOwn
      />,
    );
    const info = await screen.findByTestId("token-info");
    expect(info.textContent).toMatch(/aug_ab12…/);
    expect(info.textContent).toMatch(/Expires in 5 days/);
    expect(info.textContent).toMatch(/Never used/);
    expect(info.textContent).toMatch(/Token expiring/);
    expect(info.textContent).toMatch(/Token unused/);
  });

  it("says when there is no token or it never expires", async () => {
    mockFetch(base);
    const { unmount } = wrap(<ConnectTab agent={agent(null) as never} wsId="w1" canOwn />);
    expect((await screen.findByTestId("token-info")).textContent).toMatch(/no live token/);
    unmount();
    wrap(<ConnectTab agent={agent(tokenView()) as never} wsId="w1" canOwn />);
    expect((await screen.findByTestId("token-info")).textContent).toMatch(/Never expires/);
  });

  it("regenerating reveals the new expiry once", async () => {
    mockFetch({
      ...base,
      "POST /v1/agents/a1/token": () =>
        json(200, { token: "aug_NEW", container_config: config, expires_at: iso(30) }),
    });
    wrap(<ConnectTab agent={agent(tokenView()) as never} wsId="w1" canOwn />);
    fireEvent.click(await screen.findByRole("button", { name: "Regenerate token" }));
    fireEvent.click(screen.getByRole("button", { name: "Yes, regenerate" }));
    const note = await screen.findByTestId("revealed-expiry");
    expect(note.textContent).toMatch(/expires on/);
  });
});

describe("one-time reveal", () => {
  it("states that a token without a maximum age does not expire", () => {
    render(
      <AgentRevealDialog
        revealed={{ token: "aug_X", config: null, agentName: "Bot", expiresAt: null }}
        onClose={() => {}}
      />,
    );
    expect(screen.getByTestId("revealed-expiry").textContent).toBe("This token does not expire.");
  });
});
