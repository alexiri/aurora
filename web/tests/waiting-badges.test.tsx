import { QueryClient, QueryClientProvider, focusManager } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useWaitingCount, WorkspaceNav } from "@/components/app-shell";
import { json } from "./support/api";
import { seedMoreControls } from "./support/render";

// P5-REFINE-16 item 19 (G1, G2, G6): the workspace badges are one number from C-1082 (`GET /v1/attention/counts`), read by one
// shared hook: Approvals shows its topic's part, Overview the whole, the phone's menu dot the same whole. Words for a screen
// reader inside the link, no badge at zero, none for someone who cannot act (the scope comes back null), read again on focus.

vi.mock("next/navigation", () => ({ useParams: () => ({}), usePathname: () => "/o/acme/w/main", useRouter: () => ({ push: vi.fn(), replace: vi.fn() }), useSearchParams: () => new URLSearchParams() }));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

type Counts = { count: number; approvals: number } | null;

function stub(role: "owner" | "member", counts: () => Counts) {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      calls.push(url.pathname + url.search);
      if (url.pathname === "/v1/workspaces/w1") return json(200, { id: "w1", org_id: "o1", slug: "main", name: "Main", my_role: role });
      if (url.pathname === "/v1/attention/counts") {
        const c = counts();
        return json(200, { workspace: c ? { workspace_id: "w1", count: c.count, topics: c.approvals ? { approvals: c.approvals } : {} } : null });
      }
      if (url.pathname === "/v1/me") return json(200, { user: { id: "u", is_instance_admin: false }, memberships: [] });
      return json(200, { items: [], next_cursor: null });
    }),
  );
  return calls;
}

const client = () => {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  seedMoreControls(qc, true);
  return qc;
};

function nav() {
  return render(
    <QueryClientProvider client={client()}>
      <WorkspaceNav base="/o/acme/w/main" wsId="w1" pathname="/o/acme/w/main" />
    </QueryClientProvider>,
  );
}

function Dot({ wsId }: { wsId: string }) {
  return <span data-testid="menu-count">{useWaitingCount(wsId)}</span>;
}

describe("Workspace count badges", () => {
  it("Approvals shows its own part and Overview the whole, with screen-reader words inside the links", async () => {
    stub("owner", () => ({ count: 3, approvals: 2 }));
    nav();
    const approvals = await screen.findByTestId("approvals-badge");
    expect(approvals).toHaveTextContent("2 need your attention");
    expect(approvals.closest("a")).toHaveAttribute("href", "/o/acme/w/main/approvals");
    const overview = await screen.findByTestId("overview-badge");
    expect(overview).toHaveTextContent("3 need your attention");
    expect(overview.closest("a")).toHaveAttribute("href", "/o/acme/w/main");
  });

  it("asks once for both badges, with the workspace in the query", async () => {
    const calls = stub("owner", () => ({ count: 3, approvals: 2 }));
    nav();
    await screen.findByTestId("overview-badge");
    expect(calls.filter((c) => c.startsWith("/v1/attention/counts"))).toEqual(["/v1/attention/counts?workspace=w1"]);
  });

  it("has no badge at zero", async () => {
    const calls = stub("owner", () => ({ count: 0, approvals: 0 }));
    nav();
    await waitFor(() => expect(calls.some((c) => c.startsWith("/v1/attention/counts"))).toBe(true));
    expect(screen.queryByTestId("approvals-badge")).toBeNull();
    expect(screen.queryByTestId("overview-badge")).toBeNull();
  });

  it("has no badge for someone who cannot act: the scope is null, and a member has no Approvals entry", async () => {
    const calls = stub("member", () => null);
    nav();
    await waitFor(() => expect(calls.some((c) => c.startsWith("/v1/attention/counts"))).toBe(true));
    await screen.findByRole("link", { name: /Activity/ });
    expect(screen.queryByRole("link", { name: /Approvals/ })).toBeNull();
    expect(screen.queryByTestId("approvals-badge")).toBeNull();
    expect(screen.queryByTestId("overview-badge")).toBeNull();
  });

  it("drops when what waits is decided, read again on tab focus and every 30 s", async () => {
    let c: Counts = { count: 2, approvals: 2 };
    stub("owner", () => c);
    const spy = vi.spyOn(globalThis, "setInterval");
    nav();
    expect(await screen.findByTestId("approvals-badge")).toHaveTextContent("2");
    expect(spy.mock.calls.some((call) => call[1] === 30_000)).toBe(true);
    c = { count: 1, approvals: 1 };
    act(() => {
      focusManager.setFocused(false);
      focusManager.setFocused(true);
    });
    await waitFor(() => expect(screen.getByTestId("approvals-badge")).toHaveTextContent("1 needs your attention"));
    expect(screen.getByTestId("overview-badge")).toHaveTextContent("1 needs your attention");
    c = { count: 0, approvals: 0 };
    act(() => {
      focusManager.setFocused(false);
      focusManager.setFocused(true);
    });
    await waitFor(() => expect(screen.queryByTestId("approvals-badge")).toBeNull());
    expect(screen.queryByTestId("overview-badge")).toBeNull();
  });

  it("the phone's menu dot reads the same whole number (G6)", async () => {
    stub("owner", () => ({ count: 3, approvals: 2 }));
    render(
      <QueryClientProvider client={client()}>
        <Dot wsId="w1" />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("menu-count")).toHaveTextContent("3"));
  });
});
