import type { FieldInfo } from "@/lib/api/settings";

/** Seconds in each unit a duration is shown in. */
const UNIT_SECS: Record<string, number> = { seconds: 1, minutes: 60, hours: 3600, days: 86400 };

function plural(n: number, unit: string): string {
  const singular = unit.endsWith("s") ? unit.slice(0, -1) : unit;
  return n === 1 ? `1 ${singular}` : `${n} ${unit}`;
}

/** A number of seconds in the largest whole unit: "15 minutes", "1 hour", "45 seconds". */
export function durationWords(secs: number): string {
  for (const [size, unit] of [
    [86400, "days"],
    [3600, "hours"],
    [60, "minutes"],
  ] as const) {
    if (secs >= size && secs % size === 0) return plural(secs / size, unit);
  }
  return plural(secs, "seconds");
}

function scalarWords(field: FieldInfo | undefined, v: unknown): string {
  if (typeof v === "boolean") return v ? "On" : "Off";
  if (typeof v === "number") {
    if (field?.kind === "duration") return durationWords(v);
    if (field?.unit === "bytes") return bytesWords(v);
    return field?.unit && Number.isInteger(v) ? plural(v, field.unit) : String(v);
  }
  if (typeof v === "string") return field?.choices?.find((c) => c.value === v)?.label ?? v;
  if (typeof v === "object" && v !== null) {
    const o = v as Record<string, unknown>;
    const name = o.host ?? o.id ?? o.class;
    return typeof name === "string" ? name : "An entry";
  }
  return "";
}

/** A setting's value in words, never a raw enumeration id or JSON: "On", "Approved endpoints only", "None". */
export function valueWords(field: FieldInfo | undefined, v: unknown): string {
  if (v === null || v === undefined) return "Not set";
  if (Array.isArray(v)) {
    if (v.length === 0) return "None";
    if (field?.kind === "keyed_list") return `${v.length} configured`;
    return v.map((x) => scalarWords(field, x)).join(", ");
  }
  return scalarWords(field, v);
}

/** The unit's size in seconds (for a duration field), else 1. */
export function unitSecs(field: FieldInfo): number {
  return field.kind === "duration" ? (UNIT_SECS[field.unit ?? "seconds"] ?? 1) : 1;
}

const GB = 1024 ** 3;
const MB = 1024 ** 2;

/** A byte count in the largest whole-ish unit: "5 GB", "512 MB", "100 bytes". */
export function bytesWords(n: number): string {
  const round = (x: number) => String(Math.round(x * 10) / 10);
  if (n >= GB) return `${round(n / GB)} GB`;
  if (n >= MB) return `${round(n / MB)} MB`;
  return plural(n, "bytes");
}

const BYTE_UNITS: Record<string, number> = { bytes: 1, KiB: 1024, MiB: MB, GiB: GB };

/** A byte count in the server's words ("5 GiB", "256 KiB", "100 bytes", `aurora-config` fields.rs) → bytes; else null. */
function bytesOf(text: string): number | null {
  const m = /^(\d+(?:\.\d+)?) ?(bytes|KiB|MiB|GiB)$/.exec(text.trim());
  return m ? Number(m[1]) * BYTE_UNITS[m[2]!]! : null;
}

/** "5 GiB" (the server's default in words) → "5 GB"; any other text is unchanged. */
export function defaultInWords(text: string): string {
  const n = bytesOf(text);
  return n === null ? text : bytesWords(n);
}

/**
 * The unit a number control edits in and the size of that unit in the stored unit: a duration in its `unit`, a byte
 * count in GB or MB by the size of its default (stable while typing), anything else as it is.
 */
export function editUnit(field: FieldInfo): { size: number; label: string | null } {
  if (field.unit === "bytes") {
    const n = bytesOf(field.default_display) ?? 0;
    if (n >= GB) return { size: GB, label: "GB" };
    if (n >= MB) return { size: MB, label: "MB" };
    return { size: 1, label: "bytes" };
  }
  return { size: unitSecs(field), label: field.unit ?? null };
}

/** (P5-REFINE-18 #3, D-341) One change in a sentence, for the confirm dialog and the presets: never a key, an id or a reason code. */
export interface ChangeContext {
  /** The setting's words (C-972); its label names the setting in the generic sentence. */
  field?: FieldInfo;
  /** The entry of a keyed list the change is about (a provider's id). */
  entryId?: string | null;
}

const hostOf = (v: unknown): string | null => {
  if (typeof v === "string") return v;
  if (typeof v === "object" && v !== null && "host" in v && typeof (v as { host: unknown }).host === "string") {
    const o = v as { host: string; path?: unknown };
    return typeof o.path === "string" && o.path ? `${o.host}${o.path}` : o.host;
  }
  return null;
};
const listOf = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const keyOfItem = (x: unknown) => JSON.stringify(x);
/** Entries in `a` that are not in `b`. */
const minus = (a: unknown[], b: unknown[]) => a.filter((x) => !b.some((y) => keyOfItem(x) === keyOfItem(y)));
const joinWords = (xs: string[]) => (xs.length <= 1 ? (xs[0] ?? "") : `${xs.slice(0, -1).join(", ")} and ${xs[xs.length - 1]}`);
const days = (v: unknown, none = "no limit") => (typeof v === "number" ? plural(v, "days") : none);
const mib = (v: unknown, none = "no limit") => (typeof v === "number" ? bytesWords(v * MB) : none);

type Template = (before: unknown, after: unknown) => string;

const SCOPE_WORDS: Record<string, string> = { organization: "organization", workspace: "workspace", user: "personal" };

/** Sentences per setting key; a key without one gets the generic sentence. */
const CHANGE_TEMPLATES: Record<string, Template> = {
  "governance.egress.baseline_allowlist": (b, a) => {
    const added = minus(listOf(a), listOf(b)).map(hostOf).filter((h): h is string => h !== null);
    const removed = minus(listOf(b), listOf(a)).map(hostOf).filter((h): h is string => h !== null);
    const parts: string[] = [];
    if (added.length > 0) parts.push(`Agents everywhere may now reach ${joinWords(added)}.`);
    if (removed.length > 0) parts.push(`Agents everywhere no longer reach ${joinWords(removed)} by default.`);
    return parts.join(" ") || "The websites allowed everywhere changed.";
  },
  "governance.egress.minimum_posture": (_b, a) =>
    a === "credentialed_only" ? "Agents may only reach the allowed websites when they have no account for the site." : "Agents may reach any website, even with no account for it.",
  "governance.egress.open_lane_minimum": (_b, a) =>
    a === "disabled"
      ? "Browser connections are turned off for everyone."
      : a === "allowlist_only"
        ? "Browser connections may reach only the allowed websites."
        : "Browser connections may reach any website.",
  "governance.llm.minimum_posture": (_b, a) =>
    a === "sanctioned_only" ? "Agents may only use AI keys that Aurora provides." : "Agents may now use AI keys they bring themselves.",
  "governance.llm.allowed_key_scopes": (b, a) => {
    const added = minus(listOf(a), listOf(b)).map((s) => SCOPE_WORDS[String(s)] ?? String(s));
    const removed = minus(listOf(b), listOf(a)).map((s) => SCOPE_WORDS[String(s)] ?? String(s));
    const parts: string[] = [];
    if (added.length > 0) parts.push(`AI keys may now be added at the ${joinWords(added)} level.`);
    if (removed.length > 0) parts.push(`AI keys may no longer be added at the ${joinWords(removed)} level.`);
    return parts.join(" ") || "Who may add AI keys changed.";
  },
  "governance.llm.require_key_origin": (_b, a) =>
    a === true ? "Agents may only use AI keys whose origin Aurora can verify." : "Agents may use AI keys even when Aurora cannot verify where they come from.",
  "byo.enabled": (_b, a) => (a === true ? "People may run agents outside Aurora." : "Agents outside Aurora are turned off for everyone."),
  "byo.minimum_posture": (_b, a) =>
    a === "disabled" ? "Nobody may run agents outside Aurora." : a === "owners_only" ? "Only owners may run agents outside Aurora." : "Anyone who can use a workspace may run agents outside Aurora.",
  "governance.agent_tokens.max_age_days": (b, a) =>
    typeof a === "number" ? `Agent tokens now last at most ${days(a)}.` : `Agent tokens no longer have a longest life${typeof b === "number" ? " (it was " + days(b) + ")" : ""}.`,
  "retention.request_log_days": (b, a) => `Request activity is kept ${days(a)} instead of ${days(b)}.`,
  "retention.conversation_days_max": (b, a) => `Organizations may keep conversations ${typeof a === "number" ? "up to " + days(a) : "without a limit"} instead of ${typeof b === "number" ? "up to " + days(b) : "without a limit"}.`,
  "attachments.days_max": (b, a) => `Organizations may keep files ${typeof a === "number" ? "up to " + days(a) : "without a limit"} instead of ${typeof b === "number" ? "up to " + days(b) : "without a limit"}.`,
  "attachments.default_org_quota_mib": (b, a) => `Each organization's file storage starts at ${mib(a)} instead of ${mib(b)}.`,
  "attachments.org_quota_mib_max": (b, a) => `Organizations may have up to ${mib(a)} of files instead of ${mib(b)}.`,
  "terminal.enabled": (_b, a) => (a === true ? "People may open a terminal on their agent's computer." : "The web terminal is turned off."),
  "scim.enabled": (_b, a) => (a === true ? "People and groups may be provisioned from your directory." : "Provisioning from your directory is turned off."),
  "hosted.enabled": (_b, a) => (a === true ? "People may create agents that run on Aurora." : "Agents that run on Aurora cannot be created."),
  "auth.signup_mode": (_b, a) =>
    a === "open" ? "Anyone may sign up." : a === "sso_jit" ? "People who sign in with your identity provider get an account." : "Signing up is by invitation only.",
  "auth.org_creation": (_b, a) => (a === "anyone" ? "Anyone may create an organization." : "Only administrators may create an organization."),
};

/** The setting's last segment as words: "gateway.condition_body_bytes" → "Condition body bytes". Only when no field label is known. */
function humanise(key: string): string {
  const last = key.split(".").pop() ?? key;
  const w = last.replace(/_/g, " ").trim();
  return w.charAt(0).toUpperCase() + w.slice(1);
}

/**
 * One change as a sentence from the setting's key and its value before and after (D-341 labels, `valueWords`). The key
 * itself never appears: a key with no template gets "<Label> changes from <old> to <new>."
 */
export function describeChange(key: string, before: unknown, after: unknown, ctx: ChangeContext = {}): string {
  const label = ctx.field?.label ?? humanise(key);
  // The preview named the change but not its values: say that much, never "from Not set to Not set".
  const noValues = before === undefined && after === undefined;
  if (noValues && !ctx.entryId) return `${label} now allows more than before.`;
  const t = noValues ? undefined : CHANGE_TEMPLATES[key];
  if (t) return t(before, after);
  // An entry of a list (a sign-in provider, an endpoint): the whole entry changed, so say which, not its fields.
  if (ctx.entryId) return `${label}: ${ctx.entryId} now allows more than before.`;
  return `${label} changes from ${valueWords(ctx.field, before)} to ${valueWords(ctx.field, after)}.`;
}
