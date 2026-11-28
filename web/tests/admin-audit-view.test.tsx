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
  actor_name: "Root Admin",
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

// The viewer of Administration is an instance administrator: Set up and above, so a row has its Details button
// (it opens the Technical details fold).
const ADMIN = { user: { id: "u1", display_name: "Root Admin", email: "root@x.io", is_instance_admin: true } };

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
      if (url.pathname === "/v1/me") return new Response(JSON.stringify(ADMIN), { status: 200, headers: { "content-type": "application/json" } });
      const body = url.pathname.endsWith("/admin/orgs") ? { items: [] } : { items: [entry("1", "instance_settings.update", null)], next_cursor: null };
      if (url.pathname === "/v1/admin/audit") seen.push(url.search);
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
    await screen.findByText("Root changed the instance settings");
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
    await screen.findByText("Root changed the instance settings");
    const q = new URLSearchParams(seen[0]);
    expect(q.get("scope")).toBe("instance");
    expect(q.has("since")).toBe(false);
    expect(q.has("until")).toBe(false);
  });
  it("writes a changed filter back, keeping tab and leaving out empty values", async () => {
    stubAudit([]);
    view();
    await screen.findByText("Root changed the instance settings");
    fireEvent.click(screen.getByLabelText("Security-critical only"));
    expect(nav.replace).toHaveBeenLastCalledWith("/admin/data?tab=audit&critical=1", { scroll: false });
    // (admin-10) One "All actions" select in words, applied at once: no typed key, no Filter button.
    const action = screen.getByLabelText("Action") as HTMLSelectElement;
    expect(action.options[0]!.textContent).toBe("All actions");
    expect(screen.queryByRole("button", { name: "Filter" })).toBeNull();
    expect(document.body.textContent).not.toMatch(/Action prefix|e\.g\. plugin/);
    fireEvent.change(action, { target: { value: "plugin." } });
    expect(nav.replace).toHaveBeenLastCalledWith("/admin/data?tab=audit&action=plugin.&critical=1", { scroll: false });
    expect(action.selectedOptions[0]!.textContent).toBe("Apps");
  });
  it("opens and closes a row's Details", async () => {
    stubAudit([]);
    view();
    await screen.findByText("Root changed the instance settings");
    const btn = await screen.findByRole("button", { name: "Details" });
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
          if (url.pathname === "/v1/admin/audit") seen.push(url.search);
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
    expect(await screen.findByText("Root trusted a new app publisher")).toBeTruthy();
    expect(screen.queryByText(/acme/)).toBeNull();
    expect(seen[0]).toContain("scope=instance");
    expect(screen.getAllByText("Critical")).toHaveLength(1);

    fireEvent.click(screen.getByLabelText("All organizations"));
    expect(await screen.findByText("Root paused an organization")).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/did something|root@x\.io/);
    // The line under a sentence names the organization, or the instance for an instance-level row.
    expect(screen.getByText(/· acme/)).toBeTruthy();
    expect(screen.getByText(/· Instance$/)).toBeTruthy();

    fireEvent.change(await screen.findByLabelText("Organization"), { target: { value: "o1" } });
    await waitFor(() => expect(seen.some((s) => s.includes("org=o1"))).toBe(true));

    fireEvent.click(screen.getByLabelText("Security-critical only"));
    await waitFor(() => expect(seen.some((s) => s.includes("critical=true"))).toBe(true));
  });
});

describe("System events are hidden by the server, with its count (C-1088, admin-10)", () => {
  it("asks include_system=false, shows hidden_system, and asks again with the switch", async () => {
    const seen: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL) => {
        const req = input instanceof Request ? input : null;
        const url = new URL(req ? req.url : String(input), "http://localhost");
        if (url.pathname.endsWith("/admin/orgs")) return new Response(JSON.stringify({ items: [] }), { status: 200, headers: { "content-type": "application/json" } });
        if (url.pathname === "/v1/me") return new Response(JSON.stringify(ADMIN), { status: 200, headers: { "content-type": "application/json" } });
        if (url.pathname !== "/v1/admin/audit") return new Response(JSON.stringify({ error: { code: "not_found", message: "nf" } }), { status: 404, headers: { "content-type": "application/json" } });
        seen.push(url.search);
        const withSystem = url.searchParams.get("include_system") === "true";
        const items = [entry("1", "instance_settings.update", null), ...(withSystem ? [{ ...entry("2", "plugin.ingest", null), actor_kind: "system", actor_label: "system" }] : [])];
        return new Response(JSON.stringify({ items, next_cursor: null, ...(withSystem ? {} : { hidden_system: 40 }) }), { status: 200, headers: { "content-type": "application/json" } });
      }),
    );
    view();
    await screen.findByText("Root changed the instance settings");
    expect(new URLSearchParams(seen[0]).get("include_system")).toBe("false");
    expect(await screen.findByText("40 system install events are hidden.")).toBeTruthy();
    fireEvent.click(screen.getByRole("switch", { name: "Show system events" }));
    // A machine actor in words: Aurora, never the slug "system".
    expect(await screen.findByText("Aurora added the package of an app")).toBeTruthy();
    expect(screen.getAllByTestId("audit-sentence")).toHaveLength(2);
    expect(new URLSearchParams(seen[seen.length - 1]).get("include_system")).toBe("true");
  });
});
