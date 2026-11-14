import { ApiError } from "@/lib/api/client";
import { fieldErrors } from "@/lib/api/errors";
import type { components } from "@/lib/api/schema";

// Pure rules of the per-agent connection grant editor (docs/14 › agent page › Access, 07 §Grants, 09 §3). Which
// tools exist, their read/write group and which of them is a group umbrella all come from the app's catalogue
// (`GET /v1/apps/{app}`, `Tool.group`, `Tool.wildcard`); nothing is decided here that the catalogue does not say.

type S = components["schemas"];
export type Tool = S["Tool"];
export type ConnectionGrant = S["ConnectionGrant"];
export type PutConnectionGrant = S["PutConnectionGrant"];

export type Decision = "allow" | "ask" | "block";
export type Group = "read" | "write";
export const GROUPS: { value: Group; label: string }[] = [
  { value: "read", label: "Read" },
  { value: "write", label: "Write" },
];

/** Bounds of an ask timeout (07 §Approval deadlines); the instance maximum may be lower, the API then says so. */
export const ASK_TIMEOUT_MIN = 60;
export const ASK_TIMEOUT_MAX = 14400;

export const groupOf = (t: Tool): Group => (t.group === "write" ? "write" : "read");

/**
 * The tools that apply to a connection made with `method` (FIX-123, D-214): a tool restricted to other connect methods
 * is not offered (the gateway does not classify it for this connection either). A tool without `connect_methods`
 * applies to every method, and a connection with no recorded method hides nothing.
 */
export function toolsForMethod(tools: Tool[], method: string | null | undefined): Tool[] {
  return tools.filter((t) => !method || (t.connect_methods ?? []).length === 0 || (t.connect_methods ?? []).includes(method));
}

/** The tools of a group, its umbrella first (an umbrella exists only where the manifest declares and proves one). */
export function groupTools(tools: Tool[], g: Group): Tool[] {
  const own = tools.filter((t) => groupOf(t) === g);
  return [...own.filter((t) => t.wildcard), ...own.filter((t) => !t.wildcard)];
}

/** Tool ids a grant names that the app's catalogue no longer has (a package upgrade removed them): inert (07). */
export function inertToolIds(tools: Tool[], grant: Pick<ConnectionGrant, "allow_tools" | "ask_tools"> | null | undefined): string[] {
  if (!grant) return [];
  const known = new Set(tools.map((t) => t.id));
  return [...grant.allow_tools, ...grant.ask_tools].filter((id) => !known.has(id));
}

/**
 * The starting decisions of the editor: every catalogue tool on block (so a tool added by an upgrade, or a fresh
 * `custom_only` grant, starts blocked), then the grant's own allow / ask ids, inert ones included.
 */
export function initialDecisions(tools: Tool[], grant: Pick<ConnectionGrant, "mode" | "allow_tools" | "ask_tools"> | null | undefined): Record<string, Decision> {
  const out: Record<string, Decision> = Object.fromEntries(tools.map((t) => [t.id, "block" as Decision]));
  if (grant?.mode === "custom") {
    for (const id of grant.allow_tools) out[id] = "allow";
    for (const id of grant.ask_tools) out[id] = "ask";
  }
  return out;
}

export function setGroup(decisions: Record<string, Decision>, tools: Tool[], g: Group, d: Decision): Record<string, Decision> {
  const next = { ...decisions };
  for (const t of tools) if (groupOf(t) === g) next[t.id] = d;
  return next;
}

/** `Allow` count, `Ask` count and whether the group is uniformly one decision (for the group setter's state). */
export function groupState(decisions: Record<string, Decision>, tools: Tool[], g: Group): Decision | "mixed" | null {
  const own = tools.filter((t) => groupOf(t) === g);
  if (own.length === 0) return null;
  const first = decisions[own[0].id] ?? "block";
  return own.every((t) => (decisions[t.id] ?? "block") === first) ? first : "mixed";
}

export function counts(decisions: Record<string, Decision>): { allow: number; ask: number } {
  const v = Object.values(decisions);
  return { allow: v.filter((d) => d === "allow").length, ask: v.filter((d) => d === "ask").length };
}

/** The `PUT` body of a custom grant. `ask_timeout_secs` is sent only with ask tools (the API refuses it otherwise). */
export function customBody(decisions: Record<string, Decision>, timeoutText: string): PutConnectionGrant {
  const allow_tools = Object.entries(decisions).filter(([, d]) => d === "allow").map(([id]) => id);
  const ask_tools = Object.entries(decisions).filter(([, d]) => d === "ask").map(([id]) => id);
  const body: PutConnectionGrant = { mode: "custom", allow_tools, ask_tools };
  const t = parseTimeout(timeoutText);
  if (ask_tools.length > 0 && t !== null && t !== "invalid") body.ask_timeout_secs = t;
  return body;
}

/** Empty = no preference (null); a whole number otherwise; anything else is "invalid". */
export function parseTimeout(text: string): number | null | "invalid" {
  const s = text.trim();
  if (s === "") return null;
  return /^\d+$/.test(s) ? Number(s) : "invalid";
}

/** A client-side hint for the timeout field (the API stays authoritative and may refuse a lower instance maximum). */
export function timeoutProblem(text: string): string | undefined {
  const t = parseTimeout(text);
  if (t === null) return undefined;
  if (t === "invalid" || t < ASK_TIMEOUT_MIN || t > ASK_TIMEOUT_MAX)
    return `Enter a whole number of seconds from ${ASK_TIMEOUT_MIN} to ${ASK_TIMEOUT_MAX}, or leave it empty.`;
  return undefined;
}

export interface GrantState {
  kind: "none" | "full" | "custom";
  label: string;
  allow: number;
  ask: number;
}

export function grantState(grant: Pick<ConnectionGrant, "mode" | "allow_tools" | "ask_tools"> | null | undefined): GrantState {
  if (!grant) return { kind: "none", label: "No access", allow: 0, ask: 0 };
  if (grant.mode === "full") return { kind: "full", label: "Full access", allow: 0, ask: 0 };
  const allow = grant.allow_tools.length;
  const ask = grant.ask_tools.length;
  return { kind: "custom", label: `Custom: ${allow} allowed, ${ask} ask`, allow, ask };
}

export interface GrantErrors {
  /** Per tool id (rows). */
  tools: Record<string, string>;
  /** The ask timeout field. */
  timeout?: string;
  /** Anything not tied to a row or field. */
  general?: string;
  /** A 422 `full_grant_forbidden`: the app is custom-only. */
  customOnly?: boolean;
}

/** Maps a failed save to rows and fields: `grant_invalid` reasons, `approval_timeout_out_of_range`, `full_grant_forbidden`. */
export function mapGrantError(err: unknown, tools: Tool[], decisions: Record<string, Decision>): GrantErrors {
  const out: GrantErrors = { tools: {} };
  if (!(err instanceof ApiError)) return { ...out, general: "Could not save the access. Check your connection and try again." };
  if (err.code === "full_grant_forbidden") return { ...out, customOnly: true };
  if (err.code === "approval_timeout_out_of_range") {
    return { ...out, timeout: fieldErrors(err).ask_timeout_secs ?? err.message };
  }
  if (err.code === "grant_invalid") {
    const reason = (err.details as { reason?: unknown } | null)?.reason;
    if (reason === "empty_custom") return { ...out, general: "Allow or ask at least one tool, or remove this connection's access instead." };
    if (reason === "ask_timeout_without_ask_tools") return { ...out, timeout: "An approval timeout needs at least one tool set to Ask." };
    if (reason === "tool_in_both") {
      return { ...out, general: "A tool cannot be both allowed and set to Ask. Choose one for each tool." };
    }
    if (reason === "unknown_tool") {
      const known = new Set(tools.map((t) => t.id));
      for (const id of Object.keys(decisions)) {
        if (decisions[id] !== "block" && !known.has(id)) out.tools[id] = "This tool is not in the app's catalogue any more.";
      }
      out.general =
        Object.keys(out.tools).length > 0
          ? "Some tools are not in the app's catalogue. Set them to Block and save again."
          : "A tool is not in the app's catalogue. Reload and try again.";
      return out;
    }
  }
  return { ...out, general: err.message };
}
