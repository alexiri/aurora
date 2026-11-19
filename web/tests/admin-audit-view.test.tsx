import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AdminAuditView } from "@/components/admin/audit-view";

const nav = vi.hoisted(() => ({ search: "tab=audit", replace: vi.fn() }));
vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(nav.search),
  usePathname: () => "/admin/data",
  useRouter: () => ({ replace: nav.replace, push: vi.fn() }),
}));

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

// (cleanup and unstubAllGlobals come from tests/setup.ts)
afterEach(() => {
  nav.search = "tab=audit";
  nav.replace.mockClear();
});

function stubAudit(seen: string[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const body = url.pathname.endsWith("/admin/orgs") ? { items: [] } : { items: [entry("1", "instance_settings.update", null)], next_cursor: null };
      if (!url.pathname.endsWith("/admin/orgs")) seen.push(url.search);
      return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
    }),
  );
}

function view() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AdminAuditView />
    </QueryClientProvider>,
  );
}

describe("AdminAuditView filters in the URL (FIX-171)", () => {
  it("takes its first filters from the search parameters", async () => {
    nav.search = "tab=audit&action=instance_settings.&scope=all&critical=1&since=2026-11-18&until=2026-11-19";
    const seen: string[] = [];
    stubAudit(seen);
    view();
    await screen.findByText("instance_settings.update");
    const q = new URLSearchParams(seen[0]);
    expect(q.get("action")).toBe("instance_settings.");
    expect(q.get("scope")).toBe("all");
    expect(q.get("critical")).toBe("true");
    expect(q.get("since")).toBe("2026-11-18T00:00:00.000Z");
    expect(q.get("until")).toBe("2026-11-20T00:00:00.000Z");
  });
  it("ignores an invalid date or scope", async () => {
    nav.search = "tab=audit&scope=everything&since=yesterday&until=2026-13-45";
    const seen: string[] = [];
    stubAudit(seen);
    view();
    await screen.findByText("instance_settings.update");
    const q = new URLSearchParams(seen[0]);
    expect(q.get("scope")).toBe("instance");
    expect(q.has("since")).toBe(false);
    expect(q.has("until")).toBe(false);
  });
  it("writes a changed filter back, keeping tab and leaving out empty values", async () => {
    stubAudit([]);
    view();
    await screen.findByText("instance_settings.update");
    fireEvent.click(screen.getByLabelText("Security-critical only"));
    expect(nav.replace).toHaveBeenLastCalledWith("/admin/data?tab=audit&critical=1", { scroll: false });
    fireEvent.change(screen.getByLabelText("Filter by action"), { target: { value: "plugin." } });
    fireEvent.click(screen.getByRole("button", { name: "Filter" }));
    expect(nav.replace).toHaveBeenLastCalledWith("/admin/data?tab=audit&action=plugin.&critical=1", { scroll: false });
  });
  it("opens and closes a row's Details", async () => {
    stubAudit([]);
    view();
    await screen.findByText("instance_settings.update");
    const btn = screen.getByRole("button", { name: /Details of instance_settings.update/ });
    expect(btn).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(btn);
    expect(btn).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByTestId("audit-detail")).toBeInTheDocument();
    fireEvent.click(btn);
    expect(screen.queryByTestId("audit-detail")).toBeNull();
  });
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
            items: all ? [entry("1", "trusted_key.create", null), entry("2", "org.freeze", "acme")] : [entry("1", "trusted_key.create", null)],
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
