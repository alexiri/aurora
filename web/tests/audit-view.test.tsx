import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { OrgAuditView } from "@/components/org/audit-view";

const entry = (id: string, action: string, critical: boolean) => ({
  id,
  org_id: "o1",
  workspace_id: null,
  actor_kind: "user",
  actor_id: null,
  actor_label: "a@x.io",
  action,
  target_kind: null,
  target_id: null,
  outcome: "success",
  source: "api",
  ip: null,
  metadata: {},
  critical,
  created_at: "2026-11-08T10:00:00Z",
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("OrgAuditView (RFE-008)", () => {
  it("badges critical rows and filters to the security-critical set", async () => {
    const seen: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL) => {
        const req = input instanceof Request ? input : null;
        const url = new URL(req ? req.url : String(input), "http://localhost");
        seen.push(url.search);
        const only = url.searchParams.get("critical") === "true";
        const items = only
          ? [entry("1", "policy.publish", true)]
          : [entry("1", "policy.publish", true), entry("2", "agent.create", false)];
        return new Response(JSON.stringify({ items, next_cursor: null }), {
          status: 200,
          headers: { "content-type": "application/json" },
        });
      }),
    );
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <OrgAuditView orgId="o1" />
      </QueryClientProvider>,
    );
    expect(await screen.findByText("agent.create")).toBeTruthy();
    // Only the critical row carries the badge.
    expect(screen.getAllByText("Critical")).toHaveLength(1);

    fireEvent.click(screen.getByLabelText("Security-critical only"));
    expect(await screen.findByText("policy.publish")).toBeTruthy();
    await waitFor(() => expect(screen.queryByText("agent.create")).toBeNull());
    expect(seen.some((s) => s.includes("critical=true"))).toBe(true);
  });
});
