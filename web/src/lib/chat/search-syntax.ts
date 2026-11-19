// (P5-CHAT-4 part B, D-229, C-727; docs/14 §Message search) The search box's syntax and how it becomes parameters. The
// API gets parameters, never the raw syntax: `from:`/`has:`/`after:`/`before:`/`on:` become chips, `in:` is chosen from
// a list of conversation titles, and everything else is `q` (an unknown filter stays a word of `q`).

export type FilterChip =
  | { kind: "from"; value: "me" | "agent" | "others" }
  | { kind: "has"; value: "file" }
  | { kind: "in"; value: string; label: string }
  | { kind: "after" | "before" | "on"; value: string };

export type ChipKind = FilterChip["kind"];

export const FROM_VALUES = ["me", "agent", "others"] as const;
const DATE_KINDS = ["after", "before", "on"] as const;

/** `YYYY-MM-DD` of a real calendar day → that day's local midnight; otherwise `null` (no rolling `02-30` into March). */
export function localDayStart(day: string): Date | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
  if (!m) return null;
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  const date = new Date(y, mo - 1, d);
  return date.getFullYear() === y && date.getMonth() === mo - 1 && date.getDate() === d ? date : null;
}

/** The chip's text, for display and for its remove button's name. */
export function chipLabel(c: FilterChip): string {
  switch (c.kind) {
    case "from":
      return `From: ${c.value}`;
    case "has":
      return `Has: ${c.value}`;
    case "in":
      return `In: ${c.label}`;
    case "after":
      return `After: ${c.value}`;
    case "before":
      return `Before: ${c.value}`;
    case "on":
      return `On: ${c.value}`;
  }
}

/** A chip replaces another of its kind; `on` and `after`/`before` exclude each other (one day vs a range). */
export function addChip(chips: FilterChip[], chip: FilterChip): FilterChip[] {
  const drop = (k: ChipKind) =>
    chip.kind === k || (chip.kind === "on" && (k === "after" || k === "before")) || ((chip.kind === "after" || chip.kind === "before") && k === "on");
  return [...chips.filter((c) => !drop(c.kind)), chip];
}

interface Token {
  text: string;
  start: number;
  end: number;
  /** Followed by whitespace, so the person has finished it. */
  complete: boolean;
}

/** Whitespace-separated tokens; a `"quoted phrase"` is one token, and an unclosed quote runs to the end (never complete). */
export function tokenize(text: string): Token[] {
  const out: Token[] = [];
  let i = 0;
  while (i < text.length) {
    while (i < text.length && /\s/.test(text[i]!)) i += 1;
    if (i >= text.length) break;
    const start = i;
    let quoted = false;
    while (i < text.length && (quoted || !/\s/.test(text[i]!))) {
      if (text[i] === '"') quoted = !quoted;
      i += 1;
    }
    out.push({ text: text.slice(start, i), start, end: i, complete: !quoted && i < text.length });
  }
  return out;
}

/** The chip a `name:value` token stands for, or `null` when it is not a (valid) filter. `in:` is never typed through. */
export function chipOfToken(token: string): FilterChip | null {
  const m = /^([a-z]+):(.+)$/i.exec(token);
  if (!m) return null;
  const name = m[1]!.toLowerCase();
  const value = m[2]!;
  if (name === "from") {
    const v = value.toLowerCase();
    return (FROM_VALUES as readonly string[]).includes(v) ? { kind: "from", value: v as "me" | "agent" | "others" } : null;
  }
  if (name === "has") return value.toLowerCase() === "file" ? { kind: "has", value: "file" } : null;
  if ((DATE_KINDS as readonly string[]).includes(name)) return localDayStart(value) ? { kind: name as "after" | "before" | "on", value } : null;
  return null;
}

/**
 * Moves the finished filter tokens out of `text` into chips. With `final`, the last token counts as finished too (Enter).
 * `rest` is the text without them (the other words keep their spacing, with one space left between neighbours).
 */
export function extractFilters(text: string, chips: FilterChip[], final = false): { rest: string; chips: FilterChip[] } {
  let next = chips;
  let rest = "";
  let cursor = 0;
  for (const t of tokenize(text)) {
    const chip = t.complete || final ? chipOfToken(t.text) : null;
    if (!chip) continue;
    next = addChip(next, chip);
    rest += text.slice(cursor, t.start);
    cursor = t.end;
    // swallow the whitespace after the token so removing it leaves no double gap
    while (cursor < text.length && /\s/.test(text[cursor]!)) cursor += 1;
  }
  rest += text.slice(cursor);
  return { rest, chips: next };
}

/** The partial `in:` token being typed at the end of the box (without its prefix), or `null`. */
export function inTokenAtEnd(text: string): string | null {
  const last = tokenize(text).at(-1);
  if (!last || last.complete || last.end !== text.length) return null;
  const m = /^in:(.*)$/i.exec(last.text);
  return m ? m[1]!.replace(/^"/, "").replace(/"$/, "") : null;
}

/** Removes the partial `in:` token at the end of `text`. */
export function withoutInToken(text: string): string {
  const last = tokenize(text).at(-1);
  return last && /^in:/i.test(last.text) ? text.slice(0, last.start).trimEnd() : text;
}

/** The text with the whole words collapsed: what is sent as `q` (the API trims too, this keeps the count honest). */
export const queryOf = (text: string): string => text.trim().replace(/\s+/g, " ");

export interface SearchParams {
  q: string;
  agent_id?: string;
  conversation_id?: string;
  author?: "me" | "agent" | "others";
  has?: "file";
  after?: string;
  before?: string;
  sort: "recent" | "relevance";
}

/** Local midnight as an RFC 3339 instant, so "a day" means the reader's day, not UTC's. */
const instant = (day: string, plusDays = 0): string | undefined => {
  const d = localDayStart(day);
  if (!d) return undefined;
  d.setDate(d.getDate() + plusDays);
  return d.toISOString();
};

/** The request parameters for a query, its chips, the scope (`agentId` = this agent only) and the sort. */
export function toSearchParams(q: string, chips: FilterChip[], opts: { agentId?: string; sort: "recent" | "relevance" }): SearchParams {
  const p: SearchParams = { q, sort: opts.sort };
  if (opts.agentId) p.agent_id = opts.agentId;
  for (const c of chips) {
    if (c.kind === "from") p.author = c.value;
    else if (c.kind === "has") p.has = c.value;
    else if (c.kind === "in") p.conversation_id = c.value;
    else if (c.kind === "after") p.after = instant(c.value);
    else if (c.kind === "before") p.before = instant(c.value);
    else {
      p.after = instant(c.value);
      p.before = instant(c.value, 1);
    }
  }
  return p;
}
