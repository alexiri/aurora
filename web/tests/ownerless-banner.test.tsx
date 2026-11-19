import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { OwnerlessBanner } from "@/components/admin/ownerless-banner";
import { wrap } from "./support/render";
import { json } from "./support/api";

function mockOverview(body: unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => json(200, body)),
  );
}

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
    expect(banner.textContent).toContain("“Install with Helm” in the operator guide");
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
