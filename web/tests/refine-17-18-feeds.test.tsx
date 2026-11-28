import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

let pathname = "/o/acme";
vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useParams: () => ({ org: "acme" }),
  usePathname: () => pathname,
}));

import { ChannelsView } from "@/components/channels/channels-view";
import { historySentence } from "@/components/internet/internet-level";
import { OrgNavList } from "@/components/app-shell";
import { NeedsYouOrg } from "@/components/org/needs-you-org";
import { err, json } from "./support/api";
import { wrap } from "./support/render";

// P5-REFINE-17 items 14, 16, 17 and org-1 (the brief-8 and C-1087 items), on top of C-1080/C-1082.

const SINCE = "2026-11-19T10:00:00Z";
const REQ_ID = "22222222-2222-4222-8222-222222222222";
const common = { level: "organization", reason: null, own: false, state: "open", closed_at: null, message: null, closed_by_role: null, closed_by: null };
const request = {
  ...common,
  id: REQ_ID,
  kind: "chat_platform_request",
  topic: "chat-platforms",
  subject_id: REQ_ID,
  title: "Carol Member wants Mattermost",
  detail: "Your organization has no Mattermost connected.",
  since: SINCE,
  href: "/o/acme/chat-platforms",
  decision: true,
  actors: [{ user_id: "u-carol", display_name: "Carol Member" }],
  actors_total: 1,
  reason: "Our team lives in Mattermost.",
  can_decline: true,
  action: { label: "Connect a chat platform", href: "/o/acme/chat-platforms" },
};

type Handler = (req: Request, body: unknown) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const calls: { key: string; body: unknown; search: string }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input as Request;
      const url = new URL(req.url, "http://localhost");
      const key = `${req.method} ${url.pathname}`;
      const text = req.method === "GET" ? "" : await req.clone().text();
      const body = text ? (JSON.parse(text) as unknown) : null;
      calls.push({ key, body, search: url.search });
      const h = routes[key];
      return h ? h(req, body) : err(404, "not_found");
    }),
  );
  return calls;
}

beforeEach(() => {
  pathname = "/o/acme";
});
afterEach(() => cleanup());

describe("what a rule change changed (org-14, C-1087)", () => {
  it("reads who changed what, in one sentence", () => {
    const actor = { display_name: "Alice Admin" };
    expect(historySentence({ number: 5, actor, changes: [{ kind: "added", rule_name: "Block tracker" }], more: 0 })).toBe("Alice Admin added ‘Block tracker’");
    expect(historySentence({ number: 4, actor, changes: [{ kind: "removed", rule_name: "Ask before changes" }], more: 0 })).toBe("Alice Admin removed ‘Ask before changes’");
    expect(
      historySentence({ number: 3, actor, changes: [{ kind: "added", rule_name: "A" }, { kind: "removed", rule_name: "B" }], more: 0 }),
    ).toBe("Alice Admin added ‘A’ and removed ‘B’");
    expect(
      historySentence({ number: 3, actor, changes: [{ kind: "edited", rule_name: "A" }, { kind: "edited", rule_name: "B" }], more: 0 }),
    ).toBe("Alice Admin edited ‘A’ and ‘B’");
    expect(historySentence({ number: 2, actor, changes: [{ kind: "added", rule_name: "A" }, { kind: "added", rule_name: "B" }], more: 19 })).toBe("Alice Admin changed 21 rules");
  });

  it("falls back to the version when nothing changed, and to the publisher when there is no actor", () => {
    expect(historySentence({ number: 1, actor: { display_name: "Alice Admin" }, changes: [], more: 0 })).toBe("Alice Admin published version 1");
    // The seeded first version has no author: the starting rules, not "Someone published version 1".
    expect(historySentence({ number: 1, changes: [], more: 0 })).toBe("The starting rules, version 1");
    expect(historySentence({ number: 2, changes: [], more: 0 })).toBe("Someone published version 2");
    expect(historySentence({ number: 2, published_by: { display_name: "Bob Builder" }, changes: [{ kind: "added", rule_name: "X" }], more: 0 })).toBe("Bob Builder added ‘X’");
  });
});

describe("Manage › Overview: Needs you from the organization feed (#16, C-1080)", () => {
  const feed = (items: unknown[], count = items.length) => () => json(200, { items, count });

  it("shows the request with the asker, the reason, its action and Decline, and the count from the feed", async () => {
    mockFetch({
      "GET /v1/orgs/o1/attention": feed([request]),
      "GET /v1/orgs/o1/invitations": () => json(200, { items: [{ id: "i1" }, { id: "i2" }], next_cursor: null }),
    });
    wrap(<NeedsYouOrg orgId="o1" orgSlug="acme" />);
    const card = await screen.findByTestId("org-needs-you");
    expect(within(card).getByLabelText("1 waiting")).toBeInTheDocument();
    const row = within(card).getByTestId(`attention-item-${REQ_ID}`);
    expect(row).toHaveTextContent("Carol Member wants Mattermost");
    expect(row).toHaveTextContent("Your organization has no Mattermost connected.");
    expect(row).toHaveTextContent("Our team lives in Mattermost.");
    expect(within(row).getByRole("link", { name: /^Connect a chat platform/ })).toHaveAttribute("href", "/o/acme/chat-platforms");
    // The invitations are an uncounted extra row (C-1080 puts requests only at the organization level).
    expect(await within(card).findByText("2 invitations are waiting for an answer.")).toBeInTheDocument();
  });

  it("Decline asks for a message and posts it to the organization's decline route", async () => {
    const calls = mockFetch({
      "GET /v1/orgs/o1/attention": feed([request]),
      "GET /v1/orgs/o1/invitations": () => json(200, { items: [], next_cursor: null }),
      [`POST /v1/orgs/o1/app-setup-requests/${REQ_ID}/decline`]: () => json(200, {}),
    });
    wrap(<NeedsYouOrg orgId="o1" orgSlug="acme" />);
    fireEvent.click(await screen.findByRole("button", { name: /^Decline/ }));
    const dialog = await screen.findByRole("dialog", { name: "Decline Carol’s request?" });
    expect(dialog).toHaveTextContent("Carol Member asked for Mattermost. They will be told you declined.");
    fireEvent.change(within(dialog).getByLabelText("Message for Carol (optional)"), { target: { value: "Not this quarter." } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Decline" }));
    await waitFor(() => expect(calls.find((c) => c.key.startsWith("POST"))?.body).toEqual({ message: "Not this quarter." }));
  });

  it("a library update and a broken organization account: a line, a link and a status each (nav-3b)", async () => {
    const update = {
      ...common, id: "library_update:l1", kind: "library_update", topic: "skills", subject_id: "l1", since: SINCE,
      title: "Anthropic skills has an update to review", detail: "Shared with every workspace in Acme",
      href: "/o/acme/skills?tab=libraries", decision: true, actors: [], actors_total: 0, can_decline: false,
      action: { label: "Review", href: "/o/acme/skills?tab=libraries" },
    };
    const broken = {
      ...common, id: "org_account_broken:c1", kind: "org_account_broken", topic: "apps", subject_id: "c1", since: SINCE,
      title: "The GitHub account for Acme failed to refresh its token", detail: "Workspaces that use it stop until it is renewed",
      href: "/o/acme/apps?tab=accounts", decision: true, actors: [], actors_total: 0, can_decline: false,
      action: { label: "Fix", href: "/o/acme/apps?tab=accounts" },
    };
    mockFetch({
      "GET /v1/orgs/o1/attention": feed([update, broken]),
      "GET /v1/orgs/o1/invitations": () => json(200, { items: [], next_cursor: null }),
    });
    wrap(<NeedsYouOrg orgId="o1" orgSlug="acme" />);
    const card = await screen.findByTestId("org-needs-you");
    expect(within(card).getByLabelText("2 waiting")).toBeInTheDocument();

    const lib = within(card).getByTestId("attention-item-library_update:l1");
    expect(lib).toHaveTextContent("Skills · Shared with every workspace in Acme");
    expect(within(lib).getByRole("link", { name: update.title })).toHaveAttribute("href", "/o/acme/skills?tab=libraries");
    expect(within(lib).getByText("Review")).toBeInTheDocument();
    // The whole row opens the page: the status is not a second link.
    expect(within(lib).getAllByRole("link")).toHaveLength(1);

    const account = within(card).getByTestId("attention-item-org_account_broken:c1");
    expect(account).toHaveTextContent("Apps & accounts · Workspaces that use it stop until it is renewed");
    expect(within(account).getByRole("link", { name: broken.title })).toHaveAttribute("href", "/o/acme/apps?tab=accounts");
    expect(within(account).getByText("Failed")).toHaveClass("text-danger-ink");
    expect(within(account).getAllByRole("link")).toHaveLength(1);
  });

  it("nothing waits: no card", async () => {
    const calls = mockFetch({
      "GET /v1/orgs/o1/attention": feed([]),
      "GET /v1/orgs/o1/invitations": () => json(200, { items: [], next_cursor: null }),
    });
    wrap(<NeedsYouOrg orgId="o1" orgSlug="acme" />);
    await waitFor(() => expect(calls.some((c) => c.key === "GET /v1/orgs/o1/attention")).toBe(true));
    await new Promise((r) => setTimeout(r, 30));
    expect(screen.queryByTestId("org-needs-you")).toBeNull();
  });
});

describe("the organization's count on the Manage area (#17, G3, C-1082)", () => {
  const routes = (organization: unknown) => ({
    "GET /v1/orgs/resolve/acme": () => json(200, { org: { id: "o1", slug: "acme", name: "Acme" }, workspace: null }),
    "GET /v1/resolve": () => json(200, { org: { id: "o1", slug: "acme", name: "Acme" }, workspace: null }),
    "GET /v1/orgs/o1": () => json(200, { id: "o1", slug: "acme", name: "Acme", my_role: "owner" }),
    "GET /v1/me": () =>
      json(200, {
        user: { id: "u1", email: "alice@example.org", display_name: "Alice Admin", is_instance_admin: false },
        memberships: [{ org: { id: "o1", slug: "acme", name: "Acme" }, role: "owner", status: "active", workspaces: [] }],
      }),
    "GET /v1/attention/counts": () => json(200, { administration: null, organization, workspace: null }),
  });

  it("Overview carries the total and a topic only its own part; none at zero", async () => {
    mockFetch(routes({ org_id: "o1", count: 3, topics: { "chat-platforms": 2, apps: 1 } }));
    wrap(<OrgNavList orgSlug="acme" pathname="/o/acme" />);
    const total = await screen.findByTestId("manage-attention-badge-overview");
    expect(total).toHaveTextContent("3");
    expect(total).toHaveTextContent("3 need your attention");
    expect(screen.getByTestId("manage-attention-badge-chat-platforms")).toHaveTextContent("2");
    expect(screen.getByTestId("manage-attention-badge-apps")).toHaveTextContent("1");
    expect(screen.queryByTestId("manage-attention-badge-people")).toBeNull();
    // The same menu is the phone's drawer: the entry says its number in words.
    expect(screen.getByRole("link", { name: "Chat platforms 2 need your attention" })).toBeInTheDocument();
  });

  it("no badge when the API answers null (not an owner or admin) or zero", async () => {
    const calls = mockFetch(routes(null));
    wrap(<OrgNavList orgSlug="acme" pathname="/o/acme" />);
    await screen.findByRole("navigation", { name: "Manage Acme" });
    await waitFor(() => expect(calls.some((c) => c.key === "GET /v1/attention/counts")).toBe(true));
    await new Promise((r) => setTimeout(r, 30));
    expect(screen.queryByTestId("manage-attention-badge-overview")).toBeNull();
    cleanup();
    mockFetch(routes({ org_id: "o1", count: 0, topics: {} }));
    wrap(<OrgNavList orgSlug="acme" pathname="/o/acme" />);
    await screen.findByRole("navigation", { name: "Manage Acme" });
    await new Promise((r) => setTimeout(r, 30));
    expect(screen.queryByTestId("manage-attention-badge-overview")).toBeNull();
  });
});

describe("Chat platforms: one readiness, and who asked (org-1, brief 8)", () => {
  const mattermost = {
    id: "mattermost",
    display_name: "Mattermost",
    icon_url: "/v1/connectors/mattermost/icon",
    setup_guide_md: "",
    capabilities: { presence_dedicated: true, presence_shared: true },
    integration_setup: { fields: [] },
    presence_setup: {},
  };
  const ask = (over: Record<string, unknown> = {}) => ({
    id: REQ_ID,
    org_id: "o1",
    subject: "chat_platform",
    connector_type: "mattermost",
    app_id: "",
    app_name: "Mattermost",
    status: "open",
    askers: [{ user_id: "u-carol", display_name: "Carol Member", workspace_id: "w1", asked_at: SINCE }],
    needs: [],
    unavailable: false,
    created_at: SINCE,
    closed_at: null,
    ...over,
  });
  const routes = (asks: unknown[], role = "owner") => ({
    "GET /v1/connectors": () => json(200, { items: [mattermost] }),
    "GET /v1/orgs/o1": () => json(200, { id: "o1", slug: "acme", name: "Acme", my_role: role }),
    "GET /v1/orgs/o1/channel-integrations": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/connector-availability": () => json(200, { items: [], instance_admins: [] }),
    "GET /v1/orgs/o1/app-setup-requests": () => json(200, { items: asks }),
  });

  it("a running connector reads Ready · connector running with a Connect button", async () => {
    mockFetch(routes([]));
    wrap(<ChannelsView orgId="o1" />);
    const row = await screen.findByTestId("platform-mattermost");
    expect(row).toHaveTextContent("Mattermost");
    expect(row).toHaveTextContent("Ready · connector running");
    expect(within(row).getByText("Ready")).toBeInTheDocument();
    expect(within(row).getByRole("button", { name: "Connect Mattermost" })).toHaveTextContent(/^Connect$/);
    expect(screen.getByText("Let people talk to agents from the chat tools Acme already uses.")).toBeInTheDocument();
    expect(screen.queryByTestId("platform-ask")).toBeNull();
  });

  it("names who asked for it, under the row, with Connect Mattermost", async () => {
    mockFetch(routes([ask()]));
    wrap(<ChannelsView orgId="o1" />);
    const note = await screen.findByTestId("platform-ask");
    expect(note).toHaveTextContent("Carol Member asked for Mattermost. Connect it and Carol gets a notice.");
    fireEvent.click(within(note).getByRole("button", { name: "Connect Mattermost" }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("an ask without a named platform is one line under the list; a member never reads the asks", async () => {
    mockFetch(routes([ask({ connector_type: null, app_name: "" })]));
    wrap(<ChannelsView orgId="o1" />);
    expect(await screen.findByText("Carol Member asked for a chat platform. Connect one and Carol gets a notice.")).toBeInTheDocument();
    cleanup();
    const calls = mockFetch(routes([ask()], "member"));
    wrap(<ChannelsView orgId="o1" />);
    await screen.findByTestId("platform-mattermost");
    await new Promise((r) => setTimeout(r, 30));
    expect(calls.some((c) => c.key === "GET /v1/orgs/o1/app-setup-requests")).toBe(false);
    expect(screen.queryByTestId("platform-ask")).toBeNull();
  });
});
