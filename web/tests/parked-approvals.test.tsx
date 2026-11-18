import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PushLine } from "@/components/admin/push-line";
import { ApprovalCard } from "@/components/approvals/approvals-view";
import { resumeStatus } from "@/components/approvals/parked";
import { ApprovalCard as ChatApprovalCard } from "@/components/chat/approval-card";
import { SimulationResult } from "@/components/policy/simulate-panel";
import { RuleEditorDialog } from "@/components/policy/rule-editor";
import type { TranscriptEvent } from "@/lib/api/conversations";
import { effectiveMax, fmtSecs, limitsHint, type ApprovalLimits } from "@/lib/approval-limits";
import { buildRows } from "@/lib/chat/layout";
import { applyEvents, emptyTranscript } from "@/lib/chat/transcript";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const LIMITS: ApprovalLimits = {
  timeout_secs: 300,
  max_timeout_secs: 3600,
  park: { enabled: true, threshold_secs: 900, max_timeout_secs: 86400, resume_window_secs: 1800 },
};

function stub(routes: Record<string, unknown> = {}) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const path = new URL(input.url, "http://localhost").pathname;
      if (path in routes) return json(200, routes[path]);
      if (path === "/v1/policy/approval-limits") return json(200, LIMITS);
      if (path === "/v1/workspaces/w1") return json(200, { my_role: "approver" });
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

function wrap(ui: React.ReactNode) {
  return render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);
}

const approval = (over: Record<string, unknown> = {}) =>
  ({
    id: "ap1", org_id: "o1", workspace_id: "w1", agent: { id: "a1", name: "Bot", handle: "bot" },
    method: "POST", host: "api.example.com", path: "/x", rule_name: "r",
    summary: { title: "Post to example", fields: [], source: "generic" },
    sentence: { agent: { id: "a1", name: "Bot" }, action: "send a POST request to api.example.com" },
    can_always_allow: false,
    status: "pending", parked: false, deadline: new Date(Date.now() + 3_600_000).toISOString(), created_at: "2026-01-01T00:00:00Z",
    ...over,
  }) as never;

describe("Approvals page: parked approvals", () => {
  it("a parked approval shows the chip and the stopped-waiting sentence with the live window", async () => {
    stub();
    wrap(<ul><ApprovalCard approval={approval({ parked: true })} now={Date.now()} onDecide={() => {}} pending={false} /></ul>);
    expect(screen.getByText("Parked")).toBeInTheDocument();
    expect(await screen.findByText(/The agent has stopped waiting\. If you approve, it will be told and can repeat this exact request once within 30 minutes\./)).toBeInTheDocument();
  });

  it("a held approval shows neither", () => {
    stub();
    wrap(<ul><ApprovalCard approval={approval()} now={Date.now()} onDecide={() => {}} pending={false} /></ul>);
    expect(screen.queryByText("Parked")).toBeNull();
    expect(screen.queryByTestId("parked-note")).toBeNull();
  });

  it("allowing a parked approval asks first, with the sentence; a held one decides at once", async () => {
    stub();
    const onDecide = vi.fn();
    wrap(<ul><ApprovalCard approval={approval({ parked: true })} now={Date.now()} onDecide={onDecide} pending={false} /></ul>);
    fireEvent.click(screen.getByRole("button", { name: "Allow once" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/can repeat this exact request once within/)).toBeInTheDocument();
    expect(onDecide).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Allow once" }));
    expect(onDecide).toHaveBeenCalledWith("approve");

    cleanup();
    const held = vi.fn();
    wrap(<ul><ApprovalCard approval={approval()} now={Date.now()} onDecide={held} pending={false} /></ul>);
    fireEvent.click(screen.getByRole("button", { name: "Allow once" }));
    expect(held).toHaveBeenCalledWith("approve");
  });

  it("the focused approval is marked for a notification link", () => {
    stub();
    wrap(<ul><ApprovalCard approval={approval()} now={Date.now()} onDecide={() => {}} pending={false} focused /></ul>);
    expect(document.getElementById("approval-ap1")).toHaveAttribute("aria-current", "true");
  });
});

describe("post-decision states", () => {
  const base = { parked: true, status: "approved" as const };
  it.each([
    [{ ...base, resume_state: "ready", resume_expires_at: "2026-11-17T12:30:00Z" }, /^Approved — waiting for the agent to retry \(until .+\)$/],
    [{ ...base, resume_state: "consumed" }, /^Used$/],
    [{ ...base, resume_state: "expired" }, /^Not used \(window ended\)$/],
    [{ ...base, resume_state: "revoked" }, /^Revoked$/],
    [{ parked: true, status: "abandoned", resume_state: null }, /^No longer needed$/],
  ])("%j", (a, text) => {
    expect(resumeStatus(a as never)?.text).toMatch(text);
  });
  it("is nothing for a held approval or a pending one", () => {
    expect(resumeStatus({ parked: false, status: "approved", resume_state: null } as never)).toBeNull();
    expect(resumeStatus({ parked: true, status: "pending", resume_state: null } as never)).toBeNull();
  });
});

describe("chat: the inline approval and the follow-up", () => {
  const ev = (seq: number, type: string, payload: Record<string, unknown>): TranscriptEvent => ({ seq, type, ts: "2026-01-01T00:00:00Z", payload });

  it("shows Parked instead of Held, with the sentence", async () => {
    stub();
    const pending = ev(1, "approval.pending", { approval_id: "ap1", summary: "Post to example", parked: true, expires_at: "2026-01-01T01:00:00Z" });
    const state = applyEvents(emptyTranscript, [pending]);
    wrap(<ChatApprovalCard ev={pending} state={state} wsId="w1" />);
    expect(screen.getByText("Parked")).toBeInTheDocument();
    expect(screen.queryByText("Held")).toBeNull();
    expect(await screen.findByText(/once within 30 minutes/)).toBeInTheDocument();
  });

  it("approved and parked: the agent will be told, until a time; then not used when the window ends", () => {
    stub();
    const pending = ev(1, "approval.pending", { approval_id: "ap1", summary: "s", parked: true });
    const decided = ev(2, "approval.decided", { approval_id: "ap1", decision: "approve", decided_by_label: "Ann", resume_expires_at: "2026-01-01T00:30:00Z" });
    const s1 = applyEvents(emptyTranscript, [pending, decided]);
    wrap(<ChatApprovalCard ev={pending} state={s1} wsId="w1" />);
    expect(screen.getByText(/Approved by Ann/)).toBeInTheDocument();
    expect(screen.getByTestId("parked-after").textContent).toMatch(/The agent will be told and can repeat the request until/);
    cleanup();
    const ended = ev(3, "approval.expired", { approval_id: "ap1", reason: "resume_window" });
    wrap(<ChatApprovalCard ev={pending} state={applyEvents(emptyTranscript, [pending, decided, ended])} wsId="w1" />);
    expect(screen.getByTestId("parked-after").textContent).toBe("Not used (window ended)");
  });

  it("the resume follow-up (door system, speaker Approval) carries the Approval door chip", () => {
    const queued = ev(5, "turn.queued", { turn_id: "t1", door: "system", speaker_label: "Approval", text: "Aurora: a person approved a request you were waiting for." });
    const rows = buildRows(applyEvents(emptyTranscript, [queued]), new Date("2026-01-01T00:05:00Z"));
    const group = rows.find((r) => r.kind === "group");
    expect(group).toMatchObject({ door: "Approval", name: "Aurora" });
    const other = buildRows(applyEvents(emptyTranscript, [ev(6, "turn.queued", { turn_id: "t2", door: "system", speaker_label: "Schedule", text: "x" })]), new Date("2026-01-01T00:05:00Z"));
    expect(other.find((r) => r.kind === "group")).toMatchObject({ door: "system" });
  });
});

describe("approval limits", () => {
  it("formats durations and picks the effective maximum", () => {
    expect([fmtSecs(90), fmtSecs(300), fmtSecs(3600), fmtSecs(86400)]).toEqual(["90 s", "5 min", "1 h", "24 h"]);
    expect(effectiveMax(LIMITS)).toBe(86400);
    expect(effectiveMax({ ...LIMITS, park: { ...LIMITS.park, enabled: false } })).toBe(3600);
    expect(effectiveMax(undefined)).toBeUndefined();
  });
  it("the hint adds the parking sentence only while parking is on", () => {
    expect(limitsHint(LIMITS)).toBe(
      "Empty = the instance default (5 min). At least 60 s, at most 24 h. Above 15 min the agent stops waiting and is told when you decide (up to 24 h).",
    );
    expect(limitsHint({ ...LIMITS, park: { ...LIMITS.park, enabled: false } })).toBe("Empty = the instance default (5 min). At least 60 s, at most 1 h.");
  });

  it("the rule editor takes its bound and hint from the limits route", async () => {
    stub();
    wrap(
      <RuleEditorDialog
        open
        initial={{ name: "r", enabled: true, action: "allow", require_approval: true, identities: [], targets: [], conditions: [] } as never}
        data={{ agents: [], users: [], secrets: [] }}
        onClose={() => {}}
        onSave={() => {}}
      />,
    );
    expect(await screen.findByText(/Above 15 min the agent stops waiting/)).toBeInTheDocument();
    await waitFor(() => expect(screen.getByLabelText("Approval timeout (seconds)")).toHaveAttribute("max", "86400"));
  });

  it("simulate says when an approval parks or its timeout was clamped", () => {
    const sim = { verdict: "needs_approval", approval_parks: true, approval_timeout_clamped: true, approval_timeout_secs: 86400, candidates: [], trace: { organization: { considered: [] }, workspace: { considered: [] } } };
    wrap(<SimulationResult sim={sim as never} />);
    const line = screen.getByTestId("approval-timing").textContent;
    expect(line).toContain("Parks");
    expect(line).toContain("Timeout clamped to 24 h");
  });
});

describe("Admin: Web Push line", () => {
  it.each([
    [true, "On"],
    [false, "Off"],
  ])("push %s reads %s", async (push, word) => {
    stub({ "/v1/instance": { features: { push } } });
    wrap(<PushLine />);
    const line = await screen.findByTestId("push-line");
    expect(within(line).getByText(word)).toBeInTheDocument();
  });
});
