import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(""),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useParams: () => ({}),
  usePathname: () => "/",
}));

import { AgentChannelsTab } from "@/components/channels/agent-channels-tab";

// FIX-127 Part C: the agent's Channels tab — next steps after attaching, "Add to a channel", empty states.

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const err = (status: number, code: string, message = code) => json(status, { error: { code, message } });
type Handler = (req: Request, body: unknown) => Response | Promise<Response>;
interface Rec { key: string; body: unknown; url: string }
function mockFetch(routes: Record<string, Handler>) {
  const calls: Rec[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input as Request;
      const url = new URL(req.url, "http://localhost");
      const key = `${req.method} ${url.pathname}`;
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      const body = text ? (JSON.parse(text) as unknown) : null;
      calls.push({ key, body, url: url.search });
      const h = routes[key];
      return h ? h(req, body) : err(404, "not_found");
    }),
  );
  return calls;
}
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const T = "2026-01-01T00:00:00Z";
const integ = {
  id: "i1", org_id: "o1", connector_id: "mattermost", tenant_ref: "chat.example.com/alpha", tenant_label: "@daneel on chat.example.com",
  config: { server_url: "https://chat.example.com", team: "alpha" }, shared_bot: { handle: "daneel", display_name: "Daneel" },
  status: "active", status_reason: null, created_at: T, updated_at: T,
};
const presence = {
  id: "p1", agent_id: "a1", integration_id: "i1", connector_id: "mattermost", tenant_label: "@daneel on chat.example.com",
  mode: "shared", handle: "@daneel", status: "active", created_at: T,
};
const option = {
  integration_id: "i1", connector_id: "mattermost", connector_name: "Mattermost", tenant_label: "@daneel on chat.example.com",
  modes: ["shared"], presence_setup: {}, attached: false,
};
const spaces = {
  items: [
    { space_ref: "C1", label: "town-square", kind: "channel", bot_present: true },
    { space_ref: "G1", label: "secret-plans", kind: "private", bot_present: true },
  ],
};
const binding = { id: "b1", integration_id: "i1", space_ref: "C1", space_label: "town-square", agent_id: "a1", listen: "mentions", followups_need_mention: false, active: true, bound_by: null, created_at: T };

function setup(extra: Record<string, Handler> = {}, presences: unknown[] = [presence], options: unknown[] = [{ ...option, attached: true }]) {
  return mockFetch({
    "GET /v1/connectors": () => json(200, { items: [] }),
    "GET /v1/orgs/o1": () => json(200, { id: "o1", slug: "acme", name: "Acme", external_id: null, app_posture: "open", settings: {}, my_role: "admin", created_at: T }),
    "GET /v1/orgs/o1/channel-integrations": () => json(200, { items: [integ], next_cursor: null }),
    "GET /v1/agents/a1/presences": () => json(200, { items: presences }),
    "GET /v1/agents/a1/presence-options": () => json(200, { items: options }),
    "GET /v1/agents/a1/space-bindings": () => json(200, { items: [] }),
    "GET /v1/agents/a1/reach": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/integrations/i1/spaces": () => json(200, spaces),
    ...extra,
  });
}

async function openPicker() {
  wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
  fireEvent.click(await screen.findByRole("button", { name: "Add to a channel" }));
  return screen.findByLabelText("Channel");
}

describe("next steps after attaching", () => {
  it("shows a DM link and the channel step for an active shared presence", async () => {
    setup();
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
    const panel = await screen.findByTestId("next-steps");
    const dm = await screen.findByRole("link", { name: "Open a DM with @daneel" });
    expect(dm).toHaveAttribute("href", "https://chat.example.com/alpha/messages/@daneel");
    expect(panel.textContent).toContain("Invite @daneel to the channel first, then bind it here.");
  });

  it("has no DM link, but still the step, when the integration list is refused", async () => {
    setup({ "GET /v1/orgs/o1/channel-integrations": () => err(403, "forbidden") });
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
    const panel = await screen.findByTestId("next-steps");
    expect(panel.querySelector("a")).toBeNull();
    expect(panel.textContent).toContain("Open a direct message with @daneel.");
    expect(screen.getByRole("button", { name: "Add to a channel" })).toBeInTheDocument();
  });

  it("is not shown without can_own, for a dedicated presence, or before the presence is active", async () => {
    const calls = setup();
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn={false} />);
    await screen.findByText("@daneel on chat.example.com", { selector: "span" });
    expect(screen.queryByTestId("next-steps")).toBeNull();
    expect(calls.some((c) => c.key.includes("/spaces"))).toBe(false);
    cleanup();

    setup({}, [{ ...presence, mode: "dedicated" }]);
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
    await screen.findByText("dedicated");
    expect(screen.queryByTestId("next-steps")).toBeNull();
    cleanup();

    setup({}, [{ ...presence, status: "pending_setup" }]);
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
    await screen.findByText("pending setup");
    expect(screen.queryByTestId("next-steps")).toBeNull();
  });
});

describe("Add to a channel", () => {
  it("lists my spaces and binds with the default listen mode", async () => {
    const calls = setup({ "POST /v1/agents/a1/space-bindings": () => json(201, binding) });
    const select = await openPicker();
    expect(calls.find((c) => c.key === "GET /v1/integrations/i1/spaces")?.url).toBe("?mine=true");
    expect([...select.querySelectorAll("option")].map((o) => o.textContent)).toEqual(["Select…", "# town-square", "Private · secret-plans"]);
    expect(screen.getByLabelText("Listen")).toHaveValue("mentions");
    const submit = screen.getByRole("button", { name: "Add to this channel" });
    expect(submit).toBeDisabled();
    fireEvent.change(select, { target: { value: "C1" } });
    fireEvent.click(submit);
    expect(await screen.findByText(/Bound\. @daneel announced itself in town-square\./)).toBeInTheDocument();
    expect(calls.find((c) => c.key === "POST /v1/agents/a1/space-bindings")?.body).toEqual({ integration_id: "i1", space_ref: "C1", listen: "mentions" });
    // The bindings list is refetched so the new binding appears.
    await waitFor(() => expect(calls.filter((c) => c.key === "GET /v1/agents/a1/space-bindings").length).toBe(2));
  });

  it("sends the chosen listen mode", async () => {
    const calls = setup({ "POST /v1/agents/a1/space-bindings": () => json(201, binding) });
    const select = await openPicker();
    fireEvent.change(select, { target: { value: "G1" } });
    fireEvent.change(screen.getByLabelText("Listen"), { target: { value: "all" } });
    fireEvent.click(screen.getByRole("button", { name: "Add to this channel" }));
    await waitFor(() => expect(calls.some((c) => c.key === "POST /v1/agents/a1/space-bindings")).toBe(true));
    expect(calls.find((c) => c.key === "POST /v1/agents/a1/space-bindings")?.body).toEqual({ integration_id: "i1", space_ref: "G1", listen: "all" });
  });

  it("says to invite the bot when no channel is listed, and reloads on demand", async () => {
    let n = 0;
    const calls = setup({ "GET /v1/integrations/i1/spaces": () => json(200, n++ === 0 ? { items: [] } : spaces) });
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
    fireEvent.click(await screen.findByRole("button", { name: "Add to a channel" }));
    expect(await screen.findByText(/Invite @daneel to a channel you are in/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add to this channel" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Reload channels" }));
    expect(await screen.findByLabelText("Channel")).toBeInTheDocument();
    expect(calls.filter((c) => c.key === "GET /v1/integrations/i1/spaces").length).toBe(2);
  });

  it("link_required: names the link-account action", async () => {
    setup({ "GET /v1/integrations/i1/spaces": () => err(409, "link_required", "Link your account first") });
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
    fireEvent.click(await screen.findByRole("button", { name: "Add to a channel" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Link your chat account first");
    expect(screen.getByRole("link", { name: "Link your chat account" })).toHaveAttribute("href", "/account/channels");
  });

  it("link_required on bind shows the same action", async () => {
    setup({ "POST /v1/agents/a1/space-bindings": () => err(409, "link_required") });
    const select = await openPicker();
    fireEvent.change(select, { target: { value: "C1" } });
    fireEvent.click(screen.getByRole("button", { name: "Add to this channel" }));
    expect(await screen.findByRole("link", { name: "Link your chat account" })).toBeInTheDocument();
  });

  const refusals: [string, number, string, string][] = [
    ["bot_not_in_space", 409, "Invite @daneel to the channel, then try again.", "Invite @daneel to the channel, then try again."],
    ["not_a_member", 403, "not a member", "You are not a member of that channel"],
    ["space_not_bindable", 422, "direct", "Direct conversations cannot be bound"],
    ["already_bound", 409, "bound", "already bound to that channel"],
  ];
  for (const [code, status, message, shown] of refusals) {
    it(`${code}: shows the refusal with its hint`, async () => {
      setup({ "POST /v1/agents/a1/space-bindings": () => err(status, code, message) });
      const select = await openPicker();
      fireEvent.change(select, { target: { value: "C1" } });
      fireEvent.click(screen.getByRole("button", { name: "Add to this channel" }));
      expect((await screen.findByRole("alert")).textContent).toContain(shown);
    });
  }

  it("a connector timeout while listing is shown and can be retried", async () => {
    setup({ "GET /v1/integrations/i1/spaces": () => err(503, "connector_unavailable") });
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
    fireEvent.click(await screen.findByRole("button", { name: "Add to a channel" }));
    expect((await screen.findByRole("alert")).textContent).toContain("did not answer in time");
    expect(screen.getByRole("button", { name: "Reload channels" })).toBeInTheDocument();
  });
});

describe("attach empty states", () => {
  it("preselects the only integration and a shared bot", async () => {
    const calls = setup({ "POST /v1/agents/a1/presences": () => json(201, { id: "p9" }) }, [], [option]);
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
    expect(await screen.findByRole("heading", { name: "Attach this agent to Mattermost" })).toBeInTheDocument();
    expect(screen.getByLabelText("Integration")).toHaveValue("i1");
    expect(screen.getByLabelText("Identity")).toHaveValue("shared");
    fireEvent.click(screen.getByRole("button", { name: "Attach" }));
    await waitFor(() => expect(calls.some((c) => c.key === "POST /v1/agents/a1/presences")).toBe(true));
    expect(calls.find((c) => c.key === "POST /v1/agents/a1/presences")?.body).toEqual({ integration_id: "i1", mode: "shared" });
  });

  it("no integration: an org admin gets the link to Channels", async () => {
    setup({}, [], []);
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
    const box = await screen.findByTestId("no-integration");
    expect(box.textContent).toContain("no chat integration yet");
    expect(await screen.findByRole("link", { name: "Connect a chat platform" })).toHaveAttribute("href", "/o/acme/channels");
  });

  it("no integration: a plain member only learns who can create one", async () => {
    setup({ "GET /v1/orgs/o1": () => json(200, { id: "o1", slug: "acme", name: "Acme", external_id: null, app_posture: "open", settings: {}, my_role: "member", created_at: T }) }, [], []);
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
    const box = await screen.findByTestId("no-integration");
    expect(box.textContent).toContain("organization owner or admin");
    await waitFor(() => expect(screen.queryByRole("link", { name: "Connect a chat platform" })).toBeNull());
  });
});
