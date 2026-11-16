import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const replace = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace, push: vi.fn() }),
  usePathname: () => "/",
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import Home from "@/app/page";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/** The placement request of an instance administrator stays pending until the returned function is called. */
function stubApi(opts: { admin: boolean; saved: boolean }) {
  let release: () => void = () => undefined;
  const gate = new Promise<void>((r) => {
    release = r;
  });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
      const p = new URL(req.url).pathname;
      if (p === "/v1/orgs") return json(200, { items: [], next_cursor: null });
      if (p === "/v1/me") return json(200, { user: { id: "u1", email: "root@x.io", display_name: "Root", is_instance_admin: opts.admin }, memberships: [] });
      if (p === "/v1/me/onboarding") return json(200, { welcome_seen: true, guide_dismissed: [], tour_completed_at: null });
      if (p === "/v1/admin/placement") {
        await gate;
        return json(200, { saved: opts.saved });
      }
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return release;
}

function mount() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <Home />
    </QueryClientProvider>,
  );
}

afterEach(() => {
  cleanup();
  replace.mockReset();
  vi.unstubAllGlobals();
});

describe("the root page of an administrator with no organization (P5-UX-1)", () => {
  it("never flashes 'You are not in an organization yet' on the way to Administration", async () => {
    const release = stubApi({ admin: true, saved: false });
    mount();
    // Everything but the placement has answered: the destination is not known yet, so nothing is said about organizations.
    await waitFor(() => expect(screen.queryByText("Loading…")).toBeTruthy());
    expect(screen.queryByText("You are not in an organization yet")).toBeNull();
    release();
    await waitFor(() => expect(replace).toHaveBeenCalledWith("/admin"));
    expect(screen.queryByText("You are not in an organization yet")).toBeNull();
  });

  it("still says it to a person who really has no organization and nowhere to go", async () => {
    stubApi({ admin: false, saved: true })();
    mount();
    expect(await screen.findByText("You are not in an organization yet")).toBeTruthy();
    expect(replace).not.toHaveBeenCalled();
  });
});
