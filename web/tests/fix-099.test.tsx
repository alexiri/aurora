import { QueryClient, QueryClientProvider, useMutation } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops", handle: "scout" }),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
  usePathname: () => "/o/acme/w/ops/agents/scout",
  useSearchParams: () => new URLSearchParams(),
}));

import { MessageGroup } from "@/components/chat/message-group";
import { GettingStarted } from "@/components/onboarding/getting-started";
import { AttachmentCard } from "@/components/chat/attachments";
import { Composer } from "@/components/chat/composer";
import { errorCopy, emptyTranscript, type TranscriptState } from "@/lib/chat/transcript";
import type { TranscriptEvent } from "@/lib/api/conversations";
import { safeNext } from "@/lib/auth/use-unauthorized-redirect";
import { useCreateSecret, useSetSecretValue, useAcceptSecret } from "@/lib/api/secrets";
import { usePublish } from "@/lib/api/policy";
import { InstructionsTab } from "@/components/agents/instructions-tab";
import { useApprovalsStream } from "@/lib/api/approvals";

// FIX-099 (REVIEW-5 WEBA/WEBX/WEBC): dashboard defects of the agent page, chat and onboarding.

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const calls: Request[] = [];
  const fn = vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
    const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
    calls.push(req);
    const h = routes[`${req.method} ${new URL(req.url).pathname}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return { fn, calls };
}
function wrap(ui: React.ReactNode, qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })) {
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}
beforeEach(() => {
  vi.unstubAllGlobals();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const ev = (seq: number, type: string, payload: Record<string, unknown>): TranscriptEvent =>
  ({ seq, type, ts: "2026-11-09T10:00:00Z", payload }) as TranscriptEvent;

describe("chat error blocks (WEBA-7, WEBA-9, WEBX-9, WEBX-5)", () => {
  const sandbox = {
    agent_id: "a1", state: "failed", boot_id: null, runner_id: null, image: null, started_at: null, keep_awake_reasons: [],
    failure_code: "runner_unavailable", failure_message: "no runner", start_attempt: null, retryable: true, start_log: [], processes: [], ports: [],
  };
  const state: TranscriptState = {
    ...emptyTranscript,
    events: [ev(1, "turn.queued", { turn_id: "t1", text: "hello there" }), ev(2, "error", { turn_id: "t1", code: "runner_unavailable", message: "no runner" })],
  };
  const row = {
    kind: "group" as const, key: "g2", side: "agent" as const, senderKey: "agent", name: "", door: "", ts: "2026-11-09T10:00:00Z",
    blocks: [{ kind: "error" as const, ev: state.events[1] }],
  };
  const group = (frozen: boolean) => (
    <MessageGroup row={row} state={state} wsId="w1" base="/o/acme/w/ops" convId="c1" agent={{ id: "a1", name: "Scout", frozen }} />
  );
  const baseRoutes = {
    "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", my_role: "owner" }),
    "GET /v1/agents/a1/sandbox": () => json(200, sandbox),
  };

  it("a frozen agent's chat error has no Retry now", async () => {
    mockFetch(baseRoutes);
    wrap(group(true));
    expect(await screen.findByRole("alert")).toHaveTextContent("No runner is connected");
    expect(screen.queryByRole("button", { name: /retry now/i })).toBeNull();
  });

  it("the re-post after Retry now carries an Idempotency-Key, and a failed re-post is visible", async () => {
    const { calls } = mockFetch({
      ...baseRoutes,
      "POST /v1/agents/a1/sandbox/retry": () => json(202, { ...sandbox, state: "starting", retryable: false }),
      "POST /v1/conversations/c1/messages": () => json(503, { error: { code: "unavailable", message: "Could not reach the queue." } }),
    });
    wrap(group(false));
    fireEvent.click(await screen.findByRole("button", { name: /retry now/i }));
    await waitFor(() => expect(screen.getByText("Could not reach the queue.")).toBeInTheDocument());
    const post = calls.find((c) => c.method === "POST" && c.url.endsWith("/messages"))!;
    expect(post.headers.get("Idempotency-Key")).toMatch(/\S{16,}/);
  });

  it("at_capacity and agent_start_failed have copy; an unknown code keeps the server message out of the main copy", () => {
    expect(errorCopy("at_capacity").message).toMatch(/no room/);
    expect(errorCopy("agent_start_failed").message).toMatch(/failed to start/);
    const unknown = errorCopy("weird_new_code", "internal detail xyz");
    expect(unknown.message).not.toContain("internal detail xyz");
    expect(unknown.showServerMessage).toBe(true);
  });
});

describe("person attachments (WEBX-7)", () => {
  it("names a person's attachment from the stored filename", async () => {
    mockFetch({
      "HEAD /v1/attachments/att1/content": () =>
        new Response(null, { status: 200, headers: { "content-disposition": 'attachment; filename="report-q3.pdf"', "content-length": "2048" } }),
    });
    wrap(<AttachmentCard id="att1" />);
    expect(await screen.findByText("report-q3.pdf")).toBeInTheDocument();
  });
});

describe("composer draft (WEBA-20)", () => {
  it("fills the box from ?draft= once and drops the parameter from the URL", async () => {
    window.history.replaceState(null, "", "/o/acme/w/ops/agents/scout?tab=chat&draft=Say%20hello");
    mockFetch({});
    wrap(<Composer convId="c1" agentId="a1" base="/o/acme/w/ops" agentName="Scout" activeTurn={null} state={emptyTranscript} />);
    expect(await screen.findByDisplayValue("Say hello")).toBeInTheDocument();
    await waitFor(() => expect(window.location.search).toBe("?tab=chat"));
    cleanup();
    wrap(<Composer convId="c1" agentId="a1" base="/o/acme/w/ops" agentName="Scout" activeTurn={null} state={emptyTranscript} />);
    expect(screen.queryByDisplayValue("Say hello")).toBeNull();
  });
});

describe("onboarding guide failures (WEBA-6, WEBA-8)", () => {
  it("a failed readiness query shows an error with a retry, not 'Checking…' forever", async () => {
    mockFetch({
      "GET /v1/workspaces/w1/agents": () => json(200, { items: [{ id: "a1", name: "Scout", handle: "scout", is_default: true, kind: "hosted" }], next_cursor: null }),
      "GET /v1/agents/a1/model-readiness": () => json(500, { error: { code: "internal", message: "readiness exploded" } }),
      "GET /v1/workspaces/w1/secrets": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/orgs/o1/secrets": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/me/onboarding": () => json(200, { guide_dismissed: [] }),
    });
    wrap(<GettingStarted org="acme" ws="ops" wsId="w1" orgId="o1" isOwner variant="welcome" />);
    expect(await screen.findByText(/readiness exploded|went wrong|internal/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /try again/i })).toBeInTheDocument();
    expect(screen.queryByText(/Checking what/)).toBeNull();
  });
});

describe("agent page (WEBA-1 effort, WEBX-14 access)", () => {
  it("choosing Default effort sends effort: null", async () => {
    const { calls } = mockFetch({
      "PATCH /v1/workspaces/w1/agents/a1": () => json(200, {}),
      "PATCH /v1/agents/a1": () => json(200, {}),
    });
    const agent = {
      id: "a1", handle: "scout", name: "Scout", kind: "byo", brief: "b", effort: "high", proactive_messaging: "ask",
      is_default: false, avatar_url: null, frozen: null, updated_at: "2026-01-01T00:00:00Z",
    };
    wrap(<InstructionsTab agent={agent as never} wsId="w1" />);
    fireEvent.change(await screen.findByLabelText("Effort"), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    await waitFor(() => expect(calls.some((c) => c.method === "PATCH")).toBe(true));
    const patch = calls.find((c) => c.method === "PATCH")!;
    expect(await patch.json()).toMatchObject({ effort: null });
  });
});

describe("approvals stream resync (WEBC-1)", () => {
  it("a resync frame refetches the approvals", async () => {
    const listeners: Record<string, (e: unknown) => void> = {};
    class FakeES {
      static OPEN = 1;
      static CLOSED = 2;
      readyState = 1;
      onopen: (() => void) | null = null;
      onerror: (() => void) | null = null;
      addEventListener(t: string, f: (e: unknown) => void) {
        listeners[t] = f;
      }
      close() {}
    }
    vi.stubGlobal("EventSource", FakeES);
    const qc = new QueryClient();
    qc.setQueryData(["approvals", "w1", "pending"], { items: [] });
    const spy = vi.spyOn(qc, "invalidateQueries");
    renderHook(() => useApprovalsStream("w1"), { wrapper: ({ children }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider> });
    act(() => listeners.resync?.({}));
    expect(listeners.resync).toBeDefined();
    expect(spy).toHaveBeenCalled();
  });
});

describe("safeNext (WEBC-5)", () => {
  it("refuses control characters", () => {
    expect(safeNext("/o/acme\r\nSet-Cookie: x")).toBe("/");
    expect(safeNext("/a\u0000b")).toBe("/");
    expect(safeNext("/o/acme/w/ops")).toBe("/o/acme/w/ops");
  });
});

describe("secret mutations leave no plaintext in the mutation cache (WEBC-4)", () => {
  it.each([
    ["create", () => useCreateSecret({ kind: "workspace", id: "w1" }), { name: "k", kind: "generic", value: "s3cret-plaintext" }],
    ["set value", () => useSetSecretValue({ kind: "workspace", id: "w1" }, "s1"), { value: "s3cret-plaintext" }],
    ["accept", () => useAcceptSecret({ kind: "workspace", id: "w1" }, "s1"), "s3cret-plaintext"],
  ])("%s", async (_n, hook, vars) => {
    mockFetch({
      "POST /v1/workspaces/w1/secrets": () => json(201, { id: "s1" }),
      "PUT /v1/secrets/s1/value": () => json(200, { id: "s1" }),
      "POST /v1/secrets/s1/accept": () => json(200, { id: "s1" }),
    });
    const qc = new QueryClient();
    const { result, unmount } = renderHook(hook as () => ReturnType<typeof useMutation<unknown, Error, never>>, {
      wrapper: ({ children }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>,
    });
    await act(async () => {
      await result.current.mutateAsync(vars as never).catch(() => undefined);
    });
    // `gcTime: 0`: once the form that ran it is gone, the mutation (and its `variables`) is dropped, not kept for five minutes.
    unmount();
    await waitFor(() => expect(qc.getMutationCache().getAll()).toHaveLength(0));
  });
});

describe("policy changes refresh the agents' effective and grants views (WEBC-6)", () => {
  it("publish invalidates effective and grants", async () => {
    mockFetch({ "POST /v1/workspaces/w1/policy/publish": () => json(200, {}) });
    const qc = new QueryClient();
    qc.setQueryData(["effective", "a1"], { stale: true });
    qc.setQueryData(["grants", "a1"], { stale: true });
    const { result } = renderHook(() => usePublish({ kind: "workspace", id: "w1" }), {
      wrapper: ({ children }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>,
    });
    await act(async () => {
      await result.current.mutateAsync(3).catch(() => undefined);
    });
    expect(qc.getQueryState(["effective", "a1"])?.isInvalidated).toBe(true);
    expect(qc.getQueryState(["grants", "a1"])?.isInvalidated).toBe(true);
  });
});
