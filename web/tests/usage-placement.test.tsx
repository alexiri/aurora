import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  usePathname: () => "/x",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(window.location.search),
  useParams: () => ({ org: "acme", ws: "main" }),
}));

import ActivityPage from "@/app/o/[org]/w/[ws]/activity/page";
import { AdminDataPage } from "@/components/admin/admin-topics";
import { OrgDataPage } from "@/components/org/org-topics";
import { TABS, adminRoutes, orgRoutes, wsRoutes } from "@/lib/routes";

// P5-USAGE-2 (C-1004, C-961): where the usage view lives, and who is shown it.

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const z = { turns: 0, turn_seconds: 0, awake_seconds: 0, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0 };
const usage = { from: "2026-10-19", to: "2026-11-17", totals: z, by_day: [], by_agent: [], by_org: [], by_workspace: [], other_workspaces: null, by_model: [], runtime_as_of: null };

function mock(opts: { admin?: boolean; role?: string } = {}) {
  const seen: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const url = new URL((input as Request).url, "http://localhost");
      seen.push(url.pathname);
      switch (url.pathname) {
        case "/v1/me":
          return json(200, { user: { id: "u1", email: "a@x.io", is_instance_admin: opts.admin === true }, memberships: [] });
        case "/v1/resolve":
          return json(200, { org: { id: "o1", slug: "acme" }, workspace: { id: "w1", slug: "main" } });
        case "/v1/orgs/o1":
          return json(200, { id: "o1", slug: "acme", name: "Acme", my_role: opts.role ?? "owner", contacts: { owners: [], admins: [] }, settings: {} });
        case "/v1/workspaces/w1/usage":
        case "/v1/orgs/o1/usage":
        case "/v1/admin/usage":
          return json(200, usage);
        default:
          return json(200, { items: [], next_cursor: null });
      }
    }),
  );
  return seen;
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.history.replaceState(null, "", "/");
});

function wrap(ui: React.ReactElement) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("route helpers and tab ids", () => {
  it("Activity, Data (organization) and Data (Administration) gain their tabs", () => {
    expect(TABS.wsActivity).toEqual(["requests", "usage"]);
    expect(TABS.adminData).toEqual(["retention", "storage", "audit", "usage"]);
    expect(TABS.orgData).toEqual(["retention", "storage", "audit", "usage"]);
    expect(wsRoutes("acme", "main").usage()).toBe("/o/acme/w/main/activity?tab=usage");
    expect(wsRoutes("acme", "main").usage("a1")).toBe("/o/acme/w/main/activity?tab=usage&agent=a1");
    expect(orgRoutes("acme").data("usage")).toBe("/o/acme/data?tab=usage");
    expect(adminRoutes.data("usage")).toBe("/admin/data?tab=usage");
  });
});

describe("Activity tabs", () => {
  it("opens on Requests, and an old link without ?tab= (even with ?agent=) still does", async () => {
    window.history.replaceState(null, "", "/o/acme/w/main/activity?agent=a1");
    const seen = mock();
    wrap(<ActivityPage />);
    expect(await screen.findByRole("tab", { name: "Requests" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tab", { name: "Usage" })).toHaveAttribute("aria-selected", "false");
    expect(await screen.findByText(/Every request your agents made through the gateway/)).toBeInTheDocument();
    expect(seen).not.toContain("/v1/workspaces/w1/usage");
  });

  it("?tab=usage opens the usage view", async () => {
    window.history.replaceState(null, "", "/o/acme/w/main/activity?tab=usage");
    const seen = mock();
    wrap(<ActivityPage />);
    expect(await screen.findByRole("tab", { name: "Usage" })).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByText(/How much your agents used/)).toBeInTheDocument();
    expect(seen).toContain("/v1/workspaces/w1/usage");
  });
});

describe("Data tabs", () => {
  it("Administration › Data has four tabs for an instance admin; the usage tab shows the instance view", async () => {
    window.history.replaceState(null, "", "/admin/data?tab=usage");
    const seen = mock({ admin: true });
    wrap(<AdminDataPage />);
    for (const name of ["Retention", "Storage", "Audit log", "Usage"]) expect(await screen.findByRole("tab", { name })).toBeInTheDocument();
    expect(await screen.findByLabelText("Organization")).toBeInTheDocument();
    expect(seen).toContain("/v1/admin/usage");
  });

  it("Administration stays hidden from people who are not instance admins", async () => {
    mock({ admin: false });
    wrap(<AdminDataPage />);
    await screen.findByText("This page could not be found.");
    expect(screen.queryByRole("tab", { name: "Usage" })).toBeNull();
  });

  it("the organization's Data has four tabs for an owner; its usage tab has the workspace filter", async () => {
    window.history.replaceState(null, "", "/o/acme/data?tab=usage");
    const seen = mock({ role: "owner" });
    wrap(<OrgDataPage />);
    for (const name of ["Retention", "Storage", "Audit log", "Usage"]) expect(await screen.findByRole("tab", { name })).toBeInTheDocument();
    expect(await screen.findByLabelText("Workspace")).toBeInTheDocument();
    expect(seen).toContain("/v1/orgs/o1/usage");
  });

  it("a plain member of the organization gets no Data page, so no Usage", async () => {
    const seen = mock({ role: "member" });
    wrap(<OrgDataPage />);
    await screen.findByRole("alert");
    expect(screen.queryByRole("tab", { name: "Usage" })).toBeNull();
    expect(seen).not.toContain("/v1/orgs/o1/usage");
  });
});
