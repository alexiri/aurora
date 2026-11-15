/**
 * Pure Markdown editing for the composer (P5-CHAT-3, docs/14 "What the buttons insert"): every function takes the
 * text and the selection and returns an `Edit` (replace `[from, to)` with `insert`, then select `[selStart, selEnd]`
 * in the NEW text). The component applies it through `execCommand("insertText")` so the browser's undo keeps working.
 */
export interface Edit {
  from: number;
  to: number;
  insert: string;
  selStart: number;
  selEnd: number;
}

export type InlineAction = "bold" | "italic" | "strike" | "code";
export type LineAction = "heading" | "quote" | "bullet" | "numbered" | "task";
export type FormatAction = InlineAction | LineAction | "link" | "codeblock" | "table";

const MARKS: Record<InlineAction, { mark: string; placeholder: string }> = {
  bold: { mark: "**", placeholder: "bold text" },
  italic: { mark: "_", placeholder: "italic text" },
  strike: { mark: "~~", placeholder: "strikethrough" },
  code: { mark: "`", placeholder: "code" },
};

const LINE_PREFIX: Record<Exclude<LineAction, "numbered">, string> = {
  heading: "### ",
  quote: "> ",
  bullet: "- ",
  task: "- [ ] ",
};

/** Start of the line that holds `i`. */
export function lineStart(text: string, i: number): number {
  return text.lastIndexOf("\n", i - 1) + 1;
}
/** End (exclusive of the newline) of the line that holds `i`. */
export function lineEnd(text: string, i: number): number {
  const n = text.indexOf("\n", i);
  return n < 0 ? text.length : n;
}

function inline(text: string, s: number, e: number, action: InlineAction): Edit {
  const { mark, placeholder } = MARKS[action];
  const sel = text.slice(s, e);
  if (action === "code" && sel.includes("\n")) return codeBlock(text, s, e);
  // Already wrapped, the markers outside the selection or inside it: take them off.
  if (s >= mark.length && text.slice(s - mark.length, s) === mark && text.slice(e, e + mark.length) === mark && e > s) {
    return { from: s - mark.length, to: e + mark.length, insert: sel, selStart: s - mark.length, selEnd: e - mark.length };
  }
  if (sel.length > 2 * mark.length && sel.startsWith(mark) && sel.endsWith(mark)) {
    const inner = sel.slice(mark.length, sel.length - mark.length);
    return { from: s, to: e, insert: inner, selStart: s, selEnd: s + inner.length };
  }
  const body = sel || placeholder;
  return { from: s, to: e, insert: `${mark}${body}${mark}`, selStart: s + mark.length, selEnd: s + mark.length + body.length };
}

function link(text: string, s: number, e: number): Edit {
  const sel = text.slice(s, e);
  if (sel) {
    const at = s + 1 + sel.length + 2;
    return { from: s, to: e, insert: `[${sel}](url)`, selStart: at, selEnd: at + 3 };
  }
  return { from: s, to: e, insert: "[text](url)", selStart: s + 1, selEnd: s + 5 };
}

/** A fenced block on its own lines, the caret (or the selection) inside. */
function codeBlock(text: string, s: number, e: number): Edit {
  const sel = text.slice(s, e);
  const before = s > 0 && text[s - 1] !== "\n" ? "\n" : "";
  const after = e < text.length && text[e] !== "\n" ? "\n" : "";
  const open = `${before}\`\`\`\n`;
  return {
    from: s,
    to: e,
    insert: `${open}${sel}\n\`\`\`${after}`,
    selStart: s + open.length,
    selEnd: s + open.length + sel.length,
  };
}

function table(text: string, s: number, e: number): Edit {
  const before = s > 0 && text[s - 1] !== "\n" ? "\n" : "";
  const after = e < text.length && text[e] !== "\n" ? "\n" : "";
  const head = "| Column 1 | Column 2 |";
  const insert = `${before}${head}\n| --- | --- |\n|  |  |${after}`;
  return { from: s, to: e, insert, selStart: s + before.length + 2, selEnd: s + before.length + 10 };
}

/** Prefix every line the selection touches (`1. `, `2. `… for a numbered list). */
function lines(text: string, s: number, e: number, action: LineAction): Edit {
  const from = lineStart(text, s);
  // A selection that ends at the start of a line does not include that line.
  const to = lineEnd(text, e > s && text[e - 1] === "\n" ? e - 1 : e);
  const rows = text.slice(from, to).split("\n");
  const out = rows.map((r, i) => (action === "numbered" ? `${i + 1}. ` : LINE_PREFIX[action]) + r);
  const insert = out.join("\n");
  return { from, to, insert, selStart: from + insert.length, selEnd: from + insert.length };
}

export function applyFormat(text: string, s: number, e: number, action: FormatAction): Edit {
  switch (action) {
    case "bold":
    case "italic":
    case "strike":
    case "code":
      return inline(text, s, e, action);
    case "link":
      return link(text, s, e);
    case "codeblock":
      return codeBlock(text, s, e);
    case "table":
      return table(text, s, e);
    default:
      return lines(text, s, e, action);
  }
}

/** The shortcut for an action, for tooltips, accessible names and `aria-keyshortcuts` (`Ctrl` is ⌘ on macOS). */
export const SHORTCUTS: Partial<Record<FormatAction, { keys: string[]; aria: string }>> = {
  bold: { keys: ["Ctrl", "B"], aria: "Control+B" },
  italic: { keys: ["Ctrl", "I"], aria: "Control+I" },
  strike: { keys: ["Ctrl", "Shift", "X"], aria: "Control+Shift+X" },
  link: { keys: ["Ctrl", "K"], aria: "Control+K" },
  code: { keys: ["Ctrl", "E"], aria: "Control+E" },
  numbered: { keys: ["Ctrl", "Shift", "7"], aria: "Control+Shift+7" },
  bullet: { keys: ["Ctrl", "Shift", "8"], aria: "Control+Shift+8" },
  quote: { keys: ["Ctrl", "Shift", "."], aria: "Control+Shift+Period" },
};

/** Which action a keydown is, or null. `mod` is Ctrl (or ⌘ on macOS); the physical key is used for the shifted symbols. */
export function shortcutAction(e: { key: string; code?: string; shiftKey: boolean; altKey: boolean }): FormatAction | null {
  if (e.altKey) return null;
  const k = e.key.toLowerCase();
  if (!e.shiftKey) {
    if (k === "b") return "bold";
    if (k === "i") return "italic";
    if (k === "k") return "link";
    if (k === "e") return "code";
    return null;
  }
  if (k === "x") return "strike";
  if (e.code === "Digit7" || k === "7" || k === "&") return "numbered";
  if (e.code === "Digit8" || k === "8" || k === "*") return "bullet";
  if (e.code === "Period" || k === "." || k === ">") return "quote";
  return null;
}

/** True when the caret sits inside an unclosed ``` (or ~~~) fence. */
export function inOpenFence(text: string, caret: number): boolean {
  let open = false;
  for (const line of text.slice(0, caret).split("\n")) {
    if (/^\s{0,3}(```|~~~)/.test(line)) open = !open;
  }
  return open;
}

const LIST_LINE = /^(\s*)([-*+]|\d+[.)])(\s+)(\[[ xX]\]\s+)?(.*)$/;

/**
 * Shift+Enter: on a list item continue the list (`- `, the next number, `- [ ] `); on an empty item remove its
 * marker and end the list. Null when the line is not a list item (the browser inserts the new line).
 */
export function continueList(text: string, caret: number): Edit | null {
  const from = lineStart(text, caret);
  const line = text.slice(from, lineEnd(text, caret));
  const m = LIST_LINE.exec(line);
  if (!m || inOpenFence(text, caret)) return null;
  const [, indent, marker, gap, task, content] = m;
  if (content.trim() === "") {
    return { from, to: from + line.length, insert: "", selStart: from, selEnd: from };
  }
  const n = /^\d+/.exec(marker);
  const next = n ? `${Number(n[0]) + 1}${marker.slice(n[0].length)}` : marker;
  const insert = `\n${indent}${next}${gap}${task ? "[ ] " : ""}`;
  return { from: caret, to: caret, insert, selStart: caret + insert.length, selEnd: caret + insert.length };
}

const URL_RE = /^(https?:\/\/|mailto:)\S+$/i;

/** `|` in a cell would end it. */
function cell(s: string): string {
  return s.trim().replace(/\|/g, "\\|");
}

/** Tab-separated rows as a Markdown table, or null (needs ≥ 2 rows of ≥ 2 cells with the same count). */
export function tsvToTable(clip: string): string | null {
  const rows = clip
    .replace(/\r\n?/g, "\n")
    .replace(/\n+$/, "")
    .split("\n")
    .map((r) => r.split("\t"));
  if (rows.length < 2 || rows[0].length < 2 || rows.some((r) => r.length !== rows[0].length)) return null;
  const row = (r: string[]) => `| ${r.map(cell).join(" | ")} |`;
  return [row(rows[0]), `| ${rows[0].map(() => "---").join(" | ")} |`, ...rows.slice(1).map(row)].join("\n");
}

/** Smart paste (plain text only). Null means: let the browser paste the text. */
export function smartPaste(text: string, s: number, e: number, clip: string): Edit | null {
  const url = clip.trim();
  if (s !== e && URL_RE.test(url) && !text.slice(s, e).includes("\n")) {
    const insert = `[${text.slice(s, e)}](${url})`;
    return { from: s, to: e, insert, selStart: s + insert.length, selEnd: s + insert.length };
  }
  const t = tsvToTable(clip);
  if (t) {
    const before = s > 0 && text[s - 1] !== "\n" ? "\n" : "";
    const after = e < text.length && text[e] !== "\n" ? "\n" : "";
    const insert = `${before}${t}${after}`;
    return { from: s, to: e, insert, selStart: s + insert.length, selEnd: s + insert.length };
  }
  return null;
}

/** The `:xx` being typed at the caret, for the emoji suggestions: not after a letter or digit, not in code. */
export function emojiQuery(text: string, caret: number): { from: number; query: string } | null {
  const lead = lineStart(text, caret);
  const head = text.slice(lead, caret);
  const m = /(^|[^A-Za-z0-9:])(:[A-Za-z0-9_+-]{2,})$/.exec(head);
  if (!m) return null;
  const from = lead + head.length - m[2].length;
  // Inside inline code (an odd number of backticks before) or an open fence: no suggestions.
  if ((text.slice(lead, from).match(/`/g)?.length ?? 0) % 2 === 1 || inOpenFence(text, from)) return null;
  return { from, query: m[2].slice(1).toLowerCase() };
}
