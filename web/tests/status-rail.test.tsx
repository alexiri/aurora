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
import { actionsOnly, hasProblem, mergeNeeds, sandboxLook, type AgentStatus } from "@/components/agents/status-rail/status-data";
import type { TimelineItem } from "@/lib/api/timeline";
import type { ActionApproval } from "@/lib/api/action-approvals";
import type { Approval } from "@/lib/api/approvals";
import { RAIL_OVERLAY_BELOW } from "@/components/chat/chat-panel";
import { wrap } from "./support/render";
import { json } from "./support/api";

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
  did: [],
  didLoading: false,
  didError: false,
  problem: false,
  sandbox: undefined,
  ...over,
});
const ctxOf = (over: Partial<RailContext> = {}, s: Partial<AgentStatus> = {}): RailContext => ({
  status: status(s),
  open: { active: false, line: null },
  wsId: "w",
  base: "/o/x/w/y",
  agentBase: "/o/x/w/y/agents/scout",
  agentId: "a1",
  agent: { name: "Scout", kind: "hosted", isDefault: true, brief: "Triage pull requests." },
  terminalAvailable: true,
  openConversation: { id: "c1", title: "Direct chat" },
  titleOf: (c) => c.title ?? "Untitled",
  onSelectConversation: vi.fn(),
  onNavigate: vi.fn(),
  ...over,
});
const item = (over: Partial<TimelineItem> & Pick<TimelineItem, "id" | "kind">): TimelineItem => ({
  at: new Date().toISOString(),
  state: "done",
  agent: { id: "a1", name: "Scout" },
  ...over,
});
const needs = (...n: Parameters<typeof mergeNeeds>[0]) => mergeNeeds(n, [], (id) => (id === "c1" ? "Direct chat" : null));

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
  it("a failed or blocked timeline row is a problem (the red dot on the collapsed icon); a waiting or done one is not", () => {
    expect(hasProblem([{ state: "done" }, { state: "waiting" }, { state: "running" }, { state: "skipped" }])).toBe(false);
    expect(hasProblem([{ state: "done" }, { state: "blocked" }])).toBe(true);
    expect(hasProblem([{ state: "failed" }])).toBe(true);
  });
  it("lists only actions: the agent's own model calls are filtered out", () => {
    const rows = [{ llm_model: "claude-opus" }, { llm_model: null }, {}];
    expect(actionsOnly(rows as never)).toEqual([{ llm_model: null }, {}]);
  });

  it("shares the sandbox labels and failure tooltip with the header pill", () => {
    expect(sandboxLook({ state: "starting", failure_message: null, failure_code: null })).toEqual({ tone: "neutral", label: "Sandbox starting", title: undefined });
    expect(sandboxLook({ state: "failed", failure_message: "no runner", failure_code: "pod_failed" })).toMatchObject({
      tone: "danger", label: "Sandbox failed", title: "no runner (pod_failed)",
    });
  });
});

describe("collapsed strip", () => {
  it("shows the three icons in order, each named with its state in words and labelled Need · Now · Did", () => {
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
      "Need: 2 approvals waiting for you",
      "Doing now: working",
      "What Scout did: some requests were denied or failed",
    ]);
    // (rec 3) The visible words under the icons.
    // (WCAG 2.5.3) Every collapsed button's visible text is contained in its accessible name.
    for (const b of within(strip).getAllByRole("button")) {
      const clone = b.cloneNode(true) as HTMLElement;
      clone.querySelectorAll("[aria-hidden=\"true\"]").forEach((n) => n.remove());
      const visible = (clone.textContent ?? "").trim().toLowerCase();
      if (visible) expect(b.getAttribute("aria-label")!.toLowerCase()).toContain(visible);
    }
    expect(within(strip).getByText("Need")).toBeInTheDocument();
    expect(within(strip).getByText("Now")).toBeInTheDocument();
    expect(within(strip).getByText("Did")).toBeInTheDocument();
    // The count on the Needs you icon.
    expect(within(screen.getByRole("button", { name: /^Need: / })).getByText("2")).toBeInTheDocument();
  });
  it("says idle / nothing waiting / none failed when quiet", () => {
    mock();
    const labels = stripLabels(ctxOf());
    expect(labels).toEqual({
      needs: "Need: nothing waiting for you",
      doing: "Doing now: idle",
      did: "What Scout did: none denied or failed",
    });
    expect(stripLabels(ctxOf({}, { needs: needs(gw("g1", FAR)) })).needs).toBe("Need: 1 approval waiting for you");
    expect(stripLabels(ctxOf({ open: { active: true, line: "Reading the repo" } })).doing).toBe("Doing now: working");
  });
  it("non-approvers get no Needs you icon and no Needs you part", () => {
    mock();
    wrap(<Harness ctx={ctxOf({}, { canApprove: false })} />);
    expect(screen.queryByRole("button", { name: /^Need: / })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Expand status panel" }));
    expect(screen.getByRole("heading", { name: "Doing now" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: /Needs you/ })).toBeNull();
    expect(screen.queryByText("Nothing is waiting for you.")).toBeNull();
  });
  it("the other icons expand the rail", () => {
    mock();
    const onChange = vi.fn();
    wrap(<Harness ctx={ctxOf()} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^What Scout did:/ }));
    expect(onChange).toHaveBeenCalledWith(true);
    expect(screen.getByRole("button", { name: "Collapse status panel" })).toBeInTheDocument();
  });
});

describe("Needs you pop-over", () => {
  it("opens from the icon with the pending items, closes on Esc and returns focus", async () => {
    mock();
    wrap(<Harness ctx={ctxOf({}, { needs: needs(gw("g1", FAR)) })} />);
    const icon = screen.getByRole("button", { name: "Need: 1 approval waiting for you" });
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
    fireEvent.click(screen.getByRole("button", { name: /^Need: / }));
    fireEvent.click(await screen.findByRole("button", { name: "Open the status panel" }));
    expect(await screen.findByRole("heading", { name: "Status" })).toBeInTheDocument();
  });
});

describe("expanded rail", () => {
  it("shows the three parts in order with their data: Needs you · Doing now (with the computer's line) · What Scout did", () => {
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
            did: [
              item({ id: "turn:t1", kind: "answered", outcome: "answered", model: "Workspace default", count: 1, link: { type: "conversation", id: "c1", seq: 7 } }),
              item({ id: "call:1", kind: "called", state: "blocked", outcome: "blocked_by_rule", app: { id: "gh", name: "GitHub" }, rule: { logical_id: "r", name: "Block evil.example.org" }, link: { type: "app", id: "gh" } }),
              item({ id: "approval:1", kind: "asked_before", state: "waiting", outcome: null, host: "docs.example.org", link: { type: "approval", id: "p1" } }),
              item({ id: "run:1", kind: "ran_schedule", state: "running", outcome: null, schedule: { id: "s1", name: "Morning summary" }, link: null }),
            ],
            problem: true,
            sandbox: { state: "stopped", failure_message: null, failure_code: null } as never,
          },
        )}
      />,
    );
    const heads = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(heads).toEqual(["Needs you1", "Doing now", "What Scout did"]);
    // Doing now: the open conversation's running tool from its stream.
    expect(screen.getByText("Direct chat · Running gh pr list")).toBeInTheDocument();
    // What Scout did: the timeline's rows as sentences, each a link to its target (D-415).
    const did = within(screen.getByTestId("rail-did"));
    expect(did.getByRole("link", { name: /Scout answered you/ }).getAttribute("href")).toBe("/o/x/w/y/agents/scout?c=c1&m=7");
    expect(did.getByText("Used Workspace default · 1 call · Just now")).toBeInTheDocument();
    expect(did.getByRole("link", { name: /Scout called GitHub/ }).getAttribute("href")).toBe("/o/x/w/y/apps");
    expect(did.getByText(/Blocked by your rule ‘Block evil.example.org’/)).toBeInTheDocument();
    expect(did.getByRole("link", { name: /Scout asked before changing docs.example.org/ }).getAttribute("href")).toBe("/o/x/w/y/approvals");
    expect(did.getByText(/Waiting for you in Approvals/)).toBeInTheDocument();
    // A row with no target is plain text; a running row reads as in progress.
    expect(did.getByText("Running ‘Morning summary’…")).toBeInTheDocument();
    expect(did.queryByRole("link", { name: /Morning summary/ })).toBeNull();
    expect(did.getAllByRole("listitem")).toHaveLength(4);
    expect(screen.getByRole("link", { name: "All activity" }).getAttribute("href")).toBe("/o/x/w/y/activity?agent=a1");
    expect(screen.getByRole("link", { name: "All activity" })).toHaveTextContent("All");
    // The computer is one line under Doing now, the state in words; About is gone (it lives in Settings).
    expect(screen.getByTestId("rail-computer-state")).toHaveTextContent("Asleep");
    expect(screen.getByText("On its computer")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open Scout's computer" })).toHaveTextContent("Open");
    expect(screen.queryByRole("heading", { name: "About" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Edit instructions" })).toBeNull();
    expect(screen.queryByText("Came with this workspace")).toBeNull();
    expect(screen.queryByText("Recent actions")).toBeNull();
  });
  it("says Idle when nothing runs and the computer's line carries its state in words", () => {
    mock();
    wrap(<Harness start ctx={ctxOf({}, { sandbox: { state: "running", failure_message: null, failure_code: null } as never })} />);
    expect(screen.getByText("Idle")).toBeInTheDocument();
    expect(screen.getByTestId("rail-computer-state")).toHaveTextContent("Awake");
    expect(screen.getByText("Nothing is waiting for you.")).toBeInTheDocument();
  });
  it("navigates: Open computer, and selects a working conversation", () => {
    mock();
    const ctx = ctxOf({}, { working: [{ id: "c9", title: "Nightly report", kind: "schedule", external_ref: "x", direct_user: null, active_turn: { id: "t" } } as never] });
    wrap(<Harness start ctx={ctx} />);
    fireEvent.click(screen.getByRole("button", { name: "Open Scout's computer" }));
    expect(ctx.onNavigate).toHaveBeenNthCalledWith(1, "computer");
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

describe("overlay mode (window narrower than 1400 px, chat-6)", () => {
  it("overlays the transcript below 1400 px instead of squeezing the chat", () => {
    expect(RAIL_OVERLAY_BELOW).toBe(1400);
  });
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
  it("opens the same three parts in a dialog", () => {
    mock();
    wrap(<StatusSheet ctx={ctxOf({}, { needs: needs(gw("g1", FAR, { conversation_id: "c5" })) })} open onOpenChange={() => undefined} />);
    const sheet = screen.getByRole("dialog", { name: "Status" });
    expect(within(sheet).getAllByRole("heading", { level: 3 }).map((h) => h.textContent)).toEqual(["Needs you1", "Doing now", "What Scout did"]);
    expect(within(sheet).getByRole("button", { name: "Allow once" })).toBeInTheDocument();
  });
});
