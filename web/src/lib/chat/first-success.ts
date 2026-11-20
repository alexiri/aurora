import type { TranscriptEvent } from "@/lib/api/conversations";

import { clockTime } from "./layout";

/**
 * (P5-REFINE-11 item 9, rec 10) The four first successes, each shown once and dismissed per person. The flags are the
 * person's server-side `notices_seen` keys (`PATCH /v1/me/onboarding { notice_seen }`); the API accepts lowercase letters,
 * digits and dashes only, so the keys are dashed.
 */
export type FirstSuccessKey = "first-answer" | "first-app" | "first-approval" | "first-run";

export const FIRST_SUCCESS_KEYS: readonly FirstSuccessKey[] = ["first-answer", "first-app", "first-approval", "first-run"];

/** A success older than this is history, not news: a person who has used the product for weeks sees no card for an old chat. */
export const FIRST_SUCCESS_WINDOW_MS = 24 * 60 * 60 * 1000;

export type FirstSuccess =
  | { key: "first-answer"; tools: number }
  | { key: "first-app"; appId: string; appName: string }
  | { key: "first-approval" }
  | { key: "first-run"; title: string; at: string };

const str = (v: unknown): string => (typeof v === "string" ? v : "");

/**
 * The first successes the events of this conversation show that the person has not dismissed yet, in the order of
 * the brief. Pure: `now` is passed in.
 */
export function pendingFirstSuccesses(
  events: readonly TranscriptEvent[],
  seen: readonly string[],
  conversation: { kind: string; title?: string | null } | null,
  now: Date,
): FirstSuccess[] {
  const fresh = (e: TranscriptEvent) => now.getTime() - Date.parse(e.ts) <= FIRST_SUCCESS_WINDOW_MS;
  const out: FirstSuccess[] = [];
  const done = events.filter((e) => e.type === "turn.finished" && str(e.payload.status) === "done");

  if (!seen.includes("first-answer")) {
    // A finished turn of the person's whose turn has an agent text: the person's first finished turn (D-414).
    const turn = done.find((e) => fresh(e) && events.some((x) => x.type === "text" && str(x.payload.turn_id) === str(e.payload.turn_id)));
    if (turn) {
      const tools = events.filter((x) => x.type === "tool.started" && str(x.payload.turn_id) === str(turn.payload.turn_id)).length;
      out.push({ key: "first-answer", tools });
    }
  }
  if (!seen.includes("first-app")) {
    const given = events.find((e) => e.type === "notice" && str(e.payload.code) === "app_access_given" && fresh(e));
    if (given) out.push({ key: "first-app", appId: str(given.payload.app_id), appName: str(given.payload.app_name) });
  }
  if (!seen.includes("first-approval")) {
    const allowed = events.find((e) => e.type === "approval.decided" && fresh(e) && ["approved", "allow", "allowed"].includes(str(e.payload.decision || e.payload.status)));
    if (allowed) out.push({ key: "first-approval" });
  }
  if (!seen.includes("first-run") && conversation?.kind === "schedule") {
    const last = [...done].reverse().find(fresh);
    if (last) out.push({ key: "first-run", title: conversation.title || "The schedule", at: last.ts });
  }
  return out;
}

/** "Morning summary ran at 09:00." */
export const runSentence = (title: string, at: string): string => `${title} ran at ${clockTime(at)}.`;
