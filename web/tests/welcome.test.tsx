import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops" }),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
  usePathname: () => "/o/acme/w/ops/welcome",
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

import { AccessTab } from "@/components/agents/access-tab";
import { WelcomeView, roleSentence } from "@/components/onboarding/welcome-view";

// (P5-UX-7, C-927) The Welcome page: its title, its three cards from the five sentences with real names, and the agent's
// summary on Welcome and on the Access tab.

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request) => Response | Promise<Response>;
let routes: Record<string, Handler>;

beforeEach(() => {
  routes = {};
  window.history.replaceState(null, "", "/");
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
      const h = routes[`${req.method} ${new URL(req.url, "http://localhost").pathname}`];
      return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
});
const wrap = (ui: React.ReactElement) => render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);

const AGENT = { id: "a1", handle: "daneel", name: "Daneel", kind: "hosted", is_default: true, frozen: null };
const account = (over: Record<string, unknown> = {}) => ({
  ref: { type: "connection", id: "c1" },
  kind: "app",
  name: "Linear account",
  app: { app_id: "linear", name: "Linear", custom_only: false },
  scope: "workspace",
  levels: ["read", "ask", "act"],
  health: { state: "ok" },
  agents: [{ agent: { id: "a1", name: "Daneel", handle: "daneel" }, workspace: { id: "w1", name: "Ops" }, level: "ask", always_allowed: [] }],
  can: { give_access: true, change_access: true, manage: true },
  given: true,
  ...over,
});

function world(o: { role?: string; personal?: boolean; name?: string; accounts?: unknown[]; rules?: unknown[] } = {}) {
  routes["GET /v1/resolve"] = () => json(200, { org: { id: "o1", slug: "acme", name: "Acme" }, workspace: { id: "w1", slug: "ops", name: o.name ?? "Ops" } });
  routes["GET /v1/orgs/o1"] = () => json(200, { id: "o1", slug: "acme", name: "Acme" });
  routes["GET /v1/workspaces/w1"] = () => json(200, { id: "w1", org_id: "o1", slug: "ops", name: o.name ?? "Ops", my_role: o.role ?? "owner" });
  routes["GET /v1/workspaces/w1/agents"] = () => json(200, { items: [AGENT], next_cursor: null });
  routes["GET /v1/me"] = () =>
    json(200, { user: { display_name: "Bob", email: "bob@x" }, memberships: [{ org: { id: "o1", slug: "acme", name: "Acme" }, status: "active", workspaces: [{ id: "w1", slug: "ops", name: "Ops", personal: o.personal === true }] }] });
  routes["GET /v1/me/onboarding"] = () => json(200, { welcome_seen: false, guide_dismissed: [], tour_completed_at: null });
  routes["GET /v1/workspaces/w1/accounts"] = () => json(200, { items: o.accounts ?? [], next_cursor: null });
  routes["GET /v1/orgs/o1/governance/effective"] = () =>
    json(200, { egress: { posture: { value: "open", source: "default" }, baseline_allowlist: [], org_allowlist: [] } });
  // (FIX-164, C-1020) The published rules: every member of the workspace reads them, and the summary says them.
  routes["GET /v1/workspaces/w1/internet-access"] = () =>
    json(200, { number: 1, default_action: "none", uncredentialed_default: "allow", uncredentialed_allowlist: [], open_lane_posture: "enabled", rules: o.rules ?? [] });
  routes["GET /v1/agents/a1/model-readiness"] = () => json(200, { harness: "claude-code", keys: [], override_dropped: false, default: null });
  routes["GET /v1/workspaces/w1/secrets"] = () => json(200, { items: [] });
  routes["GET /v1/orgs/o1/secrets"] = () => json(200, { items: [] });
  routes["GET /v1/agents/a1/conversations"] = () => json(200, { items: [], next_cursor: null });
}

describe("Welcome (C-927)", () => {
  it("a shared workspace: 'Welcome to <workspace>', and the three cards with real names from the five sentences", async () => {
    world({ accounts: [account()] });
    wrap(<WelcomeView />);
    expect(await screen.findByRole("heading", { name: "Welcome to Ops" })).toBeInTheDocument();
    const where = screen.getByRole("region", { name: "Where you are" });
    expect(where.textContent).toContain("This is Ops, in Acme. Daneel works here, on its own computer.");
    const safe = screen.getByRole("region", { name: "How it stays safe" });
    expect(safe.textContent).toContain("Everything Daneel sends goes through Aurora's gateway. It holds the keys, applies the access you choose, and puts anything that needs your say in Approvals.");
    expect(within(safe).getByRole("link", { name: "Approvals" })).toHaveAttribute("href", "/o/acme/w/ops/approvals");
    const agent = screen.getByRole("region", { name: "Daneel is your agent" });
    await waitFor(() => expect(agent.textContent).toContain("Daneel can read Linear. It asks you before changing anything in Linear. It can reach any website."));
  });

  it("promises nothing the summary does not show: a fresh agent asks nothing, and the page says it can't use any app", async () => {
    world();
    wrap(<WelcomeView />);
    const agent = await screen.findByRole("region", { name: "Daneel is your agent" });
    await waitFor(() => expect(agent.textContent).toContain("Daneel can't use any app yet. It can reach any website."));
    expect(document.body.textContent).not.toMatch(/asks you before risky actions/);
  });

  it("a person's own personal workspace: 'Welcome, <name>'", async () => {
    world({ personal: true, name: "Bob" });
    wrap(<WelcomeView />);
    expect(await screen.findByRole("heading", { name: "Welcome, Bob" })).toBeInTheDocument();
  });

  it("a member reads their role in one sentence and the same summary, websites included, without the owner's policy", async () => {
    world({ role: "member", accounts: [account({ levels: ["read"], agents: [{ agent: { id: "a1", name: "Daneel", handle: "daneel" }, workspace: { id: "w1", name: "Ops" }, level: "use" }] })] });
    let policyRead = false;
    routes["GET /v1/workspaces/w1/policy"] = () => {
      policyRead = true;
      return json(403, { error: { code: "forbidden", message: "no" } });
    };
    wrap(<WelcomeView />);
    expect(await screen.findByText(/You are a member here: you can chat with Daneel/)).toBeInTheDocument();
    const agent = screen.getByRole("region", { name: "Daneel is your agent" });
    await waitFor(() => expect(agent.textContent).toContain("Daneel can read Linear. It can reach any website."));
    expect(policyRead).toBe(false);
  });

  it("when the Internet access cannot be read the summary says nothing about websites rather than guess", async () => {
    world({ role: "member", accounts: [account({ levels: ["read"], agents: [{ agent: { id: "a1", name: "Daneel", handle: "daneel" }, workspace: { id: "w1", name: "Ops" }, level: "use" }] })] });
    routes["GET /v1/workspaces/w1/internet-access"] = () => json(403, { error: { code: "forbidden", message: "no" } });
    wrap(<WelcomeView />);
    const agent = await screen.findByRole("region", { name: "Daneel is your agent" });
    await waitFor(() => expect(agent.textContent).toContain("Daneel can read Linear."));
    expect(agent.textContent).not.toContain("website");
  });

  it("the role sentence is for people who are not owners", () => {
    expect(roleSentence("owner", "Daneel")).toBeNull();
    expect(roleSentence("approver", "Daneel")).toMatch(/approver/);
    expect(roleSentence("member", "Daneel")).toMatch(/member/);
  });
});

describe("the same summary on the agent's Access tab", () => {
  it("says it above the accounts list, with a rule that asks", async () => {
    world({
      accounts: [account()],
      rules: [{ id: "r1", enabled: true, action: "allow", require_approval: true, targets: [{ type: "network", host: "x.example" }] }],
    });
    wrap(<AccessTab agentId="a1" agentName="Daneel" wsId="w1" orgId="o1" canOwn base="/o/acme/w/ops" />);
    const box = await screen.findByRole("region", { name: "What Daneel can do" });
    await waitFor(() =>
      expect(box.textContent).toContain(
        "Daneel can read Linear. It asks you before changing anything in Linear. It can reach any website. A rule asks you first on 1 website.",
      ),
    );
  });
});
