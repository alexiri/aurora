import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { TIMELINE_KIND, TIMELINE_OUTCOME_LINE, timelineWords } from "@/lib/labels";
import type { TimelineItem } from "@/lib/api/timeline";
import { timelineHref } from "@/lib/timeline-links";

// (P5-REFINE-11, -16; D-415) The timeline's rows as sentences, from structured kinds and names; and where each row leads.

const item = (over: Partial<TimelineItem> & Pick<TimelineItem, "kind">): TimelineItem => ({
  id: "x",
  at: "2026-11-20T10:00:00Z",
  state: "done",
  agent: { id: "a", name: "Dani" },
  ...over,
});

describe("timelineWords", () => {
  it("writes the three rows of the workspace-2 mockup from the fields alone", () => {
    expect(timelineWords(item({ kind: "answered", outcome: "answered", model: "Workspace default", count: 1 }))).toEqual({
      title: "Dani answered you",
      meta: "Used Workspace default · 1 call",
      tone: "accent",
    });
    expect(
      timelineWords(item({ kind: "called", state: "blocked", outcome: "blocked_by_rule", app: { id: "gh", name: "GitHub" }, rule: { logical_id: "r", name: "Block evil.example.org" }, count: 1 })),
    ).toEqual({ title: "Dani called GitHub", meta: "Blocked by your rule ‘Block evil.example.org’", tone: "danger" });
    expect(timelineWords(item({ kind: "asked_before", state: "waiting", outcome: null, host: "docs.example.org" }))).toEqual({
      title: "Dani asked before changing docs.example.org",
      meta: "Waiting for you in Approvals",
      tone: "pending",
    });
  });

  it("a running row reads as in progress and its outcome replaces it", () => {
    const schedule = { id: "s", name: "Morning summary" };
    expect(timelineWords(item({ kind: "ran_schedule", state: "running", outcome: null, schedule })).title).toBe("Running ‘Morning summary’…");
    expect(timelineWords(item({ kind: "ran_schedule", state: "done", outcome: "answered", schedule }))).toMatchObject({ title: "Ran ‘Morning summary’", meta: "Answered" });
    expect(timelineWords(item({ kind: "answered", state: "running", outcome: null })).title).toBe("Dani is answering");
  });

  it("names the computer's work and the other kinds", () => {
    expect(timelineWords(item({ kind: "computer", outcome: "ok", command: "ls", files: ["notes.txt"], count: 1 }))).toMatchObject({
      title: "Dani ran ‘ls’ on its computer",
      meta: "Wrote notes.txt",
    });
    expect(timelineWords(item({ kind: "computer", outcome: "ok", command: null, files: ["notes.txt"], count: 1 })).title).toBe("Dani wrote notes.txt");
    expect(timelineWords(item({ kind: "computer", outcome: "ok", command: null, files: null, count: null })).title).toBe("Dani ran something on its computer");
    expect(timelineWords(item({ kind: "handed_task", outcome: "completed", peer: { id: "p", name: "Rex" } }))).toMatchObject({ title: "Dani asked Rex for help", meta: "Done" });
    expect(timelineWords(item({ kind: "got_task", outcome: "completed", peer: { id: "p", name: "Rex" } })).title).toBe("Rex asked Dani for help");
    expect(timelineWords(item({ kind: "received_webhook", outcome: "refused_signature", webhook: { id: "w", name: "Deploys" } }))).toMatchObject({ title: "Received ‘Deploys’", meta: "Refused: the signature didn't match" });
    expect(timelineWords(item({ kind: "asked_for_access", state: "waiting", outcome: null, app: { id: "j", name: "Jira" } }))).toMatchObject({ title: "Dani asked for access to Jira", meta: "Waiting for you" });
  });

  it("says who decided an approval", () => {
    expect(timelineWords(item({ kind: "asked_before", outcome: "allowed_once", host: "x.test", person: { display_name: "Alice" } })).meta).toBe("Allowed once by Alice");
  });
});

describe("the outcome words", () => {
  const SCHEMA = readFileSync(join(__dirname, "..", "src", "lib", "api", "schema.d.ts"), "utf8");
  const kinds = [...(/TimelineKind: ((?:"[^"]+"(?: \| )?)+);/.exec(SCHEMA)?.[1] ?? "").matchAll(/"([^"]+)"/g)].map((m) => m[1]!);

  it("has a filter word and a table of outcome lines for every kind the API sends", () => {
    expect(kinds.length).toBe(9);
    for (const k of kinds) {
      expect(TIMELINE_KIND, k).toHaveProperty(k);
      expect(Object.keys(TIMELINE_OUTCOME_LINE[k] ?? {}).length, k).toBeGreaterThan(0);
    }
  });

  it("has a line for every outcome word C-1083 lists, and none reads as a raw key", () => {
    const listed: Record<string, string[]> = {
      answered: ["answered", "failed", "stopped"],
      computer: ["ok", "failed"],
      asked_before: ["allowed_once", "always_allowed", "blocked", "expired", "abandoned"],
      called: ["allowed", "blocked_by_rule", "blocked_default", "not_approved", "rate_limited", "failed"],
      ran_schedule: ["answered", "skipped_by_gate", "failed", "skipped"],
      received_webhook: ["answered", "failed", "refused_signature", "refused_too_large", "refused_paused", "refused_frozen", "refused_unauthorized", "duplicate"],
      handed_task: ["completed", "cancelled", "expired"],
      got_task: ["completed", "cancelled", "expired"],
      asked_for_access: ["given", "dismissed", "expired"],
    };
    for (const [k, outs] of Object.entries(listed)) {
      for (const o of outs) {
        const line = TIMELINE_OUTCOME_LINE[k]?.[o];
        expect(line, `${k}.${o}`).toBeTruthy();
        expect(line, `${k}.${o}`).not.toMatch(/_/);
      }
    }
  });
});

describe("timelineHref", () => {
  const where = { base: "/o/x/w/y", agentBase: "/o/x/w/y/agents/dani" };
  it("leads each row to its target", () => {
    expect(timelineHref(item({ kind: "answered", link: { type: "conversation", id: "c1", seq: 4 } }), where)).toBe("/o/x/w/y/agents/dani?c=c1&m=4");
    expect(timelineHref(item({ kind: "asked_before", link: { type: "approval", id: "p" } }), where)).toBe("/o/x/w/y/approvals");
    expect(timelineHref(item({ kind: "asked_before", link: { type: "action_approval", id: "p" } }), where)).toBe("/o/x/w/y/approvals");
    expect(timelineHref(item({ kind: "computer", link: { type: "file", id: "a", path: "/workspace/notes.txt" } }), where)).toBe(
      "/o/x/w/y/agents/dani?tab=files&path=%2Fworkspace%2Fnotes.txt",
    );
    expect(timelineHref(item({ kind: "called", link: { type: "app", id: "gh" } }), where)).toBe("/o/x/w/y/apps");
    expect(timelineHref(item({ kind: "ran_schedule", link: { type: "run", id: "r" } }), where)).toBe("/o/x/w/y/agents/dani?tab=schedules");
    expect(timelineHref(item({ kind: "handed_task", link: { type: "peer_task", id: "t" } }), where)).toBe("/o/x/w/y/agents/dani?tab=peers");
    expect(timelineHref(item({ kind: "ran_schedule", link: null }), where)).toBeNull();
  });
});
