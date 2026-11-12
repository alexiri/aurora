import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { OwnerlessBanner } from "@/components/admin/ownerless-banner";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function mockOverview(body: unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => json(200, body)),
  );
}

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

// FIX-050 (C-630): the Admin warning banner names the ownerless organizations and how to repair
// them, and stays hidden when there are none (the field is absent on such responses).
describe("admin ownerless-org banner (FIX-050, C-630)", () => {
  it("renders for ownerless_orgs and names the organizations with the remediation", async () => {
    mockOverview({
      connector_hosts: [],
      counters: { backend: "postgres", state: "shared" },
      ownerless_orgs: [
        { slug: "acme", members: 3, created_at: "2026-11-01T10:00:00Z" },
        { slug: "globex", members: 1, created_at: "2026-11-02T10:00:00Z" },
      ],
    });
    wrap(<OwnerlessBanner />);
    const banner = await screen.findByRole("alert");
    expect(banner).toHaveTextContent("acme");
    expect(banner).toHaveTextContent("globex");
    expect(banner.textContent).toContain("no active owner");
    expect(banner.textContent).toContain("docs/operator/install-helm.md");
  });

  it("renders nothing for an overview without ownerless_orgs (none)", async () => {
    mockOverview({ connector_hosts: [], counters: { backend: "postgres", state: "shared" } });
    wrap(<OwnerlessBanner />);
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  });

  it("renders nothing for an empty ownerless_orgs", async () => {
    mockOverview({
      connector_hosts: [],
      counters: { backend: "postgres", state: "shared" },
      ownerless_orgs: [],
    });
    wrap(<OwnerlessBanner />);
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  });
});
