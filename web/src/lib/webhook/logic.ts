// P5-WEBHOOK-4 (C-913): the pure parts of the outgoing-webhook editor, with no React and no network. The server is
// authoritative (C-909); everything here only saves the person a round trip.
import { ApiError } from "@/lib/api/client";
import type { ContentType, Slot, SlotType, WebhookConfig, WebhookPreset } from "@/lib/api/webhook";

export const LIMITS = { template: 16 * 1024, slots: 20, secrets: 4, headers: 4, description: 1000, slotDescription: 300, secretValue: 4096 } as const;

export const CONTENT_TYPES: { value: ContentType; label: string }[] = [
  { value: "application/json", label: "JSON" },
  { value: "application/x-www-form-urlencoded", label: "Form fields" },
  { value: "text/plain", label: "Plain text" },
];

export const SLOT_TYPES: { value: SlotType; label: string }[] = [
  { value: "string", label: "Text" },
  { value: "integer", label: "Whole number" },
  { value: "number", label: "Number" },
  { value: "boolean", label: "Yes or no" },
  { value: "string_list", label: "List of text" },
  { value: "enum", label: "One of a list" },
  { value: "json", label: "Any JSON" },
];

export function emptyConfig(): WebhookConfig {
  return {
    version: 1,
    method: "POST",
    content_type: "application/json",
    template: "",
    slots: [],
    headers: [],
    description: "",
    signing: false,
    rate: { per_minute: null, per_day: null },
    preset: null,
  };
}

// ---- Presets ----

/** The host of a URL a person is typing: "" while it is not a URL yet. */
export function hostOfUrl(input: string): string {
  const t = input.trim();
  if (!t) return "";
  try {
    return new URL(t.includes("://") ? t : `https://${t}`).hostname.toLowerCase();
  } catch {
    return "";
  }
}

/** The preset a URL's host suggests (`suggest_for_hosts`), or null. A suggestion only: any URL works with any preset. */
export function suggestPreset(presets: WebhookPreset[], url: string): WebhookPreset | null {
  const host = hostOfUrl(url);
  if (!host) return null;
  return presets.find((p) => (p.suggest_for_hosts ?? []).some((h) => h.toLowerCase() === host)) ?? null;
}

/** A preset's configuration as a fresh copy, with the preset's id kept so the card can say where it began. */
export function configFromPreset(preset: WebhookPreset, keep?: Pick<WebhookConfig, "description" | "rate" | "signing" | "method" | "headers">): WebhookConfig {
  const c = structuredClone(preset.config);
  c.preset = preset.id;
  if (keep) {
    c.description = keep.description || c.description;
    c.rate = keep.rate;
    c.signing = keep.signing;
    c.headers = keep.headers.length > 0 ? keep.headers : c.headers;
    c.method = keep.method;
  }
  return c;
}

// ---- The example picker ----

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/** What the person set on a clicked value. The path (a JSON pointer, "" for the root) is the key of `Picks`. */
export interface SlotPick {
  name: string;
  description: string;
  required: boolean;
  max_length?: number;
  max_items?: number;
}
export type Picks = Record<string, SlotPick>;

export function parseExample(text: string): { ok: true; value: Json } | { ok: false; message: string } {
  if (!text.trim()) return { ok: false, message: "Paste the example the receiver documents." };
  try {
    return { ok: true, value: JSON.parse(text) as Json };
  } catch {
    return { ok: false, message: "That is not valid JSON. Paste the example body exactly as the receiver documents it." };
  }
}

const isObject = (v: Json): v is { [key: string]: Json } => typeof v === "object" && v !== null && !Array.isArray(v);

/** A value can become a slot unless it is `null` (nothing to fill in). */
export const isPickable = (v: Json): boolean => v !== null;

/** The slot type a value suggests: a string list for an all-text array, `json` for any other array or an object. */
export function inferType(v: Json): SlotType {
  if (typeof v === "string") return "string";
  if (typeof v === "boolean") return "boolean";
  if (typeof v === "number") return Number.isInteger(v) ? "integer" : "number";
  if (Array.isArray(v)) return v.every((x) => typeof x === "string") ? "string_list" : "json";
  return "json";
}

/** A key as a valid, unique slot name (`^[a-z][a-z0-9_]{0,39}$`): `iconEmoji` → `icon_emoji`, `1st` → `v_1st`, a clash → `_2`. */
export function slotNameFrom(key: string, taken: ReadonlySet<string>): string {
  let base = key
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "");
  if (!base) base = "value";
  if (!/^[a-z]/.test(base)) base = `v_${base}`;
  base = base.slice(0, 40).replace(/_+$/, "");
  if (!taken.has(base)) return base;
  for (let n = 2; ; n++) {
    const suffix = `_${n}`;
    const cand = `${base.slice(0, 40 - suffix.length)}${suffix}`;
    if (!taken.has(cand)) return cand;
  }
}

export const childPath = (path: string, key: string | number) => `${path}/${String(key).replace(/~/g, "~0").replace(/\//g, "~1")}`;

/** Make the value at `path` a slot: named from `key`, unique among the picks; picks beneath it are dropped. */
export function pickAt(picks: Picks, path: string, key: string): Picks {
  const next: Picks = {};
  for (const [p, v] of Object.entries(picks)) if (!(p === path || p.startsWith(`${path}/`))) next[p] = v;
  const taken = new Set(Object.values(next).map((p) => p.name));
  next[path] = { name: slotNameFrom(key, taken), description: "", required: true };
  return next;
}

/** Make it fixed again: the constant returns to the template. */
export function unpickAt(picks: Picks, path: string): Picks {
  const next = { ...picks };
  delete next[path];
  return next;
}

/** The nearest picked ancestor of `path` (a value under a slot is part of it), or null. */
export function coveredBy(picks: Picks, path: string): string | null {
  for (const p of Object.keys(picks)) if (p !== path && (p === "" || path.startsWith(`${p}/`))) return p;
  return null;
}

/** The template and slots an example and its picks make, in document order. Everything not picked stays a constant. */
export function buildFromExample(root: Json, picks: Picks): { template: string; slots: Slot[] } {
  const slots: Slot[] = [];
  const emit = (v: Json, path: string, depth: number): string => {
    const pick = picks[path];
    if (pick) {
      const type = inferType(v);
      const slot: Slot = { name: pick.name, type, required: path === "" ? true : pick.required, description: pick.description, example: v };
      if (type === "string" && pick.max_length) slot.max_length = pick.max_length;
      if (type === "string_list") {
        if (pick.max_length) slot.max_length = pick.max_length;
        if (pick.max_items) slot.max_items = pick.max_items;
      }
      slots.push(slot);
      return `{{${pick.name}}}`;
    }
    const pad = "  ".repeat(depth);
    if (Array.isArray(v)) {
      if (v.length === 0) return "[]";
      return `[\n${v.map((x, i) => `${pad}  ${emit(x, childPath(path, i), depth + 1)}`).join(",\n")}\n${pad}]`;
    }
    if (isObject(v)) {
      const keys = Object.keys(v);
      if (keys.length === 0) return "{}";
      return `{\n${keys.map((k) => `${pad}  ${JSON.stringify(k)}: ${emit(v[k]!, childPath(path, k), depth + 1)}`).join(",\n")}\n${pad}}`;
    }
    return JSON.stringify(v);
  };
  const template = emit(root, "", 0);
  return { template, slots };
}

/** Edit a pick's fields (description, required, limits, name). */
export function editPick(picks: Picks, path: string, patch: Partial<SlotPick>): Picks {
  const cur = picks[path];
  if (!cur) return picks;
  return { ...picks, [path]: { ...cur, ...patch } };
}

// ---- Placeholders and the quick checks ----

const PLACEHOLDER = /\{\{\s*(secret:)?([a-z][a-z0-9_]*)\s*\}\}/g;

export function placeholders(template: string): { slots: Set<string>; secrets: Set<string> } {
  const slots = new Set<string>();
  const secrets = new Set<string>();
  for (const m of template.matchAll(PLACEHOLDER)) (m[1] ? secrets : slots).add(m[2]!);
  return { slots, secrets };
}

export interface Issue {
  /** Dotted path of the control: `template`, `slots.2.name`, `headers.0.value`, `rate.per_minute`, … */
  path: string;
  code: string;
  message: string;
}

/** C-909's validation codes in words. */
export const CODE_WORDS: Record<string, string> = {
  template_json: "The template is not valid JSON once the blanks are filled in. Check the commas, quotes and brackets.",
  template_syntax: "A blank is not written correctly. Write {{name}} for a value the agent fills in and {{secret:name}} for a secret value.",
  placeholder_unknown: "The template uses a blank that is not in the table below. Add it there, or change the template.",
  slot_unused: "No blank in the template uses this value. Use it in the template, or remove it.",
  secret_unused: "No blank or header uses this secret value. Use {{secret:name}} where it belongs, or remove it.",
  slot_name: "Use a lowercase name of up to 40 letters, digits and underscores, starting with a letter.",
  slot_duplicate: "Two values have this name. Names must be different.",
  slot_type_content_type: "This kind of value cannot be used in this kind of body. Choose another type, or another content type.",
  slot_limits: "These limits do not fit this type of value.",
  optional_slot_at_root: "A value that makes up the whole body must be required.",
  schema_unsupported: "The schema uses a keyword that is not supported. Use type, properties, required, items, enum, maxLength, minimum, maximum or additionalProperties.",
  header_name_denied: "Aurora sets this header itself. Choose another name.",
  header_placeholder: "A header may use a secret value, {{secret:name}}, but not a value the agent fills in.",
  rate_above_ceiling: "This is above the limit your organization or administrator allows.",
  too_large: "This is larger than allowed.",
};

export const INVALID_SLOT_WORDS: Record<string, string> = {
  missing: "This value is required.",
  unknown: "The template has no such value.",
  type: "This is not the right kind of value.",
  max_length: "This is longer than the limit.",
  max_items: "There are more items than the limit.",
  enum: "Choose one of the listed values.",
  schema: "This does not match the schema.",
};

const SLOT_NAME = /^[a-z][a-z0-9_]{0,39}$/;

/** `/slots/2/name`, `webhook/slots/2/name` or `slots[2].name` → `slots.2.name`. */
export function normalizePath(path: string): string {
  return path
    .replace(/\[(\d+)\]/g, ".$1")
    .replace(/\//g, ".")
    .replace(/^\.+/, "")
    .replace(/^webhook\./, "");
}

/** The checks the browser can make with no server: unknown and unused blanks, names. Same codes and paths as the server's. */
export function quickIssues(config: WebhookConfig, secretNames: string[]): Issue[] {
  const out: Issue[] = [];
  const add = (path: string, code: string) => out.push({ path, code, message: CODE_WORDS[code] ?? "Check this field." });
  const used = placeholders(config.template);
  const headerSecrets = new Set<string>();
  config.headers.forEach((h, i) => {
    const p = placeholders(h.value);
    if (p.slots.size > 0) add(`headers.${i}.value`, "header_placeholder");
    for (const s of p.secrets) headerSecrets.add(s);
  });
  const names = new Set<string>();
  config.slots.forEach((s, i) => {
    if (!SLOT_NAME.test(s.name)) add(`slots.${i}.name`, "slot_name");
    else if (names.has(s.name)) add(`slots.${i}.name`, "slot_duplicate");
    names.add(s.name);
    if (SLOT_NAME.test(s.name) && !used.slots.has(s.name)) add(`slots.${i}`, "slot_unused");
  });
  for (const u of used.slots) if (!names.has(u)) add("template", "placeholder_unknown");
  const known = new Set(secretNames);
  const missingSecret = "A blank uses a secret value that is not in the list of secret values. Add it under Extras, or change the template.";
  for (const u of used.secrets) if (!known.has(u)) out.push({ path: "template", code: "placeholder_unknown", message: missingSecret });
  config.headers.forEach((h, i) => {
    for (const u of placeholders(h.value).secrets) if (!known.has(u)) out.push({ path: `headers.${i}.value`, code: "placeholder_unknown", message: missingSecret });
  });
  secretNames.forEach((n) => {
    if (!used.secrets.has(n) && !headerSecrets.has(n)) add(`secret_values.${n}`, "secret_unused");
  });
  if (config.template.length > LIMITS.template) add("template", "too_large");
  return out;
}

/** `webhook_config_invalid.fields[{ path, code }]` as issues, in words (C-909). Empty for any other error. */
export function configIssues(err: unknown): Issue[] {
  if (!(err instanceof ApiError) || err.code !== "webhook_config_invalid") return [];
  const fields = (err.details as { fields?: unknown } | null)?.fields;
  if (!Array.isArray(fields)) return [];
  const out: Issue[] = [];
  for (const f of fields) {
    if (typeof f !== "object" || f === null) continue;
    const { path, code } = f as { path?: unknown; code?: unknown };
    if (typeof code !== "string") continue;
    out.push({ path: typeof path === "string" ? normalizePath(path) : "", code, message: CODE_WORDS[code] ?? "Check this field." });
  }
  return out;
}

/** 422 `invalid_slot { slot, rule }` (preview, Send test): which value and why. */
export function invalidSlot(err: unknown): { slot: string; message: string } | null {
  if (!(err instanceof ApiError) || err.code !== "invalid_slot") return null;
  const d = (err.details ?? {}) as { slot?: unknown; rule?: unknown };
  if (typeof d.slot !== "string") return null;
  return { slot: d.slot, message: INVALID_SLOT_WORDS[String(d.rule)] ?? "This value is not accepted." };
}

/** The issues for a control: its own path and everything beneath it. */
export const issuesAt = (issues: Issue[], path: string) => issues.filter((i) => i.path === path || i.path.startsWith(`${path}.`));
export const firstMessage = (issues: Issue[], path: string) => issuesAt(issues, path)[0]?.message;

/** Top-level areas the editor has a control for; an issue with any other path goes in the general notice. */
const AREAS = new Set(["template", "slots", "headers", "rate", "description", "secret_values", "method", "content_type", "signing"]);
export const unmatchedIssues = (issues: Issue[]) => issues.filter((i) => !AREAS.has(i.path.split(".")[0] ?? ""));

/** The sentences for the connect request's own refusals (C-909), for the URL field or the dialog. */
export function connectRefusal(err: unknown): { field: "url" | null; message: string } | null {
  if (!(err instanceof ApiError)) return null;
  switch (err.code) {
    case "link_not_https":
      return { field: "url", message: "The address must start with https://." };
    case "link_invalid":
      return { field: "url", message: "That is not a usable web address. Paste the whole URL the receiver gave you." };
    case "link_destination_refused":
      return { field: "url", message: "Aurora may not send to that address. Ask an administrator if it should be allowed." };
    case "outgoing_webhooks_disabled":
      return { field: null, message: "Outgoing webhooks are turned off for this organization. An owner or admin can turn them on." };
    default:
      return null;
  }
}

// ---- Sample values and the values form ----

/** Text inputs for each slot, filled from its example: what the preview and Send test forms start with. */
export function initialInputs(slots: Slot[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const s of slots) {
    const ex = s.example;
    if (s.type === "boolean") out[s.name] = ex === true ? "true" : "false";
    else if (s.type === "string_list") out[s.name] = Array.isArray(ex) ? ex.join("\n") : "";
    else if (s.type === "json") out[s.name] = ex === undefined ? "" : JSON.stringify(ex, null, 2);
    else if (s.type === "enum") out[s.name] = typeof ex === "string" ? ex : (s.enum?.[0] ?? "");
    else out[s.name] = ex === undefined || ex === null ? "" : String(ex);
  }
  return out;
}

/** Inputs → the JSON values of the request. A blank optional value is left out; a blank required one is sent for the server to name. */
export function valuesFromInputs(slots: Slot[], inputs: Record<string, string>): { values: Record<string, unknown>; errors: Record<string, string> } {
  const values: Record<string, unknown> = {};
  const errors: Record<string, string> = {};
  for (const s of slots) {
    const raw = inputs[s.name] ?? "";
    if (s.type === "boolean") {
      values[s.name] = raw === "true";
      continue;
    }
    if (raw.trim() === "" && s.type !== "string") {
      if (s.required) errors[s.name] = INVALID_SLOT_WORDS.missing!;
      continue;
    }
    if (raw === "" && !s.required) continue;
    switch (s.type) {
      case "integer":
      case "number": {
        const n = Number(raw);
        if (!Number.isFinite(n) || (s.type === "integer" && !Number.isInteger(n))) errors[s.name] = s.type === "integer" ? "Enter a whole number." : "Enter a number.";
        else values[s.name] = n;
        break;
      }
      case "string_list":
        values[s.name] = raw.split("\n").map((x) => x.trim()).filter(Boolean);
        break;
      case "json":
        try {
          values[s.name] = JSON.parse(raw) as unknown;
        } catch {
          errors[s.name] = "That is not valid JSON.";
        }
        break;
      default:
        values[s.name] = raw;
    }
  }
  return { values, errors };
}

// ---- Limits ----

/** "" → null (the ceiling); a whole number of 1 or more → itself; anything else → "invalid". */
export function parseRate(v: string): number | null | "invalid" {
  const t = v.trim();
  if (t === "") return null;
  if (!/^\d+$/.test(t)) return "invalid";
  const n = Number(t);
  return n >= 1 ? n : "invalid";
}

export const SOURCE_WORDS = { instance: "the instance's limit", org: "your organization's limit", connection: "this webhook's own limit" } as const;

/** The card's line: "30 a minute, 500 a day, set by your organization". */
export function effectiveLimitSentence(rate: { per_minute: number; per_day: number; source: keyof typeof SOURCE_WORDS }): string {
  return `${rate.per_minute} a minute, ${rate.per_day} a day (${SOURCE_WORDS[rate.source]})`;
}
