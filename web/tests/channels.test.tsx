import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

let search = "token=aui_secret";
vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(search),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useParams: () => ({}),
  usePathname: () => "/",
}));

import { AccountChannelsView } from "@/components/channels/account-channels-view";
import { AgentChannelsTab } from "@/components/channels/agent-channels-tab";
import { ChannelsView } from "@/components/channels/channels-view";
import { LinkConfirmView } from "@/components/channels/link-confirm-view";
import { ConnectorsAdminView } from "@/components/connectors/connectors-admin-view";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
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
beforeEach(() => {
  search = "token=aui_secret";
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const T = "2026-01-01T00:00:00Z";
const slack = {
  id: "slack", display_name: "Slack", icon_url: "/v1/connectors/slack/icon", setup_guide_md: "Create an **app**",
  capabilities: { presence_dedicated: true, presence_shared: true },
  integration_setup: { fields: [
    { name: "workspace", label: "Workspace", type: "text", required: true, store: "metadata" },
    { name: "bot_token", label: "Bot token", type: "secret", required: true, store: "credentials" },
  ] },
  presence_setup: {},
};
const integ = {
  id: "i1", org_id: "o1", connector_id: "slack", tenant_ref: "T1", tenant_label: "Acme Slack", config: {},
  shared_bot: { handle: "@aurora", display_name: "Aurora" }, status: "active", status_reason: null, created_at: T, updated_at: T,
};

describe("ChannelsView", () => {
  function setup(post: Handler) {
    return mockFetch({
      "GET /v1/connectors": () => json(200, { items: [slack] }),
      "GET /v1/orgs/o1/channel-integrations": () => json(200, { items: [integ], next_cursor: null }),
      "POST /v1/orgs/o1/channel-integrations": post,
    });
  }
  async function fill() {
    wrap(<ChannelsView orgId="o1" />);
    expect(await screen.findByText("Acme Slack")).toBeInTheDocument();
    expect(screen.getByText("Credentials: stored")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Connect Slack/ }));
    fireEvent.change(await screen.findByLabelText("Workspace"), { target: { value: "acme" } });
    const secret = screen.getByLabelText("Bot token") as HTMLInputElement;
    expect(secret.value).toBe("");
    fireEvent.change(secret, { target: { value: "xoxb-1" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
  }

  it("splits config and credentials by store and shows a pending state", async () => {
    const calls = setup(async () => {
      await new Promise((r) => setTimeout(r, 30));
      return json(201, integ);
    });
    await fill();
    expect(await screen.findByRole("button", { name: "Validating…" })).toBeDisabled();
    await waitFor(() => expect(screen.queryByRole("button", { name: "Validating…" })).toBeNull());
    expect(calls.find((c) => c.key.startsWith("POST"))?.body).toEqual({
      connector_id: "slack", config: { workspace: "acme" }, credentials: { bot_token: "xoxb-1" },
    });
  });

  it.each([
    [409, "tenant_already_bound", "connected to another organization"],
    [422, "credentials_invalid", "The platform rejected the credentials: <b>bad token</b>"],
    [503, "connector_unavailable", "did not answer in time"],
  ])("maps %s %s", async (status, code, text) => {
    setup(() => err(status, code, "<b>bad token</b>"));
    await fill();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain(text);
    expect(alert.querySelector("b")).toBeNull(); // plain text, never markup
  });

  it("renders the setup guide with the safe markdown renderer", async () => {
    setup(() => json(201, integ));
    wrap(<ChannelsView orgId="o1" />);
    fireEvent.click(await screen.findByRole("button", { name: /Connect Slack/ }));
    expect((await screen.findByText("app")).tagName).toBe("STRONG");
  });
});

describe("ChannelsView verified-email auto-link switch (RFE-001)", () => {
  const withEmail = { ...slack, capabilities: { ...slack.capabilities, email_verified: true } };
  function setup(connector: unknown, config: Record<string, unknown>) {
    return mockFetch({
      "GET /v1/connectors": () => json(200, { items: [connector] }),
      "GET /v1/orgs/o1/channel-integrations": () => json(200, { items: [{ ...integ, config }], next_cursor: null }),
      "PATCH /v1/channel-integrations/i1": () => json(200, { ...integ, config }),
    });
  }

  it("is off by default, needs a confirm with the trust warning, then patches config", async () => {
    const calls = setup(withEmail, { _capabilities: { email_verified: true }, workspace: "acme" });
    wrap(<ChannelsView orgId="o1" />);
    const box = (await screen.findByLabelText("Link members automatically by verified email")) as HTMLInputElement;
    expect(box.checked).toBe(false);
    fireEvent.click(box);
    expect(screen.getByRole("alertdialog").textContent).toContain("it can create links");
    expect(calls.some((c) => c.key.startsWith("PATCH"))).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Confirm enable" }));
    await waitFor(() => expect(calls.some((c) => c.key.startsWith("PATCH"))).toBe(true));
    expect(calls.find((c) => c.key.startsWith("PATCH"))?.body).toEqual({
      config: { workspace: "acme", verified_email_autolink: true },
    });
  });

  it("is hidden when the connector does not offer email_verified", async () => {
    setup(slack, {});
    wrap(<ChannelsView orgId="o1" />);
    await screen.findByText("Acme Slack");
    expect(screen.queryByLabelText("Link members automatically by verified email")).toBeNull();
  });
});

describe("ConnectorsAdminView", () => {
  const pending = {
    id: "slack", display_name: "Slack", status: "pending_approval", version: "2.0.0", contract: 1,
    manifest: { capabilities: { threads: true, files: true, edits: false }, integration_setup: { fields: [{ name: "bot_token", type: "secret", store: "credentials" }] } },
    manifest_digest: "sha256:new", approved_manifest_digest: "sha256:old", widening: ["adds capability files"],
    replicas: [], integration_count: 2, approved_at: null, created_at: T,
    managed: false, host_status: [], secrets: {},
  };

  it("renders an old-vs-new manifest diff from approved_manifest", async () => {
    mockFetch({
      "GET /v1/admin/connectors": () =>
        json(200, {
          items: [
            {
              ...pending,
              manifest: {
                capabilities: { threads: true, files: true },
                integration_setup: { fields: [{ name: "bot_token", type: "secret", store: "credentials" }] },
                presence_setup: { dedicated: { fields: [{ name: "token", type: "secret", store: "credentials" }] } },
                public_callbacks: ["/hook"],
              },
              approved_manifest: {
                capabilities: { threads: true, edits: true },
                integration_setup: { fields: [{ name: "bot_token", type: "string", store: "credentials" }] },
                public_callbacks: [],
              },
            },
          ],
        }),
    });
    wrap(<ConnectorsAdminView />);
    const diff = within(await screen.findByTestId("manifest-diff"));
    expect(diff.getByText("files")).toBeInTheDocument();
    expect(diff.getByText("edits")).toBeInTheDocument();
    expect(diff.getByText("Integration: bot_token")).toBeInTheDocument();
    expect(diff.getByText("Dedicated presence: token")).toBeInTheDocument();
    expect(diff.getByText("/hook")).toBeInTheDocument();
    expect(diff.getByText("files").closest("li")?.textContent).toContain("Added:");
    expect(diff.getByText("edits").closest("li")?.textContent).toContain("Removed:");
    expect(diff.getByText("Integration: bot_token").closest("li")?.textContent).toContain("Changed:");
  });

  it("shows the widening diff and approves the exact digest; stale manifest refetches", async () => {
    let digest = "sha256:new";
    let first = true;
    const calls = mockFetch({
      "GET /v1/admin/connectors": () => json(200, { items: [{ ...pending, manifest_digest: digest }] }),
      "POST /v1/admin/connectors/slack/approve": () => {
        if (first) {
          first = false;
          digest = "sha256:newer";
          return err(409, "stale_manifest", "changed");
        }
        return json(200, { ...pending, status: "active", widening: [] });
      },
    });
    wrap(<ConnectorsAdminView />);
    expect(await screen.findByText("adds capability files")).toBeInTheDocument();
    const summary = within(screen.getByTestId("manifest-summary"));
    expect(summary.getByText("files")).toBeInTheDocument();
    expect(summary.queryByText("edits")).toBeNull();
    expect(summary.getByText("bot_token")).toBeInTheDocument();
    const approve = screen.getByRole("button", { name: "Approve manifest" });
    expect(approve).toBeDisabled();
    fireEvent.click(screen.getByLabelText(/I have reviewed/));
    fireEvent.click(approve);
    expect((await screen.findByRole("alert")).textContent).toContain("different manifest");
    expect(calls.find((c) => c.key.endsWith("/approve"))?.body).toEqual({ manifest_digest: "sha256:new" });
    // refetched, review required again
    await waitFor(() => expect(calls.filter((c) => c.key === "GET /v1/admin/connectors").length).toBe(2));
    expect(screen.getByRole("button", { name: "Approve manifest" })).toBeDisabled();
    fireEvent.click(screen.getByLabelText(/I have reviewed/));
    fireEvent.click(screen.getByRole("button", { name: "Approve manifest" }));
    await waitFor(() =>
      expect(calls.filter((c) => c.key.endsWith("/approve")).at(-1)?.body).toEqual({ manifest_digest: "sha256:newer" }),
    );
  });

  it("shows the provisioned token once", async () => {
    mockFetch({
      "GET /v1/admin/connectors": () => json(200, { items: [] }),
      "POST /v1/admin/connectors": () => json(201, { connector: { ...pending, status: "awaiting_registration", manifest: null }, token: "aun_once" }),
    });
    wrap(<ConnectorsAdminView />);
    fireEvent.click(await screen.findByRole("button", { name: "Add connector" }));
    fireEvent.click(await screen.findByRole("button", { name: "Self-hosted connector (advanced)" }));
    fireEvent.change(await screen.findByLabelText("Connector id"), { target: { value: "slack" } });
    fireEvent.change(screen.getByLabelText("Display name"), { target: { value: "Slack" } });
    fireEvent.click(screen.getByRole("button", { name: "Provision" }));
    expect((await screen.findByTestId("secret-key")).textContent).toBe("aun_once");
    fireEvent.click(screen.getByRole("button", { name: "I have saved it" }));
    await waitFor(() => expect(screen.queryByText("aun_once")).toBeNull());
    expect(document.body.textContent).not.toContain("aun_once");
  });

  it("maps connector_in_use on delete", async () => {
    mockFetch({
      "GET /v1/admin/connectors": () => json(200, { items: [{ ...pending, status: "active", widening: [] }] }),
      "DELETE /v1/admin/connectors/slack": () => err(409, "connector_in_use"),
    });
    wrap(<ConnectorsAdminView />);
    fireEvent.click(await screen.findByRole("button", { name: "Delete" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm delete" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Integrations still use");
  });
});

describe("AgentChannelsTab", () => {
  const grant = {
    id: "g1", agent_id: "a1", integration_id: "i1", subject_kind: "space", subject_ref: "C1", subject_label: "#general",
    state: "pending", listen: "mentions", revision: 3, decided_by: null, decided_at: null, expires_at: null, created_at: T,
  };
  function setup(extra: Record<string, Handler> = {}, caps: Record<string, boolean> = { presence_shared: true }) {
    return mockFetch({
      "GET /v1/connectors": () => json(200, { items: [{ ...slack, capabilities: caps }] }),
      "GET /v1/orgs/o1/channel-integrations": () => json(200, { items: [integ], next_cursor: null }),
      "GET /v1/agents/a1/presences": () => json(200, { items: [] }),
      "GET /v1/agents/a1/presence-options": () =>
        json(200, {
          items: [
            { integration_id: "i1", connector_id: "slack", connector_name: "Slack", tenant_label: "Acme Slack",
              modes: [...(caps.presence_dedicated ? ["dedicated"] : []), ...(caps.presence_shared ? ["shared"] : [])],
              presence_setup: {}, attached: false },
          ],
        }),
      "GET /v1/agents/a1/space-bindings": () =>
        json(200, { items: [{ id: "b1", integration_id: "i1", space_ref: "C1", space_label: "#general", agent_id: "a1", listen: "mentions", followups_need_mention: false, active: true, bound_by: null, created_at: T }] }),
      "GET /v1/agents/a1/reach": () => json(200, { items: [grant, { ...grant, id: "g2", subject_label: "#old", state: "blocked" }], next_cursor: null }),
      ...extra,
    });
  }

  it("offers only the modes the connector supports and maps no_shared_bot", async () => {
    const calls = setup({ "POST /v1/agents/a1/presences": () => err(422, "no_shared_bot") });
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
    fireEvent.change(await screen.findByLabelText("Integration"), { target: { value: "i1" } });
    const mode = screen.getByLabelText("Identity");
    const opts = within(mode).getAllByRole("option").map((o) => o.textContent);
    expect(opts.some((o) => o?.startsWith("Shared bot"))).toBe(true);
    expect(opts.some((o) => o?.startsWith("Dedicated"))).toBe(false);
    fireEvent.change(mode, { target: { value: "shared" } });
    fireEvent.click(screen.getByRole("button", { name: "Attach" }));
    expect((await screen.findByRole("alert")).textContent).toContain("no shared bot");
    expect(calls.find((c) => c.key === "POST /v1/agents/a1/presences")?.body).toEqual({ integration_id: "i1", mode: "shared" });
  });

  it("attaches through presence-options without the owner-only integration list", async () => {
    const calls = setup({ "GET /v1/orgs/o1/channel-integrations": () => err(403, "forbidden") });
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn={false} />);
    fireEvent.change(await screen.findByLabelText("Integration"), { target: { value: "i1" } });
    expect(screen.getByLabelText("Identity")).toBeInTheDocument();
    expect(calls.some((c) => c.key === "GET /v1/orgs/o1/channel-integrations")).toBe(false);
    expect(calls.some((c) => c.key === "GET /v1/agents/a1/presence-options")).toBe(true);
  });

  it("approves a reach request with its revision and refetches on stale_revision", async () => {
    const calls = setup({ "POST /v1/reach-grants/g1/decision": () => err(409, "stale_revision") });
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
    fireEvent.click(await screen.findByRole("button", { name: "Approve #general" }));
    expect((await screen.findAllByRole("alert")).some((a) => a.textContent?.includes("changed in the meantime"))).toBe(true);
    expect(calls.find((c) => c.key.endsWith("/decision"))?.body).toEqual({ decision: "approve", listen: "mentions", revision: 3 });
    await waitFor(() => expect(calls.filter((c) => c.key === "GET /v1/agents/a1/reach").length).toBe(2));
    expect(screen.getByText("History (1)")).toBeInTheDocument();
  });

  it("edits and removes space bindings", async () => {
    const calls = setup({
      "PATCH /v1/space-bindings/b1": () => json(200, {}),
      "DELETE /v1/space-bindings/b1": () => json(204, null),
    });
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
    fireEvent.change(await screen.findByLabelText("Listen mode for #general"), { target: { value: "all" } });
    fireEvent.click(screen.getByLabelText("Follow-ups need a mention"));
    fireEvent.click(screen.getByRole("button", { name: "Remove binding #general" }));
    await waitFor(() => expect(calls.some((c) => c.key === "DELETE /v1/space-bindings/b1")).toBe(true));
    const patches = calls.filter((c) => c.key.startsWith("PATCH"));
    expect(patches[0].body).toEqual({ listen: "all" });
    expect(patches[1].body).toEqual({ followups_need_mention: true });
  });

  it("hides owner-only controls without can_own", async () => {
    const calls = setup();
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn={false} />);
    expect(await screen.findByText("#general", { selector: "span" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Approve #general" })).toBeNull();
    expect(screen.queryByText("Space bindings")).toBeNull();
    expect(calls.some((c) => c.key.includes("space-bindings"))).toBe(false);
  });

  it("detaches and re-attaches a presence", async () => {
    const calls = setup({
      "GET /v1/agents/a1/presences": () =>
        json(200, { items: [{ id: "p1", agent_id: "a1", integration_id: "i1", connector_id: "slack", tenant_label: "Acme Slack", mode: "shared", handle: null, status: "needs_attention", status_reason: "token revoked", created_at: T }] }),
      "POST /v1/presences/p1/reattach": () => json(200, {}),
      "DELETE /v1/presences/p1": () => json(204, null),
    });
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn />);
    expect(await screen.findByText("token revoked")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Re-attach" }));
    fireEvent.click(screen.getByRole("button", { name: "Detach" }));
    await waitFor(() => expect(calls.some((c) => c.key === "DELETE /v1/presences/p1")).toBe(true));
    expect(calls.some((c) => c.key === "POST /v1/presences/p1/reattach")).toBe(true);
  });
});

describe("LinkConfirmView", () => {
  const me = { user: { id: "u1", display_name: "Ada", email: "ada@x.test" }, memberships: [] };
  const link = { id: "l1", integration_id: "i1", connector_id: "slack", tenant_label: "Acme Slack", platform_user_ref: "U123", user: { id: "u1", display_name: "Ada" }, method: "sign_in_link", created_at: T };

  it("confirms and strips the token from the URL", async () => {
    const replace = vi.spyOn(window.history, "replaceState");
    const calls = mockFetch({
      "GET /v1/me": () => json(200, me),
      "POST /v1/channel-links/confirm": () => json(200, link),
    });
    wrap(<LinkConfirmView />);
    fireEvent.click(await screen.findByRole("button", { name: "Confirm link" }));
    expect(await screen.findByText(/is now linked to Ada/)).toBeInTheDocument();
    expect(calls.find((c) => c.key.endsWith("/confirm"))?.body).toEqual({ token: "aui_secret" });
    expect(replace).toHaveBeenCalledWith(null, "", "/link/channel");
    expect(window.location.href).not.toContain("aui_secret");
    expect(document.body.textContent).not.toContain("aui_secret");
  });

  it.each([
    [410, "link_expired", "expired or was already used"],
    [409, "already_linked", "already linked to another Aurora user"],
  ])("handles %s %s and still strips the token", async (status, code, text) => {
    const replace = vi.spyOn(window.history, "replaceState");
    mockFetch({
      "GET /v1/me": () => json(200, me),
      "POST /v1/channel-links/confirm": () => err(status, code),
    });
    wrap(<LinkConfirmView />);
    fireEvent.click(await screen.findByRole("button", { name: "Confirm link" }));
    expect((await screen.findByRole("alert")).textContent).toContain(text);
    expect(replace).toHaveBeenCalledWith(null, "", "/link/channel");
    expect(screen.queryByRole("button", { name: "Confirm link" })).toBeNull();
  });

  it("redirects to login with a return URL when signed out", async () => {
    const assign = vi.fn();
    vi.stubGlobal("location", { ...window.location, assign, pathname: "/link/channel", search: "?token=aui_secret" });
    mockFetch({ "GET /v1/me": () => err(401, "unauthorized") });
    wrap(<LinkConfirmView />);
    await waitFor(() => expect(assign).toHaveBeenCalled());
    expect(assign.mock.calls[0][0]).toContain("/auth/login?next=");
  });

  it("rejects a link without a token", async () => {
    search = "";
    mockFetch({ "GET /v1/me": () => json(200, me) });
    wrap(<LinkConfirmView />);
    expect((await screen.findByRole("alert")).textContent).toContain("incomplete");
  });
});

describe("AccountChannelsView", () => {
  it("lists links, unlinks and maps agent_not_reachable for DM defaults", async () => {
    const calls = mockFetch({
      "GET /v1/me": () => json(200, { user: { id: "u1", display_name: "Ada", email: "a@x.test" }, memberships: [{ org: { id: "o1", name: "Acme", slug: "acme" }, role: "member", status: "active", workspaces: [{ id: "w1", name: "Main", slug: "main", my_role: "member" }] }] }),
      "GET /v1/workspaces/w1/agents": () => json(200, { items: [{ id: "a1", name: "Helper", handle: "helper" }], next_cursor: null }),
      "GET /v1/me/channel-links": () =>
        json(200, { items: [{ id: "l1", integration_id: "i1", connector_id: "slack", tenant_label: "Acme Slack", platform_user_ref: "U123", user: { id: "u1", display_name: "Ada" }, method: "verified_email", created_at: T }] }),
      "GET /v1/me/dm-defaults": () => json(200, { items: [{ integration_id: "i1", platform_user_ref: "U123", agent_id: null }] }),
      "PUT /v1/me/dm-defaults": () => err(422, "agent_not_reachable"),
      "DELETE /v1/me/channel-links/l1": () => json(204, null),
    });
    wrap(<AccountChannelsView />);
    const select = await screen.findByLabelText("Default agent for direct messages");
    await waitFor(() => expect(within(select).getAllByRole("option").length).toBe(2));
    fireEvent.change(select, { target: { value: "a1" } });
    expect((await screen.findByRole("alert")).textContent).toContain("cannot reach that agent");
    expect(calls.find((c) => c.key === "PUT /v1/me/dm-defaults")?.body).toEqual({ integration_id: "i1", agent_id: "a1" });
    fireEvent.click(screen.getByRole("button", { name: "Unlink Acme Slack" }));
    await waitFor(() => expect(calls.some((c) => c.key === "DELETE /v1/me/channel-links/l1")).toBe(true));
  });
});
