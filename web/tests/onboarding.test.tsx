import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { wrap as wrapWith } from "./support/render";

// These tests watch the onboarding GET itself (Dismiss waits for it), so the switch is not seeded into the cache.
const wrap = (ui: React.ReactNode) => wrapWith(ui, { moreControls: undefined });
import { json } from "./support/api";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops" }),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
  usePathname: () => "/o/acme/w/ops",
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

import { gapFromSetup } from "@/components/admin/sign-in-gaps";
import { GettingStarted } from "@/components/onboarding/getting-started";
import { Tour } from "@/components/onboarding/tour";
import { startTour } from "@/components/onboarding/tour-events";
import { EmptyState } from "@/components/states";

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
  vi.restoreAllMocks();
});

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
  /** `contacts` of the workspace (C-981). */
  contacts?: unknown;
}

function world(w: World = {}) {
  routes["GET /v1/workspaces/w1/agents"] = () => json(200, { items: [AGENT], next_cursor: null });
  routes["GET /v1/workspaces/w1"] = () =>
    json(200, { id: "w1", org_id: "o1", slug: "ops", name: "Ops", my_role: "member", ...(w.contacts ? { contacts: w.contacts } : {}) });
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
    expect(await screen.findByText("Your organization provides an AI key: AIGW")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Get Daneel talking" })).toBeInTheDocument();
    expect(screen.queryByLabelText("New LLM key")).not.toBeInTheDocument();
    expect(screen.queryByText("Choose a model")).not.toBeInTheDocument();
    const hello = await screen.findByRole("link", { name: "Say hello to Daneel" });
    expect(hello).toHaveAttribute("href", expect.stringContaining("/o/acme/w/ops/agents/daneel?tab=chat&draft="));
    // Two steps (C-927): the key is done, one left.
    expect(screen.getByText("1 of 2 done")).toBeInTheDocument();
    expect(screen.queryByText(/Give Daneel access to/)).not.toBeInTheDocument();
  });

  it("key missing: explains what an AI key is and opens the form in place, with the agent ticked", async () => {
    world({ readiness: readiness([]) });
    guide();
    expect(await screen.findByText(/An AI key is the password to the AI service/)).toBeInTheDocument();
    expect(await screen.findByLabelText("New LLM key")).toBeInTheDocument();
    expect(screen.getByText("0 of 2 done")).toBeInTheDocument();
    expect(screen.getByText("Comes after the steps above.")).toBeInTheDocument();
  });

  it("not granted: the access step names the key and preselects the agent", async () => {
    world({ readiness: readiness([]), wsKeys: [llmSecret("k2", "my-key", "workspace")] });
    routes["GET /v1/secrets/k2/grants"] = () => json(200, { items: [] });
    guide();
    // The step is titled with the key it gives (C-927) and appears only for a key the agent cannot use.
    expect(await screen.findByText("Give Daneel access to my-key")).toBeInTheDocument();
    expect(screen.queryByText("AI key")).not.toBeInTheDocument();
    const box = await screen.findByRole("checkbox", { name: /Daneel/ });
    await waitFor(() => expect(box).toBeChecked());
    expect(screen.getByRole("button", { name: "Give Daneel access" })).toBeEnabled();
  });

  it("no default model: the AI key step holds the model picker, and saving the choice patches the agent", async () => {
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
    expect(await screen.findByText("2 of 2 done")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Say hello to Daneel" })).not.toBeInTheDocument();
  });

  it("an error event is not an answer", async () => {
    world({ readiness: readiness([keyRow()]), orgKeys: [llmSecret("k1", "AIGW", "organization")], replied: true });
    routes["GET /v1/conversations/c1/events"] = () =>
      json(200, { items: [{ seq: 1, type: "turn.queued", ts: "2026-11-10T09:00:00Z", payload: {} }, { seq: 2, type: "error", ts: "2026-11-10T09:00:05Z", payload: { code: "no_model_key" } }], last_seq: 2 });
    guide();
    expect(await screen.findByText("1 of 2 done")).toBeInTheDocument();
  });

  it("member: sees who can do the key step, never a form, and does not read readiness", async () => {
    // The owners come from the workspace's `contacts` (C-981): a member can read them, no access list is needed.
    world({ contacts: { owners: [{ name: "Ada Lovelace" }] } });
    guide({ isOwner: false });
    expect(await screen.findByText("Only workspace owners can add an account. Ask a workspace owner.")).toBeInTheDocument();
    expect(calls.some((c) => c.key === "GET /v1/workspaces/w1/access")).toBe(false);
    expect(screen.queryByLabelText("New LLM key")).not.toBeInTheDocument();
    expect(calls.some((c) => c.key === "GET /v1/agents/a1/model-readiness")).toBe(false);
    expect(screen.getAllByText("Needs an owner")).toHaveLength(1);
  });

  it("member of a workspace whose contacts are not known still gets a sentence, not a dead button", async () => {
    world({});
    guide({ isOwner: false });
    expect(await screen.findByText("Only workspace owners can add an account.")).toBeInTheDocument();
  });

  it("Overview: one line once complete, and expandable", async () => {
    world({ readiness: readiness([keyRow()]), orgKeys: [llmSecret("k1", "AIGW", "organization")], replied: true });
    guide({ variant: "overview" });
    expect(await screen.findByText(/Getting started — 2\/2/)).toBeInTheDocument();
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
    expect(await screen.findByText(/Getting started — 0\/2/)).toBeInTheDocument();
    expect(calls.find((c) => c.key === "PATCH /v1/me/onboarding")?.body).toEqual({ guide_dismissed: ["w1"] });
    expect(screen.getByRole("button", { name: "Show" })).toBeInTheDocument();
  });

  it("Dismiss keeps the other dismissals (the PATCH replaces the list) and waits for the GET (FIX-067)", async () => {
    world({ readiness: readiness([]) });
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    routes["GET /v1/me/onboarding"] = async () => {
      await gate;
      return json(200, { welcome_seen: true, guide_dismissed: ["w9", "w8"], tour_completed_at: null });
    };
    routes["PATCH /v1/me/onboarding"] = (_r, body) =>
      json(200, { welcome_seen: true, guide_dismissed: (body as { guide_dismissed: string[] }).guide_dismissed, tour_completed_at: null });
    guide({ variant: "overview" });
    const button = await screen.findByRole("button", { name: "Dismiss" });
    // The list is unknown while the GET is pending: Dismiss cannot send a one-entry replacement.
    expect(button).toBeDisabled();
    release();
    await waitFor(() => expect(screen.getByRole("button", { name: "Dismiss" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(calls.some((c) => c.key === "PATCH /v1/me/onboarding")).toBe(true));
    expect(calls.find((c) => c.key === "PATCH /v1/me/onboarding")?.body).toEqual({ guide_dismissed: ["w9", "w8", "w1"] });
  });

  it("access step: Not now puts the panel away and Give access brings it back (FIX-067)", async () => {
    world({ readiness: readiness([keyRow()]), orgKeys: [llmSecret("k1", "AIGW", "organization")] });
    routes["GET /v1/workspaces/w1/secrets"] = () => json(200, { items: [llmSecret("k2", "Mine", "workspace")] });
    routes["GET /v1/agents/a1/model-readiness"] = () => json(200, readiness([]));
    guide();
    const notNow = await screen.findByRole("button", { name: "Not now" });
    fireEvent.click(notNow);
    expect(screen.queryByRole("button", { name: "Not now" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Give access" }));
    expect(await screen.findByRole("button", { name: "Not now" })).toBeInTheDocument();
  });

  it("never shows a secret value", async () => {
    world({ readiness: readiness([keyRow()]), orgKeys: [llmSecret("k1", "AIGW", "organization")] });
    const { container } = guide();
    await screen.findByText("Your organization provides an AI key: AIGW");
    expect(container.textContent).not.toMatch(/sk-/);
    expect(container.querySelector("input[type=password]")).toBeNull();
  });
});

describe("sign-in gaps of one organization (C-924)", () => {
  const method = (over: Record<string, unknown>) => ({ id: "m", label: "M", kind: "oauth2", audience: "person", recommended: false, ready: false, needs: [], scope_presets: { read: [], write: [] }, ...over });
  const status = (methods: unknown[]) => ({ app_id: "lin", name: "Linear", methods }) as never;

  it("an app whose only person method needs a client is a gap; a working token makes it informational; an admin-only method does not count", () => {
    const onlyOauth = status([method({ id: "oauth", needs: ["oauth_client"], client_key: "linear" })]);
    expect(gapFromSetup(onlyOauth, "pkg")).toMatchObject({ appId: "lin", name: "Linear", pkg: "pkg", clientKey: "linear", tokenOnly: false });
    const withToken = status([method({ id: "oauth", needs: ["oauth_client"], client_key: "linear" }), method({ id: "token", kind: "fields", ready: true })]);
    expect(gapFromSetup(withToken, "pkg")).toMatchObject({ tokenOnly: true });
    expect(gapFromSetup(status([method({ id: "app", audience: "admin", needs: ["oauth_client"], client_key: "x" })]))).toBeNull();
    // Blocked by a server that is not registered: not a client problem.
    expect(gapFromSetup(status([method({ needs: ["oauth_client"], client_key: "k" }), method({ id: "s", kind: "fields", needs: ["instance"] })]))).toBeNull();
  });
});

// ---------------------------------------------------------------------------------------------------------------------
// The tour
// ---------------------------------------------------------------------------------------------------------------------
const NAV = ["overview", "agents", "apps", "internet-access", "approvals", "activity", "people", "settings"];

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
    const dialog = await screen.findByRole("dialog", { name: "Overview" });
    await waitFor(() => expect(dialog).toHaveFocus());
    expect(within(dialog).getByText("Stop 1 of 8")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "overview" })).toHaveAttribute("data-tour-active", "true");
    expect(setNavOpen).not.toHaveBeenCalledWith(true);

    fireEvent.keyDown(window, { key: "ArrowRight" });
    expect(await screen.findByRole("dialog", { name: "Agents" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "agents" })).toHaveAttribute("data-tour-active", "true");
    expect(screen.getByRole("link", { name: "overview" })).not.toHaveAttribute("data-tour-active");
    fireEvent.keyDown(window, { key: "ArrowLeft" });
    expect(await screen.findByRole("dialog", { name: "Overview" })).toBeInTheDocument();
    // Back is disabled on the first stop; Next is a real button.
    expect(screen.getByRole("button", { name: "Back" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByRole("dialog", { name: "Agents" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    // Esc only leaves (FIX-067): the tour stays replayable and is not recorded as completed.
    expect(calls.some((c) => c.key === "PATCH /v1/me/onboarding")).toBe(false);
    expect(opener).toHaveFocus();
    expect(document.querySelector("[data-tour-active]")).toBeNull();
  });

  it("Skip tour records the tour as completed", async () => {
    stubLayout(false);
    routes["PATCH /v1/me/onboarding"] = () => json(200, { welcome_seen: true, guide_dismissed: [], tour_completed_at: "2026-11-10T10:00:00Z" });
    wrap(
      <>
        {nav()}
        <Tour setNavOpen={vi.fn()} />
      </>,
    );
    act(() => startTour());
    await screen.findByRole("dialog", { name: "Overview" });
    fireEvent.click(screen.getByRole("button", { name: "Skip tour" }));
    await waitFor(() => expect(calls.find((c) => c.key === "PATCH /v1/me/onboarding")?.body).toEqual({ tour_completed: true }));
  });

  it("leaves the arrow keys alone in a field and inside another dialog (FIX-067)", async () => {
    stubLayout(false);
    wrap(
      <>
        {nav()}
        <input aria-label="Name" />
        <div role="dialog" aria-label="Other">
          <button type="button">Inside</button>
        </div>
        <Tour setNavOpen={vi.fn()} />
      </>,
    );
    act(() => startTour());
    await screen.findByRole("dialog", { name: "Overview" });
    // fireEvent returns false when a handler called preventDefault.
    expect(fireEvent.keyDown(screen.getByLabelText("Name"), { key: "ArrowRight" })).toBe(true);
    expect(fireEvent.keyDown(screen.getByRole("button", { name: "Inside" }), { key: "ArrowRight" })).toBe(true);
    expect(screen.getByRole("dialog", { name: "Overview" })).toBeInTheDocument();
    // Escape in the other dialog does not close the tour either.
    fireEvent.keyDown(screen.getByRole("button", { name: "Inside" }), { key: "Escape" });
    expect(screen.getByRole("dialog", { name: "Overview" })).toBeInTheDocument();
    // Elsewhere the arrow still moves it.
    expect(fireEvent.keyDown(window, { key: "ArrowRight" })).toBe(false);
    expect(await screen.findByRole("dialog", { name: "Agents" })).toBeInTheDocument();
  });

  it("skips stops whose link is not there (a member has no Approvals, People or Settings) and ends on Done", async () => {
    stubLayout(false);
    routes["PATCH /v1/me/onboarding"] = () => json(200, { welcome_seen: true, guide_dismissed: [], tour_completed_at: "2026-11-10T10:00:00Z" });
    wrap(
      <>
        <nav>
          {NAV.filter((t) => t !== "approvals" && t !== "people" && t !== "settings").map((t) => (
            <a key={t} href={`/${t}`} data-tour={t}>
              {t}
            </a>
          ))}
        </nav>
        <Tour setNavOpen={vi.fn()} />
      </>,
    );
    act(() => startTour());
    expect(await screen.findByText("Stop 1 of 5", undefined)).toBeInTheDocument();
    // The Agents stop says an agent gets apps on its Access tab (C-975).
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText(/Give <Agent> an app/)).toBeInTheDocument();
    for (let i = 0; i < 3; i++) fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("Stop 5 of 5")).toBeInTheDocument();
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
    const dialog = await screen.findByRole("dialog", undefined);
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
    expect(await screen.findByRole("dialog", undefined)).toBeInTheDocument();
    expect(window.location.search).toBe("");
  });
});

// ---------------------------------------------------------------------------------------------------------------------
// Teaching empty states
// ---------------------------------------------------------------------------------------------------------------------
describe("teaching empty states (D-141)", () => {
  const owners = { action: "add_schedule", role: "workspace_owner" as const, people: [{ name: "Bob Builder" }], you_can: false };

  it("say what it is, when you need it, and the primary action", () => {
    render(
      <EmptyState title="No schedules" when="Use one for recurring work." action={<button type="button">New schedule</button>} ask={{ whoCan: owners, canAct: true }}>
        A schedule wakes the agent with a prompt at set times.
      </EmptyState>,
    );
    expect(screen.getByText("A schedule wakes the agent with a prompt at set times.")).toBeInTheDocument();
    expect(screen.getByText("Use one for recurring work.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New schedule" })).toBeInTheDocument();
    // (C-916) A viewer who can act is never told to ask someone else.
    expect(screen.queryByText(/Ask a workspace owner/)).not.toBeInTheDocument();
  });

  it("a person who cannot act sees who can instead of a dead button", () => {
    render(
      <EmptyState title="No schedules" when="Use one for recurring work." ask={{ whoCan: owners, canAct: false }}>
        A schedule wakes the agent with a prompt at set times.
      </EmptyState>,
    );
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(screen.getByText("Only workspace owners can set up a schedule. Ask a workspace owner.")).toBeInTheDocument();
  });
});
