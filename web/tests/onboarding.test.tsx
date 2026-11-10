import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops" }),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
  usePathname: () => "/o/acme/w/ops",
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

import { SetupView } from "@/components/admin/setup-view";
import { GettingStarted } from "@/components/onboarding/getting-started";
import { Tour } from "@/components/onboarding/tour";
import { startTour } from "@/components/onboarding/tour-events";
import { EmptyState } from "@/components/states";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request, body: unknown) => Response | Promise<Response>;
let calls: { key: string; body: unknown }[];
let routes: Record<string, Handler>;

beforeEach(() => {
  calls = [];
  routes = {};
  window.history.replaceState(null, "", "/");
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
  vi.restoreAllMocks();
});

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

// ---------------------------------------------------------------------------------------------------------------------
// The getting-started guide
// ---------------------------------------------------------------------------------------------------------------------
const AGENT = { id: "a1", handle: "daneel", name: "Daneel", kind: "hosted", is_default: true, frozen: null };
const keyRow = (over: Record<string, unknown> = {}) => ({
  secret_id: "k1", name: "AIGW", scope: "organization", provider: "litellm", endpoint_id: "litellm", usable: true, reason: null, ...over,
});
const readiness = (keys: unknown[], withDefault = true) => ({
  harness: "claude-code",
  keys,
  override_dropped: false,
  default: withDefault ? { endpoint_id: "litellm", model_id: "claude-sonnet", provider: "litellm", source: "endpoint_default" } : null,
});
const llmSecret = (id: string, name: string, scope: string) => ({
  id, scope, kind: "llm", name, status: "active", host_pattern: null, path_pattern: null, injection: { kind: "bearer" }, value_source: "value", value_version: 1, llm: null, origin: null,
});

interface World {
  isOwner?: boolean;
  readiness?: unknown;
  wsKeys?: unknown[];
  orgKeys?: unknown[];
  replied?: boolean;
  dismissed?: boolean;
  access?: unknown;
}

function world(w: World = {}) {
  routes["GET /v1/workspaces/w1/agents"] = () => json(200, { items: [AGENT], next_cursor: null });
  routes["GET /v1/me/onboarding"] = () => json(200, { welcome_seen: true, guide_dismissed: w.dismissed ? ["w1"] : [], tour_completed_at: null });
  routes["GET /v1/agents/a1/model-readiness"] = () => json(200, w.readiness ?? readiness([]));
  routes["GET /v1/workspaces/w1/secrets"] = () => json(200, { items: w.wsKeys ?? [] });
  routes["GET /v1/orgs/o1/secrets"] = () => json(200, { items: w.orgKeys ?? [] });
  routes["GET /v1/agents/a1/conversations"] = () =>
    json(200, { items: w.replied ? [{ id: "c1", agent_id: "a1", kind: "direct", last_seq: 3, created_at: "2026-11-10T09:00:00Z", last_activity_at: "2026-11-10T09:01:00Z" }] : [], next_cursor: null });
  routes["GET /v1/conversations/c1/events"] = () =>
    json(200, { items: [{ seq: 1, type: "turn.queued", ts: "2026-11-10T09:00:00Z", payload: {} }, { seq: 2, type: "text", ts: "2026-11-10T09:00:05Z", payload: {} }], last_seq: 3 });
  routes["GET /v1/workspaces/w1/access"] = () =>
    w.access === undefined
      ? json(403, { error: { code: "forbidden", message: "no" } })
      : json(200, { items: w.access, next_cursor: null });
  routes["GET /v1/llm/providers"] = () =>
    json(200, { providers: [{ id: "litellm", display_name: "LiteLLM", auth_modes: ["bearer"] }], endpoints: [{ id: "litellm", provider: "litellm", display_name: "AIGW", permitted: true, key_scopes: [] }] });
}

const guide = (props: Partial<React.ComponentProps<typeof GettingStarted>> = {}) =>
  wrap(<GettingStarted org="acme" ws="ops" wsId="w1" orgId="o1" isOwner variant="welcome" {...props} />);

describe("getting-started guide: every state", () => {
  it("key provided by the org: collapsed with what provides it, no form, hello is next", async () => {
    world({ readiness: readiness([keyRow()]), orgKeys: [llmSecret("k1", "AIGW", "organization")] });
    guide();
    expect(await screen.findByText("Your organization provides an LLM key: AIGW")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Get Daneel talking" })).toBeInTheDocument();
    expect(screen.queryByLabelText("New LLM key")).not.toBeInTheDocument();
    expect(screen.queryByText("Choose a model")).not.toBeInTheDocument();
    const hello = await screen.findByRole("link", { name: "Say hello to Daneel" });
    expect(hello).toHaveAttribute("href", expect.stringContaining("/o/acme/w/ops/agents/daneel?tab=chat&draft="));
    expect(screen.getByText("2 of 3 done")).toBeInTheDocument();
  });

  it("key missing: explains what a key is and opens the form in place", async () => {
    world({ readiness: readiness([]) });
    guide();
    expect(await screen.findByText(/An LLM key is the password to the AI service/)).toBeInTheDocument();
    expect(await screen.findByLabelText("New LLM key")).toBeInTheDocument();
    expect(screen.getByText("0 of 3 done")).toBeInTheDocument();
    expect(screen.getByText("Comes after the key.")).toBeInTheDocument();
  });

  it("not granted: the access step names the key and preselects the agent", async () => {
    world({ readiness: readiness([]), wsKeys: [llmSecret("k2", "my-key", "workspace")] });
    routes["GET /v1/secrets/k2/grants"] = () => json(200, { items: [] });
    guide();
    expect(await screen.findByText("my-key")).toBeInTheDocument();
    const box = await screen.findByRole("checkbox", { name: /Daneel/ });
    await waitFor(() => expect(box).toBeChecked());
    expect(screen.getByRole("button", { name: "Give access" })).toBeEnabled();
  });

  it("no model: shows the model step, and saving the choice patches the agent", async () => {
    world({ readiness: readiness([keyRow()], false), orgKeys: [llmSecret("k1", "AIGW", "organization")] });
    routes["GET /v1/agents/a1/models"] = () =>
      json(200, { items: [{ endpoint_id: "litellm", provider: "litellm", model_id: "claude-sonnet", display_name: "Claude Sonnet", dialects: [] }] });
    routes["PATCH /v1/agents/a1"] = () => json(200, { ...AGENT });
    guide();
    const select = await screen.findByLabelText("Model");
    await waitFor(() => expect(within(select).getByRole("option", { name: /Claude Sonnet/ })).toBeInTheDocument());
    fireEvent.change(select, { target: { value: "litellm\u0000claude-sonnet" } });
    fireEvent.click(screen.getByRole("button", { name: "Use this model" }));
    await waitFor(() => expect(calls.find((c) => c.key === "PATCH /v1/agents/a1")?.body).toEqual({ model_endpoint_id: "litellm", model_id: "claude-sonnet" }));
    expect(screen.getByText("Comes after the steps above.")).toBeInTheDocument();
  });

  it("done: an answer from the agent completes it, whatever else is missing", async () => {
    world({ readiness: readiness([keyRow()]), orgKeys: [llmSecret("k1", "AIGW", "organization")], replied: true });
    guide();
    expect(await screen.findByText("3 of 3 done")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Say hello to Daneel" })).not.toBeInTheDocument();
  });

  it("an error event is not an answer", async () => {
    world({ readiness: readiness([keyRow()]), orgKeys: [llmSecret("k1", "AIGW", "organization")], replied: true });
    routes["GET /v1/conversations/c1/events"] = () =>
      json(200, { items: [{ seq: 1, type: "turn.queued", ts: "2026-11-10T09:00:00Z", payload: {} }, { seq: 2, type: "error", ts: "2026-11-10T09:00:05Z", payload: { code: "no_model_key" } }], last_seq: 2 });
    guide();
    expect(await screen.findByText("2 of 3 done")).toBeInTheDocument();
  });

  it("member: sees who can do the key and access steps, never a form, and does not read readiness", async () => {
    world({ access: [{ user_id: "u1", display_name: "Ada Lovelace", email: "ada@example.org", role: "owner", implicit: false }, { user_id: "u2", display_name: "Bob", email: "b@example.org", role: "member", implicit: false }] });
    guide({ isOwner: false });
    expect(await screen.findByText("Ask a workspace owner (Ada Lovelace) to add an LLM key.")).toBeInTheDocument();
    expect(screen.getByText("Ask a workspace owner (Ada Lovelace) to give Daneel access to it.")).toBeInTheDocument();
    expect(screen.queryByLabelText("New LLM key")).not.toBeInTheDocument();
    expect(calls.some((c) => c.key === "GET /v1/agents/a1/model-readiness")).toBe(false);
    expect(screen.getAllByText("Needs an owner")).toHaveLength(2);
  });

  it("member who cannot read the owner list still gets a sentence, not a dead button", async () => {
    world({});
    guide({ isOwner: false });
    expect(await screen.findByText("Ask a workspace owner to add an LLM key.")).toBeInTheDocument();
  });

  it("Overview: one line once complete, and expandable", async () => {
    world({ readiness: readiness([keyRow()]), orgKeys: [llmSecret("k1", "AIGW", "organization")], replied: true });
    guide({ variant: "overview" });
    expect(await screen.findByText(/Getting started — 3\/3/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show again" }));
    expect(await screen.findByRole("heading", { name: "Get Daneel talking" })).toBeInTheDocument();
  });

  it("Overview: the full guide until dismissed; dismissing stores it server-side and leaves one line", async () => {
    world({ readiness: readiness([]) });
    let dismissed = false;
    routes["GET /v1/me/onboarding"] = () => json(200, { welcome_seen: true, guide_dismissed: dismissed ? ["w1"] : [], tour_completed_at: null });
    routes["PATCH /v1/me/onboarding"] = (_r, body) => {
      dismissed = true;
      return json(200, { welcome_seen: true, guide_dismissed: (body as { guide_dismissed: string[] }).guide_dismissed, tour_completed_at: null });
    };
    guide({ variant: "overview" });
    fireEvent.click(await screen.findByRole("button", { name: "Dismiss" }));
    expect(await screen.findByText(/Getting started — 0\/3/)).toBeInTheDocument();
    expect(calls.find((c) => c.key === "PATCH /v1/me/onboarding")?.body).toEqual({ guide_dismissed: ["w1"] });
    expect(screen.getByRole("button", { name: "Show" })).toBeInTheDocument();
  });

  it("never shows a secret value", async () => {
    world({ readiness: readiness([keyRow()]), orgKeys: [llmSecret("k1", "AIGW", "organization")] });
    const { container } = guide();
    await screen.findByText("Your organization provides an LLM key: AIGW");
    expect(container.textContent).not.toMatch(/sk-/);
    expect(container.querySelector("input[type=password]")).toBeNull();
  });
});

// ---------------------------------------------------------------------------------------------------------------------
// The instance setup wizard
// ---------------------------------------------------------------------------------------------------------------------
const placement = (over: Record<string, unknown> = {}) => ({
  saved: true, signup_mode: "open", mode: "shared_org", default_org: { id: "o1", slug: "users", name: "Users" }, default_org_role: "member",
  shared_workspaces: [], personal_workspace: { enabled: true, role: "owner", name_template: "{preferred_username}|{email_local}" },
  starter_grants: [], personal_orgs: { count: 0, items: [] }, warnings: [], updated_at: "2026-11-10T08:00:00Z", updated_by: "u9", ...over,
});
const check = (id: string, status: string, reason: string, message: string, extra: Record<string, unknown> = {}) => ({ id, status, reason, message, value: null, helm: null, ...extra });
const setupStatus = (over: Record<string, unknown> = {}) => ({
  placement_saved: true,
  you: { provider: "cern", provider_name: "CERN SSO", role_claim: "cern_roles", values: ["aurora-admins", "aurora-users", "stray"], granted: [
    { value: "aurora-admins", instance_admin: true, org: null, workspaces: [] },
    { value: "aurora-users", instance_admin: false, org: { slug: "users", role: "member" }, workspaces: [] },
  ] },
  steps: [
    { id: "roles", status: "ok", reason: "roles_ok", checks: [check("role_mappings", "ok", "role_mappings_set", "Roles in the sign-in token decide who is an admin.", { helm: "auth.oidc_providers[].role_mappings" })] },
    { id: "placement", status: "ok", reason: "placement_saved", checks: [] },
    { id: "llm", status: "warning", reason: "no_default_model", checks: [
      check("endpoint:litellm", "ok", "endpoint_reachable", "AIGW answers.", { value: "https://aigw.example.org" }),
      check("default_model:litellm", "warning", "no_default_model", "AIGW has no default model.", { helm: "llm.endpoints[].default_model" }),
      check("org_key", "ok", "org_key_present", "Users has an LLM key.", { value: "1" }),
      check("starter_grant", "todo", "no_starter_grant", "New users' agents get no key yet."),
    ] },
    { id: "runners", status: "todo", reason: "no_runner", checks: [check("runners", "todo", "no_runner", "No runner is registered, so no agent can start.")] },
    { id: "optional", status: "ok", reason: "optional", checks: [check("connectors", "todo", "no_connectors", "Connectors bring chat tools such as Mattermost to your agents.", { value: "0" })] },
  ],
  ...over,
});

function setupWorld(status = setupStatus(), place = placement()) {
  routes["GET /v1/admin/setup"] = () => json(200, status);
  routes["GET /v1/admin/placement"] = () => json(200, place);
  routes["GET /v1/admin/placement/preview"] = () => json(200, { name: "bobj", slug: "bobj", used: 0, unresolved: [] });
  routes["GET /v1/admin/placement/choices"] = () => json(200, { workspaces: [], secrets: [], connections: [] });
  routes["GET /v1/admin/orgs"] = () => json(200, { items: [{ id: "o1", slug: "users", name: "Users", created_at: "2026-11-01T00:00:00Z" }], next_cursor: null });
  routes["GET /v1/orgs/o1/secrets"] = () => json(200, { items: [llmSecret("k9", "AIGW", "organization")] });
  routes["GET /v1/orgs"] = () => json(200, { items: [{ id: "o1", slug: "users", name: "Users" }] });
  routes["GET /v1/orgs/o1/workspaces"] = () => json(200, { items: [{ id: "w1", slug: "default", name: "Default" }] });
}

describe("instance setup wizard (D-141)", () => {
  it("shows the six steps with live status, the role mapping of your token and the Helm values", async () => {
    setupWorld();
    wrap(<SetupView />);
    const steps = await screen.findByRole("list", { name: "Setup steps" });
    expect(within(steps).getAllByRole("region")).toHaveLength(6);
    expect(screen.getByText("CERN SSO")).toBeInTheDocument();
    expect(screen.getByText("cern_roles")).toBeInTheDocument();
    expect(screen.getByText("→ instance admin")).toBeInTheDocument();
    expect(screen.getByText("→ member of users")).toBeInTheDocument();
    expect(screen.getByText("→ not mapped to anything")).toBeInTheDocument();
    // The FIX-019 warning in words, with the Helm value to change and the fix.
    expect(screen.getByText("AIGW has no default model.")).toBeInTheDocument();
    expect(screen.getByText("llm.endpoints[].default_model")).toBeInTheDocument();
    expect(screen.getByText("Set a default model so agents need not choose one by one.")).toBeInTheDocument();
    // Runners: the failure in words with the fix.
    expect(screen.getByText("No runner is registered, so no agent can start.")).toBeInTheDocument();
    expect(screen.getByText(/docs\/operator\/runners\.md/)).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("link", { name: "Open your workspace" })).toHaveAttribute("href", "/o/users/w/default"));
  });

  it("an instance without role mappings says only the e-mail list makes admins, and without a sign-in claim says so", async () => {
    setupWorld(
      setupStatus({
        you: null,
        steps: setupStatus().steps.map((s) =>
          s.id === "roles"
            ? { ...s, status: "warning", reason: "no_role_mappings", checks: [check("role_mappings", "warning", "no_role_mappings", "No role mappings: only the admin e-mail list makes admins.", { helm: "auth.oidc_providers[].role_claim, role_mappings" })] }
            : s,
        ),
      }),
    );
    wrap(<SetupView />);
    expect(await screen.findByText("No role mappings: only the admin e-mail list makes admins.")).toBeInTheDocument();
    expect(screen.getByText(/did not read a role claim/)).toBeInTheDocument();
  });

  it("the starter-grant switch writes the org key into the saved placement", async () => {
    setupWorld();
    // The server keeps what was saved: the refetch after the PUT returns it.
    let current = placement();
    routes["GET /v1/admin/placement"] = () => json(200, current);
    routes["PUT /v1/admin/placement"] = (_r, body) => {
      current = placement({ starter_grants: (body as { starter_grants: { kind: string; id: string }[] }).starter_grants.map((g) => ({ ...g, name: "AIGW", detail: "llm" })) });
      return json(200, current);
    };
    wrap(<SetupView />);
    const sw = await screen.findByRole("checkbox", { name: "Give it to every new user's agent" });
    await waitFor(() => expect(sw).toBeEnabled(), { timeout: 3000 });
    fireEvent.click(sw);
    // (3s windows: the PUT -> refetch -> re-render chain has to fit, which can be slow on a loaded host.)
    await waitFor(() => expect(calls.find((c) => c.key === "PUT /v1/admin/placement")?.body).toMatchObject({
      mode: "shared_org", default_org_id: "o1", starter_grants: [{ kind: "secret", id: "k9" }],
    }), { timeout: 3000 });
    await waitFor(() => expect(sw).toBeChecked(), { timeout: 3000 });
  });

  it("without a saved shared placement the switch is off and says why", async () => {
    setupWorld(setupStatus({ placement_saved: false }), placement({ saved: false, mode: "personal_org", default_org: null, warnings: ["placement_unsaved"] }));
    wrap(<SetupView />);
    expect(await screen.findByText(/This page opens at sign-in until you save where new users land/)).toBeInTheDocument();
    expect(screen.getByText(/Choose the default organization in the step above/)).toBeInTheDocument();
  });

  it("an org without a key gets the add-a-key form for the default org", async () => {
    setupWorld();
    routes["GET /v1/orgs/o1/secrets"] = () => json(200, { items: [] });
    routes["GET /v1/llm/providers"] = () =>
      json(200, { providers: [{ id: "litellm", display_name: "LiteLLM", auth_modes: ["bearer"] }], endpoints: [{ id: "litellm", provider: "litellm", display_name: "AIGW", permitted: true, key_scopes: [] }] });
    wrap(<SetupView />);
    expect(await screen.findByText("Add an organization LLM key for Users")).toBeInTheDocument();
    expect(await screen.findByLabelText("New LLM key")).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------------------------------------------------
// The tour
// ---------------------------------------------------------------------------------------------------------------------
const NAV = ["overview", "agents", "credentials", "policy", "approvals", "activity", "members", "settings"];

function nav() {
  return (
    <nav aria-label="Workspace">
      {NAV.map((t) => (
        <a key={t} href={`/${t}`} data-tour={t}>
          {t}
        </a>
      ))}
    </nav>
  );
}

function stubLayout(narrow: boolean) {
  // jsdom has no layout: every element counts as on screen.
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue([{}] as unknown as DOMRectList);
  Element.prototype.scrollIntoView = vi.fn();
  vi.stubGlobal("matchMedia", (q: string) => ({ matches: narrow, media: q, addEventListener: vi.fn(), removeEventListener: vi.fn(), addListener: vi.fn(), removeListener: vi.fn(), onchange: null, dispatchEvent: vi.fn() }));
}

describe("tour (D-141)", () => {
  it("is keyboard operable: arrows move, Esc leaves and records it, focus returns", async () => {
    stubLayout(false);
    routes["PATCH /v1/me/onboarding"] = () => json(200, { welcome_seen: true, guide_dismissed: [], tour_completed_at: "2026-11-10T10:00:00Z" });
    const setNavOpen = vi.fn();
    wrap(
      <>
        {nav()}
        <button type="button">Show me around</button>
        <Tour setNavOpen={setNavOpen} />
      </>,
    );
    const opener = screen.getByRole("button", { name: "Show me around" });
    opener.focus();
    act(() => startTour());
    const dialog = await screen.findByRole("dialog", { name: "Overview" }, { timeout: 3000 });
    await waitFor(() => expect(dialog).toHaveFocus());
    expect(within(dialog).getByText("Stop 1 of 8")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "overview" })).toHaveAttribute("data-tour-active", "true");
    expect(setNavOpen).not.toHaveBeenCalledWith(true);

    fireEvent.keyDown(window, { key: "ArrowRight" });
    expect(await screen.findByRole("dialog", { name: "Agents and Chat" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "agents" })).toHaveAttribute("data-tour-active", "true");
    expect(screen.getByRole("link", { name: "overview" })).not.toHaveAttribute("data-tour-active");
    fireEvent.keyDown(window, { key: "ArrowLeft" });
    expect(await screen.findByRole("dialog", { name: "Overview" })).toBeInTheDocument();
    // Back is disabled on the first stop; Next is a real button.
    expect(screen.getByRole("button", { name: "Back" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByRole("dialog", { name: "Agents and Chat" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.key === "PATCH /v1/me/onboarding")?.body).toEqual({ tour_completed: true });
    expect(opener).toHaveFocus();
    expect(document.querySelector("[data-tour-active]")).toBeNull();
  });

  it("skips stops whose link is not there (a member has no Approvals or Members) and ends on Done", async () => {
    stubLayout(false);
    routes["PATCH /v1/me/onboarding"] = () => json(200, { welcome_seen: true, guide_dismissed: [], tour_completed_at: "2026-11-10T10:00:00Z" });
    wrap(
      <>
        <nav>
          {NAV.filter((t) => t !== "approvals" && t !== "members").map((t) => (
            <a key={t} href={`/${t}`} data-tour={t}>
              {t}
            </a>
          ))}
        </nav>
        <Tour setNavOpen={vi.fn()} />
      </>,
    );
    act(() => startTour());
    expect(await screen.findByText("Stop 1 of 6", undefined, { timeout: 5000 })).toBeInTheDocument();
    for (let i = 0; i < 5; i++) fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("Stop 6 of 6")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.some((c) => c.key === "PATCH /v1/me/onboarding")).toBe(true);
  });

  it("at 390 px it opens the navigation drawer and sits at the bottom of the screen", async () => {
    stubLayout(true);
    routes["PATCH /v1/me/onboarding"] = () => json(200, { welcome_seen: true, guide_dismissed: [], tour_completed_at: "2026-11-10T10:00:00Z" });
    const setNavOpen = vi.fn();
    wrap(
      <>
        {nav()}
        <Tour setNavOpen={setNavOpen} />
      </>,
    );
    act(() => startTour());
    const dialog = await screen.findByRole("dialog", undefined, { timeout: 3000 });
    expect(setNavOpen).toHaveBeenCalledWith(true);
    expect(dialog.className).toContain("bottom-4");
    fireEvent.click(screen.getByRole("button", { name: "Skip tour" }));
    await waitFor(() => expect(setNavOpen).toHaveBeenLastCalledWith(false));
  });

  it("?tour=1 starts it and is removed from the address", async () => {
    stubLayout(false);
    window.history.replaceState(null, "", "/o/acme/w/ops?tour=1");
    wrap(
      <>
        {nav()}
        <Tour setNavOpen={vi.fn()} />
      </>,
    );
    expect(await screen.findByRole("dialog", undefined, { timeout: 3000 })).toBeInTheDocument();
    expect(window.location.search).toBe("");
  });
});

// ---------------------------------------------------------------------------------------------------------------------
// Teaching empty states
// ---------------------------------------------------------------------------------------------------------------------
describe("teaching empty states (D-141)", () => {
  it("say what it is, when you need it, and the primary action", () => {
    render(
      <EmptyState title="No schedules" when="Use one for recurring work." action={<button type="button">New schedule</button>} ask="Ask a workspace owner to set one up.">
        A schedule wakes the agent with a prompt at set times.
      </EmptyState>,
    );
    expect(screen.getByText("A schedule wakes the agent with a prompt at set times.")).toBeInTheDocument();
    expect(screen.getByText("Use one for recurring work.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New schedule" })).toBeInTheDocument();
    expect(screen.queryByText("Ask a workspace owner to set one up.")).not.toBeInTheDocument();
  });

  it("a person who cannot act sees who to ask instead of a dead button", () => {
    render(
      <EmptyState title="No schedules" when="Use one for recurring work." ask="Ask a workspace owner to set one up.">
        A schedule wakes the agent with a prompt at set times.
      </EmptyState>,
    );
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(screen.getByText("Ask a workspace owner to set one up.")).toBeInTheDocument();
  });
});
