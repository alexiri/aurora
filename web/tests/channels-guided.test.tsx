import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(""),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useParams: () => ({}),
  usePathname: () => "/",
}));

import { ChannelsView } from "@/components/channels/channels-view";
import { dmUrl } from "@/components/channels/connected-done";
import { ConnectorAvailableCallout } from "@/components/connectors/available-callout";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const err = (status: number, code: string, message = code) => json(status, { error: { code, message } });
type Handler = (req: Request, body: unknown) => Response | Promise<Response>;
interface Rec { key: string; body: unknown }
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
      calls.push({ key, body });
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
const mattermost = {
  id: "mattermost", display_name: "Mattermost", icon_url: "/v1/connectors/mattermost/icon", setup_guide_md: "Create a **bot**",
  capabilities: { presence_shared: true },
  integration_setup: { fields: [
    { name: "server_url", label: "Your Mattermost server", type: "text", required: true, store: "config" },
    { name: "team", label: "Team", type: "text", required: true, store: "config", choices: "from_validate" },
    { name: "bot_token", label: "Bot access token", type: "secret", required: true, store: "credentials" },
    { name: "admin_token", label: "Admin token", type: "secret", required: false, store: "credentials" },
  ] },
  presence_setup: {},
};
const avail = (state: string) => ({
  items: [{ type_id: "mattermost", name: "Mattermost", icon_url: null, setup_state: state, connector_id: null }],
  instance_admins: [{ name: "Ada Admin" }, { name: "Bo Boss" }],
});
const me = (admin: boolean) => ({ user: { id: "u1", display_name: "Me", is_instance_admin: admin }, memberships: [] });
const created = {
  id: "i1", org_id: "o1", connector_id: "mattermost", tenant_ref: "x", tenant_label: "@daneel on mm.test", config: {},
  shared_bot: { handle: "daneel", display_name: "Daneel" }, status: "active", status_reason: null, created_at: T, updated_at: T,
};

function base(extra: Record<string, Handler>, opts: { connectors?: unknown[]; availability?: unknown; admin?: boolean } = {}) {
  return mockFetch({
    "GET /v1/me": () => json(200, me(opts.admin === true)),
    "GET /v1/connectors": () => json(200, { items: opts.connectors ?? [] }),
    "GET /v1/connector-availability": () => json(200, opts.availability ?? { items: [], instance_admins: [] }),
    "GET /v1/orgs/o1/channel-integrations": () => json(200, { items: [], next_cursor: null }),
    ...extra,
  });
}

describe("Channels: blocked states name the next actor", () => {
  it.each([
    ["not_added", "An instance administrator needs to add Mattermost first."],
    ["waiting_for_host", "waiting for the connector host. The operator must start it."],
    ["disabled", "Mattermost is disabled. An instance administrator can enable it."],
    ["needs_approval", "needs an instance administrator's approval"],
  ])("%s, as a plain member: says what is wrong and who to ask", async (state, text) => {
    base({}, { availability: avail(state) });
    wrap(<ChannelsView orgId="o1" />);
    const note = await screen.findByTestId("blocked-mattermost");
    expect(note.textContent).toContain(text);
    expect(note.textContent).toContain("Ask: Ada Admin, Bo Boss.");
    expect(screen.queryByRole("button", { name: "Add Mattermost" })).toBeNull();
    expect(screen.queryByRole("link", { name: /Admin › Connectors/ })).toBeNull();
  });

  it("not added, as an instance admin: an inline Add Mattermost opens the one-click flow", async () => {
    const mm = {
      type_id: "mattermost", name: "Mattermost", version: "1", publisher: "Aurora", icon_png_b64: null, setup_guide_md: "x",
      manifest: { capabilities: {}, public_callbacks: [] }, manifest_digest: "sha256:mm", instance_setup: { fields: [] }, hosts: 1, added: false,
    };
    const calls = base(
      {
        "GET /v1/admin/connector-types": () => json(200, { host_running: true, items: [mm] }),
        "POST /v1/admin/connectors": () => json(201, { connector: { id: "mattermost" } }),
      },
      { availability: avail("not_added"), admin: true },
    );
    wrap(<ChannelsView orgId="o1" />);
    fireEvent.click(await screen.findByRole("button", { name: "Add Mattermost" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(await within(dialog).findByRole("button", { name: "Add Mattermost" }));
    await waitFor(() => expect(calls.some((c) => c.key === "POST /v1/admin/connectors")).toBe(true));
    expect(calls.find((c) => c.key === "POST /v1/admin/connectors")?.body).toMatchObject({
      id: "mattermost", approve_manifest_digest: "sha256:mm", settings: {},
    });
  });

  it("other states, as an instance admin: a link to Admin › Connectors, no names to ask", async () => {
    base({}, { availability: avail("disabled"), admin: true });
    wrap(<ChannelsView orgId="o1" />);
    expect(await screen.findByRole("link", { name: /Admin › Connectors/ })).toHaveAttribute("href", "/admin/connectors");
    expect(screen.getByTestId("blocked-mattermost").textContent).not.toContain("Ask:");
  });

  it("nothing announced: names the administrators instead of a dead end", async () => {
    base({}, { availability: { items: [], instance_admins: [{ name: "Ada Admin" }] } });
    wrap(<ChannelsView orgId="o1" />);
    expect(await screen.findByText(/No chat platform is available yet/)).toBeInTheDocument();
    expect(screen.getByText(/Ask Ada Admin to set one up/)).toBeInTheDocument();
  });
});

describe("Channels: the integration stepper", () => {
  function open(extra: Record<string, Handler>) {
    const calls = base(
      {
        "GET /v1/orgs/o1/workspaces": () => json(200, { items: [{ id: "w1", name: "Main", org_id: "o1" }], next_cursor: null }),
        "GET /v1/workspaces/w1/agents": () => json(200, { items: [{ id: "a1", name: "Scout" }] }),
        ...extra,
      },
      { connectors: [mattermost], availability: avail("active") },
    );
    wrap(<ChannelsView orgId="o1" />);
    return calls;
  }
  async function step1() {
    fireEvent.click(await screen.findByRole("button", { name: /Connect Mattermost/ }));
    fireEvent.change(await screen.findByLabelText("Your Mattermost server"), { target: { value: "https://chat.example.com" } });
    fireEvent.change(screen.getByLabelText("Bot access token"), { target: { value: "tok" } });
    // The team and the admin token are not asked yet.
    expect(screen.queryByLabelText(/^Team/)).toBeNull();
    expect(screen.queryByLabelText(/Admin token/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Check" }));
  }

  it("check → pick the team → connect → done screen with the three next steps", async () => {
    const calls = open({
      "POST /v1/orgs/o1/channel-integrations:probe": () =>
        json(200, {
          choices: { team: [{ value: "alpha", label: "Alpha Team" }, { value: "beta", label: "Beta" }] },
          tenant_label: "@daneel on chat.example.com", warnings: ["No admin token: replies arrive as DMs."],
        }),
      "POST /v1/orgs/o1/channel-integrations": () => json(201, created),
      "POST /v1/agents/a1/presences": () => json(201, { id: "p1" }),
    });
    await step1();
    // Step 2: a select filled from the probe, the warning, the admin token.
    const team = (await screen.findByLabelText("Team")) as HTMLSelectElement;
    expect(team.tagName).toBe("SELECT");
    expect(within(team).getAllByRole("option").map((o) => o.textContent)).toEqual(["Select…", "Alpha Team", "Beta"]);
    expect(screen.getByText("No admin token: replies arrive as DMs.")).toBeInTheDocument();
    expect(screen.getByLabelText(/Admin token/)).toBeInTheDocument();
    // The probe carried what step 1 asked, credentials separated from config, nothing else.
    expect(calls.find((c) => c.key.endsWith(":probe"))?.body).toEqual({
      connector_id: "mattermost",
      config: { server_url: "https://chat.example.com" },
      credentials: { bot_token: "tok" },
    });
    // The team is required: connecting without it is refused client-side.
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(calls.some((c) => c.key === "POST /v1/orgs/o1/channel-integrations")).toBe(false);
    fireEvent.change(team, { target: { value: "alpha" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    const done = await screen.findByTestId("connected-done");
    expect(calls.find((c) => c.key === "POST /v1/orgs/o1/channel-integrations")?.body).toEqual({
      connector_id: "mattermost",
      config: { server_url: "https://chat.example.com", team: "alpha" },
      credentials: { bot_token: "tok" },
    });
    expect(done.textContent).toContain("Mattermost is connected as @daneel in Alpha Team.");
    // Next steps: DM link, account link, attach.
    expect(within(done).getByRole("link", { name: "Open a DM with @daneel" })).toHaveAttribute(
      "href",
      "https://chat.example.com/alpha/messages/@daneel",
    );
    expect(within(done).getByRole("link", { name: "Link your Mattermost account" })).toHaveAttribute("href", "/account/channels");
    await within(done).findByRole("option", { name: "Scout" });
    fireEvent.change(within(done).getByLabelText("Agent"), { target: { value: "a1" } });
    fireEvent.click(within(done).getByRole("button", { name: "Attach with the shared bot" }));
    expect(await within(done).findByText(/Attached\. People can now talk to this agent through @daneel/)).toBeInTheDocument();
    expect(calls.find((c) => c.key === "POST /v1/agents/a1/presences")?.body).toEqual({ integration_id: "i1", mode: "shared" });
  });

  it("one team is preselected", async () => {
    open({
      "POST /v1/orgs/o1/channel-integrations:probe": () =>
        json(200, { choices: { team: [{ value: "only", label: "Only Team" }] }, tenant_label: null, warnings: [] }),
    });
    await step1();
    const team = (await screen.findByLabelText("Team")) as HTMLSelectElement;
    expect(team.value).toBe("only");
  });

  it("a rejected check stays on step 1 and shows the platform's words as text", async () => {
    open({ "POST /v1/orgs/o1/channel-integrations:probe": () => err(422, "credentials_invalid", "<b>bad token</b>") });
    await step1();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("The platform rejected the credentials: <b>bad token</b>");
    expect(alert.querySelector("b")).toBeNull();
    expect(screen.getByLabelText("Bot access token")).toBeInTheDocument();
  });

  it("a connector that cannot probe: the team is typed, create still validates", async () => {
    open({ "POST /v1/orgs/o1/channel-integrations:probe": () => err(422, "probe_unsupported", "no") });
    await step1();
    expect(await screen.findByText(/cannot list the choices/)).toBeInTheDocument();
    const team = (await screen.findByLabelText("Team")) as HTMLInputElement;
    expect(team.tagName).toBe("INPUT");
  });
});

describe("dmUrl", () => {
  it("builds the direct-message address and refuses anything that is not http(s)", () => {
    expect(dmUrl("https://mm.example.com/", "team", "@bot")).toBe("https://mm.example.com/team/messages/@bot");
    expect(dmUrl("https://mm.example.com/chat/", "t", "bot")).toBe("https://mm.example.com/chat/t/messages/@bot");
    expect(dmUrl("javascript:alert(1)", "t", "bot")).toBeNull();
    expect(dmUrl("https://mm.example.com", undefined, "bot")).toBeNull();
  });
});

describe("Admin › Setup: connector callout", () => {
  const type = (added: boolean, signed = true) => ({
    type_id: "mattermost", name: "Mattermost", version: "1", publisher: "Aurora", icon_png_b64: null, setup_guide_md: "x",
    manifest: {}, manifest_digest: "d", instance_setup: { fields: [] }, hosts: 1, added, signed,
  });
  it("says a built-in type is available and links to Admin › Connectors; nothing is added by itself", async () => {
    const calls = mockFetch({ "GET /v1/admin/connector-types": () => json(200, { host_running: true, items: [type(false)] }) });
    wrap(<ConnectorAvailableCallout />);
    expect(await screen.findByText("Mattermost is available — add it.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Add Mattermost" })).toHaveAttribute("href", "/admin/connectors");
    expect(calls.every((c) => c.key.startsWith("GET"))).toBe(true);
  });
  it.each([["added", type(true)], ["unsigned", type(false, false)]])("renders nothing for a type that is %s", async (_n, t) => {
    mockFetch({ "GET /v1/admin/connector-types": () => json(200, { host_running: true, items: [t] }) });
    const { container } = wrap(<ConnectorAvailableCallout />);
    await waitFor(() => expect(container.querySelector("[aria-label]")).toBeNull());
  });
});
