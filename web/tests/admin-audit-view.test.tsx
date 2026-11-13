import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AdminAuditView } from "@/components/admin/audit-view";

const entry = (id: string, action: string, org: string | null) => ({
  id,
  org_id: org ? "o1" : null,
  org_slug: org,
  workspace_id: null,
  actor_kind: "instance_admin",
  actor_id: null,
  actor_label: "root@x.io",
  action,
  target_kind: null,
  target_id: null,
  outcome: "success",
  source: "api",
  ip: null,
  metadata: {},
  critical: action === "trusted_key.create",
  created_at: "2026-11-08T10:00:00Z",
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("AdminAuditView (FIX-125)", () => {
  it("lists instance rows by default, and org rows with an Organization column in the all scope", async () => {
    const seen: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL) => {
        const req = input instanceof Request ? input : null;
        const url = new URL(req ? req.url : String(input), "http://localhost");
        let body: unknown;
        if (url.pathname.endsWith("/admin/orgs")) {
          body = { items: [{ id: "o1", name: "Acme", slug: "acme", created_at: "2026-11-01T00:00:00Z" }] };
        } else {
          seen.push(url.search);
          const all = url.searchParams.get("scope") === "all";
          body = {
            items: all
              ? [entry("1", "trusted_key.create", null), entry("2", "org.freeze", "acme")]
              : [entry("1", "trusted_key.create", null)],
            next_cursor: null,
          };
        }
        return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
      }),
    );
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <AdminAuditView />
      </QueryClientProvider>,
    );
    expect(await screen.findByText("trusted_key.create")).toBeTruthy();
    expect(screen.queryByText("org.freeze")).toBeNull();
    expect(screen.queryByText("Organization")).toBeNull();
    expect(seen[0]).toContain("scope=instance");
    expect(screen.getAllByText("Critical")).toHaveLength(1);

    fireEvent.click(screen.getByLabelText("All organizations"));
    expect(await screen.findByText("org.freeze")).toBeTruthy();
    expect(screen.getByText("Organization")).toBeTruthy();
    expect(screen.getByText("acme")).toBeTruthy();
    // The scope toggle and the instance-level row's organization cell.
    expect(screen.getAllByText("Instance")).toHaveLength(2);

    fireEvent.change(await screen.findByLabelText("Organization"), { target: { value: "o1" } });
    await waitFor(() => expect(seen.some((s) => s.includes("org=o1"))).toBe(true));

    fireEvent.click(screen.getByLabelText("Security-critical only"));
    await waitFor(() => expect(seen.some((s) => s.includes("critical=true"))).toBe(true));
  });
});
