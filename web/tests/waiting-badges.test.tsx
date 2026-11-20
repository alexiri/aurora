import { QueryClient, QueryClientProvider, focusManager } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { WorkspaceNav } from "@/components/app-shell";
import { json } from "./support/api";
import { seedMoreControls } from "./support/render";

// P5-REFINE-16 item 19 (G1, G6): the Approvals badge is the number that waits for a person who can decide, in words for a screen
// reader, absent at zero, absent for a member, and read again on its own (no stream outside the Approvals page).

vi.mock("next/navigation", () => ({ useParams: () => ({}), usePathname: () => "/o/acme/w/main", useRouter: () => ({ push: vi.fn(), replace: vi.fn() }), useSearchParams: () => new URLSearchParams() }));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

function stub(role: "owner" | "member", pending: () => number) {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      calls.push(url.pathname);
      if (url.pathname === "/v1/workspaces/w1") return json(200, { id: "w1", org_id: "o1", slug: "main", name: "Main", my_role: role });
      if (url.pathname === "/v1/workspaces/w1/approvals") return json(200, { items: Array.from({ length: pending() }, (_, i) => ({ id: `a${i}`, created_at: `2026-01-01T00:00:0${i}Z` })), next_cursor: null });
      if (url.pathname === "/v1/workspaces/w1/action-approvals") return json(200, { items: [], next_cursor: null });
      if (url.pathname === "/v1/me") return json(200, { user: { id: "u", is_instance_admin: false }, memberships: [] });
      return json(200, { items: [], next_cursor: null });
    }),
  );
  return calls;
}

function nav() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  seedMoreControls(qc, true);
  return render(
    <QueryClientProvider client={qc}>
      <WorkspaceNav base="/o/acme/w/main" wsId="w1" pathname="/o/acme/w/main" />
    </QueryClientProvider>,
  );
}

describe("Approvals badge", () => {
  it("shows the count with screen-reader words inside the link, for someone who can decide", async () => {
    stub("owner", () => 2);
    nav();
    const badge = await screen.findByTestId("approvals-badge");
    expect(badge).toHaveTextContent("2");
    expect(badge).toHaveTextContent("2 need your attention");
    expect(badge.closest("a")).toHaveAttribute("href", "/o/acme/w/main/approvals");
  });

  it("is absent at zero", async () => {
    const calls = stub("owner", () => 0);
    nav();
    await waitFor(() => expect(calls).toContain("/v1/workspaces/w1/approvals"));
    expect(screen.queryByTestId("approvals-badge")).toBeNull();
  });

  it("is absent for a member, who cannot decide, and never asks for the list", async () => {
    const calls = stub("member", () => 3);
    nav();
    await waitFor(() => expect(calls).toContain("/v1/workspaces/w1"));
    await screen.findByRole("link", { name: /Activity/ });
    // A member has no Approvals entry at all.
    expect(screen.queryByRole("link", { name: /Approvals/ })).toBeNull();
    expect(screen.queryByTestId("approvals-badge")).toBeNull();
    expect(calls).not.toContain("/v1/workspaces/w1/approvals");
  });

  it("drops when what waits is decided, read again on its own", async () => {
    let n = 2;
    stub("owner", () => n);
    const spy = vi.spyOn(globalThis, "setInterval");
    nav();
    expect(await screen.findByTestId("approvals-badge")).toHaveTextContent("2");
    // The polling interval is registered (30 s); a tab-focus refetch reads the new count.
    expect(spy.mock.calls.some((c) => c[1] === 30_000)).toBe(true);
    n = 1;
    act(() => {
      focusManager.setFocused(false);
      focusManager.setFocused(true);
    });
    await waitFor(() => expect(screen.getByTestId("approvals-badge")).toHaveTextContent("1 needs your attention"));
  });
});
