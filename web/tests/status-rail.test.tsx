import { act, fireEvent, renderHook, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { StatusRail, StatusSheet, stripLabels, type RailContext } from "@/components/agents/status-rail/status-rail";
import {
  initialExpanded,
  RAIL_STORAGE_KEY,
  readRailPreference,
  useRailExpanded,
  writeRailPreference,
} from "@/components/agents/status-rail/rail-state";
import { decisionBadge, hasProblem, mergeNeeds, sandboxLook, type AgentStatus } from "@/components/agents/status-rail/status-data";
import type { ActionApproval } from "@/lib/api/action-approvals";
import type { Approval } from "@/lib/api/approvals";
import { wrap } from "./support/render";

// D-201: the status rail: parts, indicators with accessible names, persistence (storage may throw), approver gating,
// the Needs-you pop-over, the overlay mode.

const gw = (id: string, deadline: string, over: Partial<Approval> = {}): Approval =>
  ({
    id, org_id: "o", workspace_id: "w", status: "pending", method: "POST", host: "api.github.com", path: "/repos/x/y/issues",
    created_at: "2026-11-14T10:00:00Z", deadline, conversation_id: "c1", summary: { source: "generic", title: "Create an issue", fields: [] },
    ...over,
  }) as Approval;
const act1 = (id: string, expires_at: string, over: Partial<ActionApproval> = {}): ActionApproval =>
  ({
    id, agent_id: "a1", workspace_id: "w", status: "pending", action_key: "slack.post_message", summary: "Post to #ops",
    conversation_hidden: false, conversation_id: "c2", created_at: "2026-11-14T10:00:00Z", expires_at, ...over,
  }) as ActionApproval;

const FAR = "2099-01-01T00:00:00Z";
const status = (over: Partial<AgentStatus> = {}): AgentStatus => ({
  canApprove: true,
  needs: [],
  needsLoading: false,
  working: [],
  recent: [],
  recentLoading: false,
  recentError: false,
  problem: false,
  sandbox: undefined,
  ...over,
});
const ctxOf = (over: Partial<RailContext> = {}, s: Partial<AgentStatus> = {}): RailContext => ({
  status: status(s),
  open: { active: false, line: null },
  wsId: "w",
  base: "/o/x/w/y",
  agentId: "a1",
  agent: { kind: "hosted", isDefault: true, brief: "Triage pull requests." },
  terminalAvailable: true,
  openConversation: { id: "c1", title: "Direct chat" },
  titleOf: (c) => c.title ?? "Untitled",
  onSelectConversation: vi.fn(),
  onNavigate: vi.fn(),
  ...over,
});
const needs = (...n: Parameters<typeof mergeNeeds>[0]) => mergeNeeds(n, [], (id) => (id === "c1" ? "Direct chat" : null));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
interface Call { method: string; path: string; body: unknown }
function mock(routes: Record<string, () => Response> = {}) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" ? "" : await req.clone().text();
      calls.push({ method: req.method, path: url.pathname, body: text ? JSON.parse(text) : undefined });
      const h = routes[`${req.method} ${url.pathname}`];
      return h ? h() : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return calls;
}
/** A rail with its own expanded state, as the chat panel holds it. */
function Harness({ ctx, start = false, overlay = false, onChange }: { ctx: RailContext; start?: boolean; overlay?: boolean; onChange?: (e: boolean) => void }) {
  const [expanded, setExpanded] = useState(start);
  return (
    <StatusRail
      ctx={ctx}
      expanded={expanded}
      onExpandedChange={(e) => {
        setExpanded(e);
        onChange?.(e);
      }}
      overlay={overlay}
    />
  );
}

beforeEach(() => window.localStorage.clear());
afterEach(() => {
  vi.restoreAllMocks();
});

describe("rail persistence (per browser, never sent to the API)", () => {
  it("starts expanded from 1440 px and collapsed below when nothing is stored", () => {
    expect(initialExpanded(null, 1440)).toBe(true);
    expect(initialExpanded(null, 1920)).toBe(true);
    expect(initialExpanded(null, 1439)).toBe(false);
    expect(initialExpanded(null, 1280)).toBe(false);
  });
  it("a stored choice wins over the viewport", () => {
    expect(initialExpanded("collapsed", 1920)).toBe(false);
    expect(initialExpanded("expanded", 1280)).toBe(true);
  });
  it("remembers the choice across mounts and sends nothing to the API", () => {
    const calls = mock();
    const first = renderHook(() => useRailExpanded());
    expect(first.result.current[0]).toBe(window.innerWidth >= 1440);
    act(() => first.result.current[1](true));
    expect(window.localStorage.getItem(RAIL_STORAGE_KEY)).toBe("expanded");
    first.unmount();
    expect(renderHook(() => useRailExpanded()).result.current[0]).toBe(true);
    const second = renderHook(() => useRailExpanded());
    act(() => second.result.current[1](false));
    expect(readRailPreference()).toBe("collapsed");
    expect(calls).toEqual([]);
  });
  it("works with storage that throws (the default applies, writes are dropped)", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(readRailPreference()).toBeNull();
    expect(() => writeRailPreference("expanded")).not.toThrow();
    const { result } = renderHook(() => useRailExpanded());
    expect(result.current[0]).toBe(window.innerWidth >= 1440);
    act(() => result.current[1](true));
    expect(result.current[0]).toBe(true);
  });
  it("ignores a garbage stored value", () => {
    window.localStorage.setItem(RAIL_STORAGE_KEY, "sideways");
    expect(readRailPreference()).toBeNull();
  });
});

describe("rail data helpers", () => {
  it("merges gateway holds and action approvals, soonest expiry first", () => {
    const items = mergeNeeds(
      [gw("g1", "2099-01-01T00:10:00Z"), gw("g2", "2099-01-01T00:02:00Z", { conversation_id: null })],
      [act1("x1", "2099-01-01T00:05:00Z")],
      (id) => (id === "c1" ? "Direct chat" : null),
    );
    expect(items.map((i) => i.key)).toEqual(["g:g2", "a:x1", "g:g1"]);
    expect(items[2]).toMatchObject({ title: "Direct chat", summary: "Create an issue", requestLine: "POST api.github.com/repos/x/y/issues" });
    expect(items[1]).toMatchObject({ kind: "action", requestLine: "slack.post_message", summary: "Post to #ops" });
    expect(items[0]!.title).toBeNull();
  });
  it("labels request-log rows: an error_code reads Failed, a denial Deny", () => {
    expect(decisionBadge({ decision: "allow", error_code: null })).toEqual({ label: "Allow", tone: "accent" });
    expect(decisionBadge({ decision: "approved", error_code: null }).label).toBe("Allow");
    expect(decisionBadge({ decision: "approval_expired", error_code: null })).toEqual({ label: "Held", tone: "pending" });
    expect(decisionBadge({ decision: "blocked", error_code: null }).label).toBe("Deny");
    expect(decisionBadge({ decision: "default_denied", error_code: null }).label).toBe("Deny");
    expect(decisionBadge({ decision: "allow", error_code: "upstream_timeout" })).toEqual({ label: "Failed", tone: "danger" });
    expect(hasProblem([{ decision: "allow", error_code: null }])).toBe(false);
    expect(hasProblem([{ decision: "allow", error_code: null }, { decision: "blocked", error_code: null }])).toBe(true);
  });
  it("shares the sandbox labels and failure tooltip with the header pill", () => {
    expect(sandboxLook({ state: "starting", failure_message: null, failure_code: null })).toEqual({ tone: "pending", label: "Sandbox starting", title: undefined });
    expect(sandboxLook({ state: "failed", failure_message: "no runner", failure_code: "pod_failed" })).toMatchObject({
      tone: "danger", label: "Sandbox failed", title: "no runner (pod_failed)",
    });
  });
});

describe("collapsed strip", () => {
  it("shows the five icons in order, each named with its state in words", () => {
    mock();
    wrap(
      <Harness
        ctx={ctxOf(
          {},
          {
            needs: needs(gw("g1", FAR), gw("g2", FAR)),
            working: [{ id: "c1" } as never],
            problem: true,
            sandbox: { state: "running", failure_message: null, failure_code: null } as never,
          },
        )}
      />,
    );
    const strip = screen.getByRole("complementary", { name: "Status" });
    const names = within(strip).getAllByRole("button").map((b) => b.getAttribute("aria-label"));
    expect(names).toEqual([
      "Expand status panel",
      "Needs you: 2 approvals waiting",
      "Doing now: working",
      "Recent tool calls: some were denied or failed",
      "Computer: Sandbox running",
      "About this agent",
    ]);
    // The count on the Needs you icon.
    expect(within(screen.getByRole("button", { name: /Needs you/ })).getByText("2")).toBeInTheDocument();
  });
  it("says idle / nothing waiting / none failed when quiet", () => {
    mock();
    const labels = stripLabels(ctxOf());
    expect(labels).toMatchObject({
      needs: "Needs you: nothing waiting",
      doing: "Doing now: idle",
      recent: "Recent tool calls: none denied or failed",
      computer: "Computer: state unknown",
    });
    expect(stripLabels(ctxOf({}, { needs: needs(gw("g1", FAR)) })).needs).toBe("Needs you: 1 approval waiting");
    expect(stripLabels(ctxOf({ open: { active: true, line: "Reading the repo" } })).doing).toBe("Doing now: working");
  });
  it("non-approvers get no Needs you icon and no Needs you part", () => {
    mock();
    wrap(<Harness ctx={ctxOf({}, { canApprove: false })} />);
    expect(screen.queryByRole("button", { name: /Needs you/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Expand status panel" }));
    expect(screen.getByRole("heading", { name: "Doing now" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: /Needs you/ })).toBeNull();
    expect(screen.queryByText("Nothing is waiting for you.")).toBeNull();
  });
  it("the other icons expand the rail", () => {
    mock();
    const onChange = vi.fn();
    wrap(<Harness ctx={ctxOf()} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^Computer:/ }));
    expect(onChange).toHaveBeenCalledWith(true);
    expect(screen.getByRole("button", { name: "Collapse status panel" })).toBeInTheDocument();
  });
});

describe("Needs you pop-over", () => {
  it("opens from the icon with the pending items, closes on Esc and returns focus", async () => {
    mock();
    wrap(<Harness ctx={ctxOf({}, { needs: needs(gw("g1", FAR)) })} />);
    const icon = screen.getByRole("button", { name: "Needs you: 1 approval waiting" });
    icon.focus();
    fireEvent.click(icon);
    const pop = await screen.findByRole("dialog", { name: "Needs you" });
    expect(within(pop).getByText("Create an issue")).toBeInTheDocument();
    expect(within(pop).getByText("POST api.github.com/repos/x/y/issues")).toBeInTheDocument();
    expect(within(pop).getByRole("button", { name: "Open the status panel" })).toBeInTheDocument();
    fireEvent.keyDown(pop, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Needs you" })).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(icon));
  });
  it("Open the status panel expands the rail", async () => {
    mock();
    wrap(<Harness ctx={ctxOf({}, { needs: needs(gw("g1", FAR)) })} />);
    fireEvent.click(screen.getByRole("button", { name: /^Needs you/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Open the status panel" }));
    expect(await screen.findByRole("heading", { name: "Status" })).toBeInTheDocument();
  });
});

describe("expanded rail", () => {
  it("shows the five parts in order with their data", () => {
    mock();
    const open = { active: true, line: "Running gh pr list" };
    wrap(
      <Harness
        start
        ctx={ctxOf(
          { open, openConversation: { id: "c1", title: "Direct chat" } },
          {
            needs: needs(gw("g1", FAR)),
            working: [{ id: "c1", title: "Direct chat", kind: "direct", external_ref: "c1", direct_user: null, active_turn: { id: "t1" } } as never],
            recent: [
              { id: "r1", decision: "allow", error_code: null, tool_id: "github.list_prs", method: "GET", host: "api.github.com", latency_ms: 120 },
              { id: "r2", decision: "blocked", error_code: null, tool_id: null, method: "POST", host: "evil.test", latency_ms: 2400 },
            ] as never,
            problem: true,
            sandbox: { state: "stopped", failure_message: null, failure_code: null } as never,
          },
        )}
      />,
    );
    const heads = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(heads).toEqual(["Needs you1", "Doing now", "Recent tool calls", "Computer", "About"]);
    // Doing now: the open conversation's running tool from its stream.
    expect(screen.getByText("Direct chat · Running gh pr list")).toBeInTheDocument();
    // Recent tool calls: badge, tool id or method + host, latency, link.
    expect(screen.getByText("Allow")).toBeInTheDocument();
    expect(screen.getByText("github.list_prs")).toBeInTheDocument();
    expect(screen.getByText("Deny")).toBeInTheDocument();
    expect(screen.getByText("POST evil.test")).toBeInTheDocument();
    expect(screen.getByText("2.4 s")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "All activity" }).getAttribute("href")).toBe("/o/x/w/y/activity?agent=a1");
    // Computer and About.
    expect(screen.getByText("Sandbox stopped")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open computer" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Terminal" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Files" })).toBeInTheDocument();
    expect(screen.getByText("Default")).toBeInTheDocument();
    expect(screen.getByText("Triage pull requests.")).toBeInTheDocument();
  });
  it("says Idle when nothing runs and offers no Terminal where the page is unavailable", () => {
    mock();
    wrap(<Harness start ctx={ctxOf({ terminalAvailable: false })} />);
    expect(screen.getByText("Idle")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Terminal" })).toBeNull();
    // (P5-FILES-2) Files follows the Terminal's availability.
    expect(screen.queryByRole("button", { name: "Files" })).toBeNull();
    expect(screen.getByText("Nothing is waiting for you.")).toBeInTheDocument();
  });
  it("navigates: Open computer, Terminal and Edit instructions, and selects a working conversation", () => {
    mock();
    const ctx = ctxOf({}, { working: [{ id: "c9", title: "Nightly report", kind: "schedule", external_ref: "x", direct_user: null, active_turn: { id: "t" } } as never] });
    wrap(<Harness start ctx={ctx} />);
    fireEvent.click(screen.getByRole("button", { name: "Open computer" }));
    fireEvent.click(screen.getByRole("button", { name: "Terminal" }));
    fireEvent.click(screen.getByRole("button", { name: "Edit instructions" }));
    fireEvent.click(screen.getByRole("button", { name: "Files" }));
    expect(ctx.onNavigate).toHaveBeenNthCalledWith(1, "computer");
    expect(ctx.onNavigate).toHaveBeenNthCalledWith(2, "terminal");
    expect(ctx.onNavigate).toHaveBeenNthCalledWith(3, "instructions");
    expect(ctx.onNavigate).toHaveBeenNthCalledWith(4, "files");
    fireEvent.click(screen.getByRole("button", { name: /Nightly report · working/ }));
    expect(ctx.onSelectConversation).toHaveBeenCalledWith("c9");
  });
  it("collapses from its button", () => {
    mock();
    const onChange = vi.fn();
    wrap(<Harness start ctx={ctxOf()} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Collapse status panel" }));
    expect(onChange).toHaveBeenCalledWith(false);
    expect(screen.getByRole("button", { name: "Expand status panel" })).toBeInTheDocument();
  });
  it("lists at most five pending items and links the rest to Approvals", () => {
    mock();
    const many = Array.from({ length: 7 }, (_, i) => gw(`g${i}`, `2099-01-01T00:0${i}:00Z`, { conversation_id: "c5" }));
    wrap(<Harness start ctx={ctxOf({}, { needs: needs(...many) })} />);
    expect(screen.getAllByRole("button", { name: "Allow once" })).toHaveLength(5);
    expect(screen.getByRole("link", { name: "2 more on Approvals" }).getAttribute("href")).toBe("/o/x/w/y/approvals");
  });
});

describe("approving from the rail", () => {
  it("an item of the open conversation links to its inline card instead of repeating the buttons", () => {
    mock();
    const card = document.createElement("div");
    card.id = "approval-g1";
    card.tabIndex = -1;
    card.scrollIntoView = vi.fn();
    document.body.appendChild(card);
    wrap(<Harness start ctx={ctxOf({}, { needs: needs(gw("g1", FAR), gw("g2", FAR, { conversation_id: "c5" })) })} />);
    // g1 belongs to the open conversation c1; g2 does not.
    expect(screen.getAllByRole("button", { name: "Allow once" })).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Show in conversation" }));
    expect(card.scrollIntoView).toHaveBeenCalled();
    expect(document.activeElement).toBe(card);
    card.remove();
  });
  it("Approve and Deny of another conversation's hold post the decision", async () => {
    const calls = mock({ "POST /v1/approvals/g2/decision": () => json(200, { id: "g2", status: "approved" }) });
    wrap(<Harness start ctx={ctxOf({}, { needs: needs(gw("g2", FAR, { conversation_id: "c5" })) })} />);
    fireEvent.click(screen.getByRole("button", { name: "Allow once" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path === "/v1/approvals/g2/decision")).toBe(true));
    expect(calls.find((c) => c.method === "POST")!.body).toEqual({ decision: "approve" });
  });
  it("an action approval is decided through its own route", async () => {
    const calls = mock({ "POST /v1/action-approvals/x1/decision": () => json(200, { id: "x1", status: "rejected" }) });
    const items = mergeNeeds([], [act1("x1", FAR)], () => null);
    wrap(<Harness start ctx={ctxOf({}, { needs: items })} />);
    fireEvent.click(screen.getByRole("button", { name: "Block" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/action-approvals/x1/decision")).toBe(true));
    expect(calls.find((c) => c.method === "POST")!.body).toEqual({ decision: "deny" });
  });
  it("409 shows Already decided.", async () => {
    mock({ "POST /v1/approvals/g2/decision": () => json(409, { error: { code: "already_decided", message: "already decided" } }) });
    wrap(<Harness start ctx={ctxOf({}, { needs: needs(gw("g2", FAR, { conversation_id: "c5" })) })} />);
    fireEvent.click(screen.getByRole("button", { name: "Allow once" }));
    expect(await screen.findByText("Already decided.")).toBeInTheDocument();
  });
});

describe("overlay mode (panel narrower than 1024 px)", () => {
  it("Esc collapses the expanded rail and returns focus to the expand button", async () => {
    mock();
    const onChange = vi.fn();
    wrap(<Harness start overlay ctx={ctxOf()} onChange={onChange} />);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onChange).toHaveBeenCalledWith(false);
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", { name: "Expand status panel" })));
  });
  it("Esc does nothing when the rail pushes (not an overlay)", () => {
    mock();
    const onChange = vi.fn();
    wrap(<Harness start ctx={ctxOf()} onChange={onChange} />);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onChange).not.toHaveBeenCalled();
  });
  it("expanding by hand moves focus into the rail", async () => {
    mock();
    wrap(<Harness overlay ctx={ctxOf()} />);
    fireEvent.click(screen.getByRole("button", { name: "Expand status panel" }));
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", { name: "Collapse status panel" })));
  });
});

describe("phone sheet", () => {
  it("opens the same five parts in a dialog", () => {
    mock();
    wrap(<StatusSheet ctx={ctxOf({}, { needs: needs(gw("g1", FAR, { conversation_id: "c5" })) })} open onOpenChange={() => undefined} />);
    const sheet = screen.getByRole("dialog", { name: "Status" });
    expect(within(sheet).getAllByRole("heading", { level: 3 }).map((h) => h.textContent)).toEqual(["Needs you1", "Doing now", "Recent tool calls", "Computer", "About"]);
    expect(within(sheet).getByRole("button", { name: "Allow once" })).toBeInTheDocument();
  });
});
