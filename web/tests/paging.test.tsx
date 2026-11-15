import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, renderHook, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ChatTab } from "@/components/agents/chat-tab";
import { useMyApiKeys, useAccess, useOrgMembers, useWorkspaceKeys } from "@/lib/api/account";
import { usePendingActionApprovals } from "@/lib/api/action-approvals";
import { useOrgAttentionTokens } from "@/lib/api/agent-tokens";
import { useConnections } from "@/lib/api/apps";
import { usePendingApprovals } from "@/lib/api/approvals";
import { useConversations } from "@/lib/api/conversations";
import { useAdminOrgs } from "@/lib/api/freeze";
import { useBlockedHosts } from "@/lib/api/governance";
import { useInvitations, useOrgKeys } from "@/lib/api/org";
import { fetchAllPages, MAX_PAGES } from "@/lib/api/paging";
import { useWorkspaces } from "@/lib/api/queries";

// FIX-117: every list hook follows `next_cursor`; a two-page mock must yield both pages' rows.

vi.mock("@/lib/chat/use-transcript", () => ({
  useTranscript: () => ({ state: { events: [], deltas: {}, activity: {}, lastSeq: 0 }, loading: false, error: null, live: true }),
}));

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/** Serves `path` as two pages: no cursor → page 1 (`next_cursor: "c2"`), `cursor=c2` → page 2. Records every URL. */
function twoPages(path: string, mk: (page: 1 | 2) => Record<string, unknown>) {
  const urls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(req.url, "http://localhost");
      urls.push(`${url.pathname}${url.search}`);
      if (url.pathname !== path) return json(404, { error: { code: "not_found", message: "nf" } });
      const second = url.searchParams.get("cursor") === "c2" || url.searchParams.get("derived_cursor") === "c2";
      return json(200, mk(second ? 2 : 1));
    }),
  );
  return urls;
}

function wrapper() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  }
  return Wrapper;
}

/** `n` rows with ids `<prefix><page>-<i>` and the cursor of page 1. */
const rows = (page: 1 | 2, extra: (id: string) => Record<string, unknown>, prefix = "r") => ({
  items: [0, 1].map((i) => extra(`${prefix}${page}-${i}`)),
  next_cursor: page === 1 ? "c2" : null,
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.history.replaceState(null, "", "/");
});

describe("fetchAllPages", () => {
  it("follows next_cursor, keeps the first envelope and reports the ceiling", async () => {
    const chain: Record<string, string> = { first: "n1", n1: "n2", n2: "n3" };
    const page = vi.fn(async (cursor: string | undefined) => ({
      items: [cursor ?? "first"],
      next_cursor: chain[cursor ?? "first"],
      extra: "kept",
    }));
    const all = await fetchAllPages(page, 3);
    expect(all.items).toEqual(["first", "n1", "n2"]);
    expect(all.extra).toBe("kept");
    expect(all.truncated).toBe(true);
    expect(page).toHaveBeenCalledTimes(3);
    expect(MAX_PAGES).toBeGreaterThan(3);
  });

  it("is not truncated when the list ends before the ceiling", async () => {
    const all = await fetchAllPages(async (c) => ({ items: [c ?? "a"], next_cursor: c ? null : "b" }));
    expect(all).toMatchObject({ items: ["a", "b"], truncated: false, next_cursor: null });
  });
});

describe("list hooks read every page", () => {
  it("useWorkspaces", async () => {
    twoPages("/v1/orgs/o1/workspaces", (p) => rows(p, (id) => ({ id, name: id, slug: id })));
    const { result } = renderHook(() => useWorkspaces("o1"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.data?.items).toHaveLength(4));
  });

  it("useOrgMembers", async () => {
    twoPages("/v1/orgs/o1/members", (p) => rows(p, (id) => ({ user_id: id, email: `${id}@x.io`, display_name: id })));
    const { result } = renderHook(() => useOrgMembers("o1"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.data?.items).toHaveLength(4));
  });

  it("useAccess follows both cursor and derived_cursor", async () => {
    const urls = twoPages("/v1/workspaces/w1/access", (p) => ({
      items: rows(p, (id) => ({ user_id: id })).items,
      next_cursor: null,
      group_bindings: p === 1 ? [{ group: { id: "g" }, role: "member" }] : undefined,
      derived_members: {
        items: [{ user: { id: `d${p}` }, role: "member", sources: [] }],
        next_cursor: p === 1 ? "c2" : null,
      },
    }));
    const { result } = renderHook(() => useAccess("w1"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.data?.derived_members?.items).toHaveLength(2));
    expect(result.current.data?.group_bindings).toHaveLength(1);
    expect(urls.some((u) => u.includes("derived_cursor=c2"))).toBe(true);
  });

  it("useMyApiKeys", async () => {
    twoPages("/v1/me/api-keys", (p) => rows(p, (id) => ({ id })));
    const { result } = renderHook(() => useMyApiKeys(), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.data?.items).toHaveLength(4));
  });

  it("useWorkspaceKeys", async () => {
    twoPages("/v1/workspaces/w1/api-keys", (p) => rows(p, (id) => ({ id })));
    const { result } = renderHook(() => useWorkspaceKeys("w1"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.data?.items).toHaveLength(4));
  });

  it("useInvitations", async () => {
    twoPages("/v1/orgs/o1/invitations", (p) => rows(p, (id) => ({ id })));
    const { result } = renderHook(() => useInvitations("o1"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.data?.items).toHaveLength(4));
  });

  it("useOrgKeys", async () => {
    twoPages("/v1/orgs/o1/api-keys", (p) => rows(p, (id) => ({ id })));
    const { result } = renderHook(() => useOrgKeys("o1"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.data?.items).toHaveLength(4));
  });

  it("useConnections (org and workspace)", async () => {
    twoPages("/v1/orgs/o1/connections", (p) => rows(p, (id) => ({ id })));
    const org = renderHook(() => useConnections({ kind: "organization", id: "o1" }), { wrapper: wrapper() });
    await waitFor(() => expect(org.result.current.data?.items).toHaveLength(4));
    twoPages("/v1/workspaces/w1/connections", (p) => rows(p, (id) => ({ id })));
    const ws = renderHook(() => useConnections({ kind: "workspace", id: "w1" }), { wrapper: wrapper() });
    await waitFor(() => expect(ws.result.current.data?.items).toHaveLength(4));
  });

  it("useAdminOrgs", async () => {
    twoPages("/v1/admin/orgs", (p) => rows(p, (id) => ({ id, name: id, slug: id })));
    const { result } = renderHook(() => useAdminOrgs(false), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.data?.items).toHaveLength(4));
  });

  it("usePendingApprovals: the second page's approvals are listed (and so decidable)", async () => {
    twoPages("/v1/workspaces/w1/approvals", (p) =>
      rows(p, (id) => ({ id, status: "pending", created_at: `2026-11-1${p}T10:00:0${id.endsWith("0") ? 0 : 1}Z` })),
    );
    const { result } = renderHook(() => usePendingApprovals("w1"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.data?.items.map((a) => a.id)).toEqual(["r1-0", "r1-1", "r2-0", "r2-1"]));
    expect(result.current.data?.truncated).toBe(false);
  });

  it("usePendingActionApprovals", async () => {
    twoPages("/v1/workspaces/w1/action-approvals", (p) => rows(p, (id) => ({ id })));
    const { result } = renderHook(() => usePendingActionApprovals("w1"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.data?.items).toHaveLength(4));
  });

  it("useOrgAttentionTokens merges both lists across their pages", async () => {
    twoPages("/v1/orgs/o1/agent-tokens", (p) =>
      rows(p, (id) => ({ agent: { id, handle: id, name: id }, expires_at: null }), "t"),
    );
    const { result } = renderHook(() => useOrgAttentionTokens("o1", true), { wrapper: wrapper() });
    // stale and expiring serve the same mock, so the merge dedupes by agent: 4 agents
    await waitFor(() => expect(result.current.data).toHaveLength(4));
  });

  it("useConversations", async () => {
    twoPages("/v1/agents/a1/conversations", (p) => rows(p, (id) => ({ id })));
    const { result } = renderHook(() => useConversations("a1"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.data?.items).toHaveLength(4));
  });

  it("useBlockedHosts counts the hosts of every workspace (no cap of 10) and every page", async () => {
    const seen = new Set<string>();
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
        const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
        const url = new URL(req.url, "http://localhost");
        const m = /^\/v1\/workspaces\/(w\d+)\/activity$/.exec(url.pathname);
        if (!m) return json(404, { error: { code: "not_found", message: "nf" } });
        seen.add(m[1]!);
        const second = url.searchParams.get("cursor") === "c2";
        return json(200, {
          items: [{ host: second ? "late.example" : "early.example" }],
          next_cursor: second ? null : "c2",
        });
      }),
    );
    const ids = Array.from({ length: 12 }, (_, i) => `w${i}`);
    const { result } = renderHook(() => useBlockedHosts(ids, true), { wrapper: wrapper() });
    await waitFor(() => expect(result.current).toHaveLength(2));
    expect(seen.size).toBe(12);
    expect(result.current.find((h) => h.host === "late.example")?.count).toBe(12);
  });
});

describe("a deep link to a conversation past the loaded pages", () => {
  it("is fetched by id and shows its title", async () => {
    window.history.replaceState(null, "", "/?c=c51");
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
        const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
        const url = new URL(req.url, "http://localhost");
        const conv = (id: string, over: Record<string, unknown> = {}) => ({
          id, agent_id: "a1", kind: "group", external_ref: id, title: null, direct_user_id: null, direct_user: null,
          origin_conversation_id: null, last_seq: 0, app_turn_streak: 0, app_turn_capped: false, active_turn: null,
          last_activity_at: null, created_at: "2026-01-01T00:00:00Z", ...over,
        });
        if (req.method === "POST" && url.pathname === "/v1/agents/a1/conversations/direct") return json(200, conv("mine", { kind: "direct" }));
        if (url.pathname === "/v1/agents/a1/conversations") return json(200, { items: [conv("c1", { title: "Newest" })], next_cursor: null });
        if (url.pathname === "/v1/conversations/c51") return json(200, conv("c51", { title: "Old planning thread" }));
        return json(404, { error: { code: "not_found", message: "nf" } });
      }),
    );
    render(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" />, { wrapper: wrapper() });
    expect(await screen.findByRole("heading", { name: "Old planning thread" })).toBeTruthy();
  });
});
