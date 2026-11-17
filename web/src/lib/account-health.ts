/**
 * The wordings of an account's health (`14 §7`, C-945), written once. Every place that shows health — the account's
 * row, the agent's header, the banner above the message box, the workspace Overview, the Test result — reads them
 * from here and uses them unchanged.
 */
import type { Account } from "@/lib/api/accounts";

type Health = Account["health"];
export type HealthFix = "reconnect" | "replace" | "test" | "turn_on";

export interface HealthLine {
  /** `ok` reads as normal, `attention` is a failure someone can fix, `neutral` is anything else. */
  tone: "ok" | "attention" | "neutral";
  text: string;
  /** What fixes it, and the button's words (`Replace token`, `Replace password`, `Replace key`). */
  fix?: { kind: HealthFix; label: string };
}

/** The app a line speaks of: the app's name, else the account's own (a website, an AI key). */
function appOf(a: Pick<Account, "app" | "name">): string {
  return a.app?.name ?? a.name;
}

/** `token`, `password` or `key`: what the app refuses (the server sends the word of the account's field). */
function wordOf(h: Health): string {
  return h.detail === "password" || h.detail === "key" ? h.detail : "token";
}

/** The line of `14 §7` for an account. `used` is the "used 2 hours ago" suffix of a working or unchecked account. */
export function healthLine(a: Pick<Account, "app" | "name" | "health">, used?: string): HealthLine {
  const h = a.health;
  switch (h.state) {
    case "rejected": {
      const w = wordOf(h);
      return { tone: "attention", text: `${appOf(a)} no longer accepts this ${w}`, fix: { kind: "replace", label: `Replace ${w}` } };
    }
    case "sign_in_expired":
      return { tone: "attention", text: `The sign-in to ${appOf(a)} expired`, fix: { kind: "reconnect", label: "Reconnect" } };
    case "unreachable":
      return {
        tone: "attention",
        text: h.detail ? `Could not be reached (${h.detail})` : "Could not be reached",
        fix: { kind: "test", label: "Test" },
      };
    case "disabled":
      return { tone: "neutral", text: "Turned off", fix: { kind: "turn_on", label: "Turn on" } };
    case "ok":
      return { tone: "ok", text: used ? `Works · used ${used}` : "Works" };
    default:
      return { tone: "neutral", text: used ? `Not checked yet · used ${used}` : "Not checked yet" };
  }
}

/** Whether an account needs attention: an agent cannot use it until someone fixes it. A turned-off account does not. */
export function needsAttention(a: Pick<Account, "health">): boolean {
  return a.health.state === "rejected" || a.health.state === "sign_in_expired" || a.health.state === "unreachable";
}

/**
 * The health line in lower case for a sentence ("Daneel can't use GitHub: the sign-in expired."). The app is already
 * named by the sentence, so the clause does not repeat it.
 */
export function healthClause(a: Pick<Account, "app" | "name" | "health">): string {
  const h = a.health;
  switch (h.state) {
    case "rejected":
      return `it no longer accepts this ${wordOf(h)}`;
    case "sign_in_expired":
      return "the sign-in expired";
    case "unreachable":
      return h.detail ? `it could not be reached (${h.detail})` : "it could not be reached";
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
