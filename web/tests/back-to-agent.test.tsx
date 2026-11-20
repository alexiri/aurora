import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { BackToAgent } from "@/components/account/back-link";

const search = { value: "" };
vi.mock("next/navigation", () => ({ useSearchParams: () => new URLSearchParams(search.value) }));

afterEach(cleanup);

function setup(qs: string) {
  search.value = qs;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const url = new URL(input instanceof Request ? input.url : String(input), "http://localhost");
      const body =
        url.pathname === "/v1/resolve"
          ? { org: { id: "o1", slug: "acme", name: "Acme" }, workspace: { id: "w1", slug: "lab", name: "Lab" } }
          : url.pathname === "/v1/workspaces/w1/agents"
            ? { items: [{ id: "a1", handle: "daneel", name: "Daneel" }], next_cursor: null }
            : {};
      return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
    }),
  );
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <BackToAgent />
    </QueryClientProvider>,
  );
}

describe("BackToAgent (Account › SSH keys)", () => {
  it("shows ← Back to <Agent name> for a same-origin agent path", async () => {
    setup("from=%2Fo%2Facme%2Fw%2Flab%2Fagents%2Fdaneel");
    const link = await screen.findByRole("link", { name: /Back to Daneel/ });
    expect(link).toHaveAttribute("href", "/o/acme/w/lab/agents/daneel");
  });

  it("rejects from=//evil.test and other addresses: no link at all", () => {
    for (const bad of ["from=%2F%2Fevil.test", "from=https%3A%2F%2Fevil.test%2Fo%2Fx", "from=%2Faccount", "from=javascript%3Aalert(1)", ""]) {
      const { container, unmount } = setup(bad);
      expect(container.querySelector("a"), bad).toBeNull();
      unmount();
    }
  });
});
