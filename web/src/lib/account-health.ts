/**
 * The wordings of an account's health (`14 §7`, C-945, D-383), written once. Every place that shows health — the account's
 * row, the agent's header, the banner above the message box, the workspace Overview, the Test result — reads them
 * from here and uses them unchanged.
 *
 * (P5-POLISH-6, D-383) A health line says only what a real call proved: `working` is "Worked <when>" from the last
 * successful agent call, `failing` is "Failed <when>: <reason>" until a call succeeds. A Test is shown apart (`test`:
 * "Test passed <when>" / "Test failed <when>: <reason>") and never reads as "Worked".
 */
import type { Account } from "@/lib/api/accounts";
import { formatWhen } from "@/lib/format-date";

type Health = Account["health"];
export type HealthFix = "reconnect" | "replace" | "test" | "turn_on";

export interface HealthLine {
  /** `ok` is a call that worked, `attention` a failure someone can fix, `neutral` is anything else. */
  tone: "ok" | "attention" | "neutral";
  text: string;
  /** What fixes it, and the button's words. */
  fix?: { kind: HealthFix; label: string };
  /** What the last Test said, apart from the line above ("Test passed 5 min ago"). */
  test?: { ok: boolean; text: string };
}

/** The app a line speaks of: the app's name, else the account's own (a website, an AI key). */
function appOf(a: Pick<Account, "app" | "name">): string {
  return a.app?.name ?? a.name;
}

/** `token`, `password` or `key`: what the app refuses (the server sends the word of the account's field). */
function wordOf(h: Health): string {
  return h.detail === "password" || h.detail === "key" ? h.detail : "token";
}

/** A time after a verb: "just now", "5 min ago", "yesterday", "18 Nov". */
export function whenWord(iso: string | null | undefined, now?: number): string {
  const w = formatWhen(iso, { now, empty: "" });
  return /^[A-Z][a-z]/.test(w) ? `${w.charAt(0).toLowerCase()}${w.slice(1)}` : w;
}

/** Why a call failed, in words (never the upstream's raw text). */
function reasonOf(a: Pick<Account, "app" | "name">, h: Health): string {
  switch (h.last_error) {
    case "unreachable":
      return h.detail ? `it could not be reached (${h.detail})` : "it could not be reached";
    case "expired":
      return "the sign-in expired";
    case "unexpected":
      return `${appOf(a)} gave an unexpected answer`;
    default:
      return `${appOf(a)} no longer accepts this ${wordOf(h)}`;
  }
}

function testLine(h: Health, now?: number): HealthLine["test"] {
  const t = h.test;
  if (!t) return undefined;
  const when = whenWord(t.at, now);
  if (t.result === "ok") return { ok: true, text: `Test passed ${when}` };
  const why = t.result === "unreachable" ? "it could not be reached" : t.result === "rejected" ? "it refused the sign-in" : "it gave an unexpected answer";
  return { ok: false, text: `Test failed ${when}: ${why}` };
}

/** The line of `14 §7` for an account. */
export function healthLine(a: Pick<Account, "app" | "name" | "health"> & { status_reason?: Account["status_reason"] }, now?: number): HealthLine {
  const h = a.health;
  const test = testLine(h, now);
  // (FIX-163) The account's self-hosted app instance was deleted: it cannot be turned on again, so no fix is offered here.
  if (h.state === "disabled" && a.status_reason === "instance_deleted") {
    return { tone: "neutral", text: "Turned off: the instance of this app was removed" };
  }
  switch (h.state) {
    // (C-1033) `failing`: an agent's call through the account failed and none has succeeded since.
    case "failing": {
      const when = whenWord(h.last_error_at, now);
      const text = `Failed${when ? ` ${when}` : ""}: ${reasonOf(a, h)}`;
      if (h.last_error === "unreachable") return { tone: "attention", text, fix: { kind: "test", label: "Try again" }, test };
      return { tone: "attention", text, fix: { kind: "replace", label: "Reconnect" }, test };
    }
    case "sign_in_expired":
      return { tone: "attention", text: "Sign-in expired", fix: { kind: "reconnect", label: "Sign in again" }, test };
    case "disabled":
      return { tone: "neutral", text: "Turned off", fix: { kind: "turn_on", label: "Turn on" }, test };
    case "working": {
      const when = whenWord(h.last_ok_at ?? h.last_used_at, now);
      return { tone: "ok", text: when ? `Worked ${when}` : "Worked", test };
    }
    default:
      return { tone: "neutral", text: "Not used yet", test };
  }
}

/** Whether an account needs attention: an agent cannot use it until someone fixes it. A turned-off account does not. */
export function needsAttention(a: Pick<Account, "health">): boolean {
  return a.health.state === "failing" || a.health.state === "sign_in_expired";
}

/**
 * The health line in lower case for a sentence ("Daneel can't use GitHub: the sign-in expired."). The app is already
 * named by the sentence, so the clause does not repeat it.
 */
export function healthClause(a: Pick<Account, "app" | "name" | "health">): string {
  const h = a.health;
  switch (h.state) {
    case "failing":
      if (h.last_error === "unreachable") return h.detail ? `it could not be reached (${h.detail})` : "it could not be reached";
      if (h.last_error === "expired") return "the sign-in expired";
      if (h.last_error === "unexpected") return "it gave an unexpected answer";
      return `it no longer accepts this ${wordOf(h)}`;
    case "sign_in_expired":
      return "the sign-in expired";
    default:
      return healthLine(a).text.toLowerCase();
  }
}

/** "1 account needs attention" / "<n> accounts need attention"; `null` when none does. */
export function attentionCount(n: number): string | null {
  if (n <= 0) return null;
  return n === 1 ? "1 account needs attention" : `${n} accounts need attention`;
}

/** The result of Test shown in place of the health line (`14 §7`). `null` when the stored health says it. */
export function testOutcomeText(result: "not_testable" | "error"): string {
  return result === "error" ? "The test could not run. Try again." : "This account cannot be tested. Its state will show after an agent uses it.";
}

/** The warning on the journey's Done step after a failed first test: the account is kept. */
export function savedButText(app: string, host: string | undefined): string {
  return `Saved, but ${app} could not be reached${host ? ` (${host})` : ""}. Check the address or test again.`;
}
