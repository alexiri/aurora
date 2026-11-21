import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AppShell } from "@/components/app-shell";
import { ManageFrame } from "@/components/org/manage-frame";
import { seedMoreControls } from "./support/render";
import { json } from "./support/api";

// (P5-REFINE-20, org-3) /o/new: an unknown organization is the not-found state inside the shell, shown once and for
// good: no fake "Manage new" area, no "Loading…" and no request storm (the 404 must not be asked again and again).

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "new" }),
  usePathname: () => "/o/new",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
}));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("an unknown organization (org-3)", () => {
  it("renders the not-found state, no Manage area, and asks for the slug once", async () => {
    const calls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL) => {
        const url = new URL(input instanceof Request ? input.url : String(input), "http://localhost");
        calls.push(url.pathname + url.search);
        if (url.pathname === "/v1/resolve") return json(404, { error: { code: "not_found", message: "nf" } });
        if (url.pathname === "/v1/me") {
          return json(200, {
            user: { id: "u1", email: "a@b.c", display_name: "A", is_instance_admin: false },
            default_org_id: null,
            memberships: [{ org: { id: "o1", slug: "acme", name: "Acme" }, role: "member", status: "active", workspaces: [{ id: "w1", slug: "lab", name: "Lab", my_role: "member", personal: false }] }],
          });
        }
        if (url.pathname === "/v1/instance") return json(200, { org_creation: "instance_admins", features: {} });
        return json(404, { error: { code: "not_found", message: "nf" } });
      }),
    );
    // The same defaults as the app (providers.tsx).
    const qc = new QueryClient({ defaultOptions: { queries: { staleTime: 30_000, retry: 1 } } });
    seedMoreControls(qc, false);
    render(
      <QueryClientProvider client={qc}>
        <AppShell>
          <ManageFrame>Overview page</ManageFrame>
        </AppShell>
      </QueryClientProvider>,
    );
    expect((await screen.findAllByRole("heading", { name: "We couldn't find that organization" })).length).toBeGreaterThan(0);
    await new Promise((r) => setTimeout(r, 600));
    expect(screen.queryByTestId("manage-area")).toBeNull();
    expect(screen.queryByText("Manage new")).toBeNull();
    expect(screen.queryByText("Overview page")).toBeNull();
    const resolves = calls.filter((c) => c.startsWith("/v1/resolve?org=new"));
    expect(resolves.length).toBeLessThanOrEqual(2);
  });
});
