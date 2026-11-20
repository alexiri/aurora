import { useMutation } from "@tanstack/react-query";
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { useRefreshAttentionOnMutation } from "@/lib/api/attention";

import { AttentionBadge } from "@/components/admin/attention-badge";
import { AttentionSection } from "@/components/admin/attention-section";
import { ConnectorStatusChip } from "@/components/connectors/shared";
import type { ConnectorAdmin } from "@/lib/api/connectors";
import { err, json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

const T = "2026-11-18T10:00:00Z";
// (C-1080) The item shape every level shares.
const base = {
  level: "administration",
  actors: [],
  actors_total: 0,
  reason: null,
  own: false,
  can_decline: false,
  state: "open",
  closed_at: null,
  message: null,
  closed_by_role: null,
  closed_by: null,
};
const decision = {
  ...base,
  id: "connector_approval:mattermost",
  kind: "connector_approval",
  topic: "chat-platforms",
  subject_id: "mattermost",
  title: "Mattermost 1.3.0 is waiting for approval",
  detail: "It keeps running at the level you approved; only what is new waits. It adds: native_status.",
  since: T,
  href: "/admin/chat-platforms",
  decision: true,
  action: { label: "Review", href: "/admin/chat-platforms" },
};
const info = {
  ...base,
  id: "ownerless_org:acme",
  kind: "ownerless_org",
  topic: "people",
  subject_id: "acme",
  title: "The organization acme has no owner",
  detail: "Its 2 active member(s) cannot use the gateway until someone is made an owner.",
  since: T,
  href: "/admin/people?tab=organizations",
  decision: false,
  action: { label: "Open", href: "/admin/people?tab=organizations" },
};
const request = {
  ...base,
  id: "11111111-1111-4111-8111-111111111111",
  kind: "chat_platform_request",
  topic: "chat-platforms",
  subject_id: "11111111-1111-4111-8111-111111111111",
  title: "Carol Member wants a chat platform",
  detail: "Your organization has no chat platform connected.",
  since: T,
  href: "/admin/chat-platforms",
  decision: true,
  actors: [{ user_id: "u-carol", display_name: "Carol Member" }],
  actors_total: 1,
  reason: "We use it every day.",
  can_decline: true,
  action: { label: "Connect a chat platform", href: "/admin/chat-platforms" },
};
const counts = (count: number, topics: Record<string, number> = {}) => json(200, { administration: { count, topics }, organization: null, workspace: null });

// FIX-167 Part B (D-370, C-1022), P5-REFINE-18 #18 (C-1082): the Administration count, the Overview section and the Connectors chip.
describe("Administration attention badge", () => {
  it("shows the count with its accessible text", async () => {
    mockFetch({ "GET /v1/attention/counts": () => counts(2) });
    wrap(<AttentionBadge />);
    const badge = await screen.findByTestId("admin-attention-badge");
    expect(badge).toHaveTextContent("2");
    expect(badge).toHaveTextContent("2 need your attention");
  });

  it("a topic badge shows only that topic's part", async () => {
    mockFetch({ "GET /v1/attention/counts": () => counts(3, { "chat-platforms": 2 }) });
    wrap(
      <>
        <AttentionBadge topic="chat-platforms" />
        <AttentionBadge topic="people" />
      </>,
    );
    expect(await screen.findByTestId("admin-attention-badge-chat-platforms")).toHaveTextContent("2");
    expect(screen.queryByTestId("admin-attention-badge-people")).toBeNull();
  });

  it("shows nothing at zero", async () => {
    const f = mockFetch({ "GET /v1/attention/counts": () => counts(0) });
    wrap(<AttentionBadge />);
    await waitFor(() => expect(f).toHaveBeenCalled());
    expect(screen.queryByTestId("admin-attention-badge")).toBeNull();
  });

  it("is hidden for non-admins (the API answers null) and, disabled, fetches nothing", async () => {
    const g = mockFetch({ "GET /v1/attention/counts": () => json(200, { administration: null, organization: null, workspace: null }) });
    wrap(<AttentionBadge />);
    await waitFor(() => expect(g).toHaveBeenCalled());
    expect(screen.queryByTestId("admin-attention-badge")).toBeNull();
    cleanup();
    const f = mockFetch({ "GET /v1/attention/counts": () => counts(1) });
    wrap(<AttentionBadge enabled={false} />);
    await new Promise((r) => setTimeout(r, 50));
    expect(f).not.toHaveBeenCalled();
    expect(screen.queryByTestId("admin-attention-badge")).toBeNull();
  });

  it("is hidden, with no error, when the fetch fails", async () => {
    const f = mockFetch({ "GET /v1/attention/counts": () => err(500, "internal") });
    wrap(<AttentionBadge />);
    await waitFor(() => expect(f).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByTestId("admin-attention-badge")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

function Resolver() {
  useRefreshAttentionOnMutation(true);
  const m = useMutation({ mutationFn: async () => "ok" });
  return (
    <button type="button" onClick={() => m.mutate()}>
      Resolve
    </button>
  );
}

describe("the count follows what is resolved (P5-REFINE-18 #18)", () => {
  it("refetches the count after any mutation succeeds, without a reload", async () => {
    let n = 2;
    mockFetch({ "GET /v1/attention/counts": () => counts(n) });
    wrap(
      <>
        <AttentionBadge />
        <Resolver />
      </>,
    );
    expect(await screen.findByTestId("admin-attention-badge")).toHaveTextContent("2");
    n = 1;
    fireEvent.click(screen.getByRole("button", { name: "Resolve" }));
    await waitFor(() => expect(screen.getByTestId("admin-attention-badge")).toHaveTextContent("1"));
  });
});

describe("Administration › Overview: Needs you", () => {
  it("lists each item in the one row shape with its action, decisions first, and the count", async () => {
    mockFetch({ "GET /v1/admin/attention": () => json(200, { items: [decision, info], count: 2 }) });
    wrap(<AttentionSection />);
    const section = await screen.findByTestId("admin-attention");
    expect(section).toHaveTextContent("Needs you");
    expect(within(section).getByText("2", { selector: "[data-attention-for]" })).toBeInTheDocument();
    const rows = section.querySelectorAll("li");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveAttribute("data-kind", "connector_approval");
    expect(rows[0]).toHaveTextContent(decision.title);
    expect(rows[0]).toHaveTextContent(decision.detail);
    const review = screen.getByRole("link", { name: `Review: ${decision.title}` });
    expect(review).toHaveAttribute("href", "/admin/chat-platforms");
    expect(review).toHaveTextContent("Review");
    const open = screen.getByRole("link", { name: `Open: ${info.title}` });
    expect(open).toHaveAttribute("href", "/admin/people?tab=organizations");
    // Facts are not requests: no Decline.
    expect(screen.queryByRole("button", { name: /^Decline/ })).toBeNull();
  });

  it("a request shows the asker, the reason and Decline, which asks for an optional message and declines it (setup-8)", async () => {
    let body: unknown = null;
    mockFetch({
      "GET /v1/admin/attention": () => json(200, { items: [request], count: 1 }),
      "POST /v1/admin/requests/11111111-1111-4111-8111-111111111111/decline": async (req) => {
        body = await req.json();
        return json(200, {});
      },
    });
    wrap(<AttentionSection />);
    const row = await screen.findByTestId(`attention-item-${request.id}`);
    expect(row).toHaveTextContent("Carol Member wants a chat platform");
    expect(row).toHaveTextContent("Your organization has no chat platform connected.");
    expect(row).toHaveTextContent("We use it every day.");
    expect(within(row).getByRole("link", { name: /^Connect a chat platform/ })).toHaveAttribute("href", "/admin/chat-platforms");
    fireEvent.click(within(row).getByRole("button", { name: /^Decline/ }));
    const dialog = await screen.findByRole("dialog", { name: "Decline Carol’s request?" });
    expect(dialog).toHaveTextContent("Carol Member asked for a chat platform. They will be told you declined.");
    fireEvent.change(within(dialog).getByLabelText("Message for Carol (optional)"), { target: { value: "Not this quarter." } });
    expect(dialog).toHaveTextContent("Carol sees this with the notice.");
    fireEvent.click(within(dialog).getByRole("button", { name: "Decline" }));
    await waitFor(() => expect(body).toEqual({ message: "Not this quarter." }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("declines without a message when the field is left empty", async () => {
    let hit = false;
    mockFetch({
      "GET /v1/admin/attention": () => json(200, { items: [request], count: 1 }),
      "POST /v1/admin/requests/11111111-1111-4111-8111-111111111111/decline": async (req) => {
        hit = (await req.text()) === "";
        return json(200, {});
      },
    });
    wrap(<AttentionSection />);
    fireEvent.click(await screen.findByRole("button", { name: /^Decline/ }));
    fireEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Decline" }));
    await waitFor(() => expect(hit).toBe(true));
  });

  it("History is fetched only when opened and lists declined items with the message, by role", async () => {
    const done = {
      ...request,
      state: "declined",
      can_decline: false,
      action: null,
      closed_at: "2026-11-19T09:00:00Z",
      closed_by_role: "instance_admin",
      closed_by: { display_name: "Alice Admin" },
      message: "Not this quarter.",
    };
    const f = mockFetch({
      "GET /v1/admin/attention": (req) => (new URL(req.url).searchParams.get("state") === "done" ? json(200, { items: [done], count: 0 }) : json(200, { items: [], count: 0 })),
    });
    wrap(<AttentionSection />);
    await waitFor(() => expect(f).not.toHaveBeenCalled());
    fireEvent.click(await screen.findByText("History"));
    const row = await screen.findByTestId(`attention-history-${request.id}`);
    expect(row).toHaveTextContent("Declined by Alice Admin");
    expect(row).toHaveTextContent("Not this quarter.");
  });

  it("renders no card without items, while loading, or on an error", async () => {
    const f = mockFetch({ "GET /v1/admin/attention": () => json(200, { items: [], count: 0 }) });
    const { unmount } = wrap(<AttentionSection />);
    expect(screen.queryByTestId("admin-attention")).toBeNull();
    await waitFor(() => expect(f).toHaveBeenCalled());
    expect(screen.queryByTestId("admin-attention")).toBeNull();
    unmount();
    const g = mockFetch({ "GET /v1/admin/attention": () => err(403, "forbidden") });
    wrap(<AttentionSection />);
    await waitFor(() => expect(g).toHaveBeenCalled());
    expect(screen.queryByTestId("admin-attention")).toBeNull();
  });
});

describe("Connectors: a pending update", () => {
  const base = {
    id: "mattermost",
    display_name: "Mattermost",
    status: "active",
    managed: true,
    host_status: [],
    widening: [],
    update_pending: false,
    pending_version: null,
  } as unknown as ConnectorAdmin;

  it("shows Running and a second chip, Update waiting for approval", () => {
    wrap(<ConnectorStatusChip c={{ ...base, update_pending: true, pending_version: "1.3.0" }} />);
    expect(screen.getByTestId("status-chip")).toHaveTextContent("Running");
    expect(screen.getByTestId("update-pending-chip")).toHaveTextContent("Update waiting for approval");
  });

  it("shows only the status chip when nothing waits", () => {
    wrap(<ConnectorStatusChip c={base} />);
    expect(screen.getByTestId("status-chip")).toHaveTextContent("Running");
    expect(screen.queryByTestId("update-pending-chip")).toBeNull();
  });

  it("a first registration is Needs approval, with no second chip", () => {
    wrap(<ConnectorStatusChip c={{ ...base, status: "pending_approval", update_pending: false }} />);
    expect(screen.getByTestId("status-chip")).toHaveTextContent("Needs approval");
    expect(screen.queryByTestId("update-pending-chip")).toBeNull();
  });
});
