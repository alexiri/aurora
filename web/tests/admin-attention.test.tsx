import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { AttentionBadge } from "@/components/admin/attention-badge";
import { AttentionSection } from "@/components/admin/attention-section";
import { ConnectorStatusChip } from "@/components/connectors/shared";
import type { ConnectorAdmin } from "@/lib/api/connectors";
import { err, json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

const T = "2026-11-18T10:00:00Z";
const decision = {
  kind: "connector_approval",
  subject_id: "mattermost",
  title: "Mattermost 1.3.0 is waiting for approval",
  detail: "It keeps running at the level you approved; only what is new waits. It adds: native_status.",
  since: T,
  href: "/admin/chat-platforms",
  decision: true,
};
const info = {
  kind: "ownerless_org",
  subject_id: "acme",
  title: "The organization acme has no owner",
  detail: "Its 2 active member(s) cannot use the gateway until someone is made an owner.",
  since: T,
  href: "/admin/people?tab=organizations",
  decision: false,
};

// FIX-167 Part B (D-370, C-1022): the Administration count, the Overview section and the Connectors chip.
describe("Administration attention badge", () => {
  it("shows the item count with its accessible text", async () => {
    mockFetch({ "GET /v1/admin/attention": () => json(200, { items: [decision, info] }) });
    wrap(<AttentionBadge />);
    const badge = await screen.findByTestId("admin-attention-badge");
    expect(badge).toHaveTextContent("2");
    expect(badge).toHaveTextContent("need your attention");
  });

  it("shows nothing for an empty list", async () => {
    const f = mockFetch({ "GET /v1/admin/attention": () => json(200, { items: [] }) });
    wrap(<AttentionBadge />);
    await waitFor(() => expect(f).toHaveBeenCalled());
    expect(screen.queryByTestId("admin-attention-badge")).toBeNull();
  });

  it("is hidden for non-admins and fetches nothing", async () => {
    const f = mockFetch({ "GET /v1/admin/attention": () => json(200, { items: [decision] }) });
    wrap(<AttentionBadge enabled={false} />);
    await new Promise((r) => setTimeout(r, 50));
    expect(f).not.toHaveBeenCalled();
    expect(screen.queryByTestId("admin-attention-badge")).toBeNull();
  });

  it("is hidden, with no error, when the fetch fails", async () => {
    const f = mockFetch({ "GET /v1/admin/attention": () => err(500, "internal") });
    wrap(<AttentionBadge />);
    await waitFor(() => expect(f).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByTestId("admin-attention-badge")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("Administration › Overview: Needs your attention", () => {
  it("lists each item with title, detail, since and one button to its page, decisions first", async () => {
    mockFetch({ "GET /v1/admin/attention": () => json(200, { items: [decision, info] }) });
    wrap(<AttentionSection />);
    const section = await screen.findByTestId("admin-attention");
    expect(section).toHaveTextContent("Needs your attention");
    const rows = section.querySelectorAll("li");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveAttribute("data-kind", "connector_approval");
    expect(rows[0]).toHaveTextContent(decision.title);
    expect(rows[0]).toHaveTextContent(decision.detail);
    expect(rows[0]).toHaveTextContent("Since");
    const review = screen.getByRole("link", { name: `Review: ${decision.title}` });
    expect(review).toHaveAttribute("href", "/admin/chat-platforms");
    expect(review).toHaveTextContent("Review");
    const open = screen.getByRole("link", { name: `Open: ${info.title}` });
    expect(open).toHaveAttribute("href", "/admin/people?tab=organizations");
  });

  it("renders no section without items, while loading, or on an error", async () => {
    const f = mockFetch({ "GET /v1/admin/attention": () => json(200, { items: [] }) });
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
