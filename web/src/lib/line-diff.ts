/**
 * The pure line diff and its folding model (FIX-250): a GitLab-style diff shows every change with
 * three lines of context and folds the longer runs of unchanged lines into one row that expands in
 * steps. Rendering is the `DiffView` component's (web/src/components/ui/diff-view.tsx); everything
 * here is data and pure functions, so the folding can be tested without a component.
 */

/** Unchanged lines shown around every change before the folding starts. */
export const DIFF_CONTEXT = 3;
/** How much **Show 20 more above / below** reveals per click. */
export const EXPAND_STEP = 20;
/** Above this many lines a file counts as very large (the diff says so and stays folded). */
export const VERY_LARGE_LINES = 5_000;

export type DiffLineKind = "same" | "add" | "del";

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
  /** 1-based line number in the old file; null for added lines. */
  old: number | null;
  /** 1-based line number in the new file; null for removed lines. */
  new: number | null;
}

/** One folded run of unchanged lines. The revealed slice is `[above, count - below)` of `lines`. */
export interface FoldRow {
  type: "fold";
  id: number;
  /** How many unchanged lines the run holds in total. */
  count: number;
  /** Revealed from the top / bottom of the run, in lines. */
  above: number;
  below: number;
  /** The run's lines, in file order (all `same`). */
  lines: DiffLine[];
}

export type DiffRow = { type: "line"; line: DiffLine } | FoldRow;

export interface LineDiff {
  rows: DiffRow[];
  /** The new file's line count. */
  lines: number;
  /** The new file has more than `VERY_LARGE_LINES` lines. */
  veryLarge: boolean;
}

/** The line diff of two texts: every line, marked, with old and new line numbers (1-based). */
export function diffLines(oldText: string, newText: string): DiffLine[] {
  const split = (t: string) => (t === "" ? [] : t.split("\n"));
  const a0 = split(oldText);
  const b0 = split(newText);
  // Lines both versions share at the start and the end are identical context: only the middle needs the table.
  let head = 0;
  while (head < a0.length && head < b0.length && a0[head] === b0[head]) head++;
  let tail = 0;
  while (tail < a0.length - head && tail < b0.length - head && a0[a0.length - 1 - tail] === b0[b0.length - 1 - tail]) tail++;
  const a = a0.slice(head, a0.length - tail);
  const b = b0.slice(head, b0.length - tail);
  const out: DiffLine[] = [];
  let oldNo = 0;
  let newNo = 0;
  const push = (kind: DiffLineKind, text: string) =>
    out.push({ kind, text, old: kind === "add" ? null : ++oldNo, new: kind === "del" ? null : ++newNo });
  for (let k = 0; k < head; k++) push("same", a0[k]);
  // Longest common subsequence of the middle; past a quadratic-table size the middle falls back to
  // "all removed, then all added" (skills are at most 32 000 characters, but a file may not be).
  if (a.length * b.length > 4_000_000) {
    for (const text of a) push("del", text);
    for (const text of b) push("add", text);
  } else {
    const dp: number[][] = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
    for (let i = a.length - 1; i >= 0; i--)
      for (let j = b.length - 1; j >= 0; j--) dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    let i = 0;
    let j = 0;
    while (i < a.length && j < b.length) {
      if (a[i] === b[j]) {
        push("same", a[i]);
        i++;
        j++;
      } else if (dp[i + 1][j] >= dp[i][j + 1]) push("del", a[i++]);
      else push("add", b[j++]);
    }
    while (i < a.length) push("del", a[i++]);
    while (j < b.length) push("add", b[j++]);
  }
  for (let k = a0.length - tail; k < a0.length; k++) push("same", a0[k]);
  return out;
}

/**
 * The diff with its folding: every change keeps `DIFF_CONTEXT` unchanged lines around it; runs of
 * changes whose context meets (six or fewer unchanged lines between them) are one hunk; everything
 * longer folds into one `FoldRow` per run, collapsed to the "N unchanged lines" row.
 */
export function buildDiff(oldText: string, newText: string): LineDiff {
  const lines = diffLines(oldText, newText);
  const visible = new Array<boolean>(lines.length).fill(false);
  let i = 0;
  while (i < lines.length) {
    if (lines[i].kind === "same") {
      i++;
      continue;
    }
    let j = i;
    while (j < lines.length && lines[j].kind !== "same") j++;
    for (let k = Math.max(0, i - DIFF_CONTEXT); k < j + DIFF_CONTEXT && k < lines.length; k++) visible[k] = true;
    i = j;
  }
  const rows: DiffRow[] = [];
  let hidden: DiffLine[] = [];
  let id = 0;
  const flush = () => {
    if (hidden.length > 0) {
      rows.push({ type: "fold", id: id++, count: hidden.length, above: 0, below: 0, lines: hidden });
      hidden = [];
    }
  };
  for (const [k, line] of lines.entries()) {
    if (visible[k]) {
      flush();
      rows.push({ type: "line", line });
    } else {
      hidden.push(line);
    }
  }
  flush();
  const newCount = newText === "" ? 0 : newText.split("\n").length;
  return { rows, lines: newCount, veryLarge: newCount > VERY_LARGE_LINES };
}

function rowOf(rows: DiffRow[], id: number): FoldRow | null {
  return rows.find((r) => r.type === "fold" && r.id === id) as FoldRow | null;
}

/** Expanding only reshapes the fold's own row; its place in the file never moves. */
function replaceFold(rows: DiffRow[], fold: FoldRow, replacement: DiffRow[]): DiffRow[] {
  const at = rows.findIndex((r) => r.type === "fold" && r.id === fold.id);
  return [...rows.slice(0, at), ...replacement, ...rows.slice(at + 1)];
}

const foldLines = (fold: FoldRow): DiffRow[] => fold.lines.map((line) => ({ type: "line" as const, line }));

/** **Show 20 more above**: reveals the next `step` lines from the top of the fold's hidden run. */
export function expandAbove(rows: DiffRow[], id: number, step = EXPAND_STEP): DiffRow[] {
  const fold = rowOf(rows, id);
  if (!fold) return rows;
  const above = Math.min(fold.above + step, fold.count - fold.below);
  const next = { ...fold, above };
  return above + fold.below >= fold.count ? replaceFold(rows, fold, foldLines(next)) : replaceFold(rows, fold, [next]);
}

/** **Show 20 more below**: reveals the next `step` lines from the bottom of the fold's hidden run. */
export function expandBelow(rows: DiffRow[], id: number, step = EXPAND_STEP): DiffRow[] {
  const fold = rowOf(rows, id);
  if (!fold) return rows;
  const below = Math.min(fold.below + step, fold.count - fold.above);
  const next = { ...fold, below };
  return fold.above + below >= fold.count ? replaceFold(rows, fold, foldLines(next)) : replaceFold(rows, fold, [next]);
}

/** **Show all** on one fold: its whole run becomes ordinary lines. */
export function expandFold(rows: DiffRow[], id: number): DiffRow[] {
  const fold = rowOf(rows, id);
  return fold ? replaceFold(rows, fold, foldLines(fold)) : rows;
}

/** **Show the whole file**: every fold expands, nothing stays hidden. */
export function expandAll(rows: DiffRow[]): DiffRow[] {
  const out: DiffRow[] = [];
  for (const row of rows) out.push(...(row.type === "fold" ? foldLines(row) : [row]));
  return out;
}

/** The lines a fold still hides, with their new-file line numbers (for the buttons' labels). */
export function hiddenRange(fold: FoldRow): { from: number; to: number } | null {
  const visible = fold.count - fold.above - fold.below;
  if (visible <= 0) return null;
  const from = fold.lines[fold.above]?.new ?? 0;
  const to = from + visible - 1;
  return { from, to };
}

// ---- word-level highlight --------------------------------------------------------------------

export interface WordSpan {
  text: string;
  changed: boolean;
}

const tokens = (line: string): string[] => line.match(/\s+|\w+|[^\s\w]+/g) ?? [];

/**
 * Which words differ between a removed line and the added line replacing it: each side as spans of
 * unchanged and changed text, for the DiffView's word-level highlight. Whole-line differences stay
 * whole-line: a pair with no shared word marks both lines entirely.
 */
export function wordSpans(oldLine: string, newLine: string): { old: WordSpan[]; new: WordSpan[] } {
  const a = tokens(oldLine);
  const b = tokens(newLine);
  if (a.length * b.length > 1_000_000) {
    return {
      old: oldLine === "" ? [] : [{ text: oldLine, changed: true }],
      new: newLine === "" ? [] : [{ text: newLine, changed: true }],
    };
  }
  const dp: number[][] = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i--)
    for (let j = b.length - 1; j >= 0; j--) dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
  const changedA = new Array<boolean>(a.length).fill(true);
  const changedB = new Array<boolean>(b.length).fill(true);
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      changedA[i] = false;
      changedB[j] = false;
      i++;
      j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) i++;
    else j++;
  }
  const spans = (tokens: string[], changed: boolean[]): WordSpan[] => {
    const out: WordSpan[] = [];
    for (const [k, text] of tokens.entries()) {
      const last = out.at(-1);
      if (last && last.changed === changed[k]) last.text += text;
      else out.push({ text, changed: changed[k] });
    }
    return out;
  };
  return { old: spans(a, changedA), new: spans(b, changedB) };
}
