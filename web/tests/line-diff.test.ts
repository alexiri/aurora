import { describe, expect, it } from "vitest";

import {
  DIFF_CONTEXT,
  EXPAND_STEP,
  buildDiff,
  diffLines,
  expandAbove,
  expandAll,
  expandBelow,
  expandFold,
  hiddenRange,
  wordSpans,
  type DiffLine,
  type DiffRow,
} from "@/lib/line-diff";

// FIX-250: the pure line diff and its folding model — three lines of context, hunks closer than six
// lines merge, expanding in steps of 20 and all at once, added and removed files, identical files,
// CRLF, a missing trailing newline, a very large input that stays folded, and the word-level
// highlight spans. Rendering is DiffView's; everything here is data.

const kinds = (rows: DiffRow[]): string[] => rows.map((r) => (r.type === "fold" ? `fold:${r.count}` : r.line.kind));
const folds = (rows: DiffRow[]) => rows.filter((r): r is Extract<DiffRow, { type: "fold" }> => r.type === "fold");
const lineText = (rows: DiffRow[]): string[] => rows.filter((r) => r.type === "line").map((r) => (r as { line: DiffLine }).line.text);

describe("diffLines", () => {
  it("marks every line with its old and new number", () => {
    expect(diffLines("a\nb\nc", "a\nB\nc\nd")).toEqual([
      { kind: "same", text: "a", old: 1, new: 1 },
      { kind: "del", text: "b", old: 2, new: null },
      { kind: "add", text: "B", old: null, new: 2 },
      { kind: "same", text: "c", old: 3, new: 3 },
      { kind: "add", text: "d", old: null, new: 4 },
    ]);
  });

  it("an added file is all additions numbered from 1, a removed file all removals", () => {
    expect(diffLines("", "x\ny")).toEqual([
      { kind: "add", text: "x", old: null, new: 1 },
      { kind: "add", text: "y", old: null, new: 2 },
    ]);
    expect(diffLines("x\ny", "")).toEqual([
      { kind: "del", text: "x", old: 1, new: null },
      { kind: "del", text: "y", old: 2, new: null },
    ]);
  });

  it("identical texts are all same lines", () => {
    expect(diffLines("a\nb", "a\nb").every((l) => l.kind === "same")).toBe(true);
  });

  it("CRLF endings stay part of the line: the same CRLF text is unchanged", () => {
    expect(diffLines("a\r\nb\r\n", "a\r\nb\r\n").every((l) => l.kind === "same")).toBe(true);
  });

  it("a missing trailing newline is an added empty last line, not a changed file", () => {
    expect(diffLines("a\nb", "a\nb\n")).toEqual([
      { kind: "same", text: "a", old: 1, new: 1 },
      { kind: "same", text: "b", old: 2, new: 2 },
      { kind: "add", text: "", old: null, new: 3 },
    ]);
  });
});

describe("buildDiff (the folding model)", () => {
  it("keeps three lines of context around a change and folds the rest", () => {
    const text = Array.from({ length: 20 }, (_, i) => `line ${i + 1}`).join("\n");
    const next = text.replace("line 10", "line ten");
    const d = buildDiff(text, next);
    // 9 unchanged lines before (context 3, fold 6) — the pair — 10 after (context 3, fold 7).
    // 6 unchanged lines fold before the pair, its 3 + 3 lines of context show, 7 fold after.
    expect(kinds(d.rows)).toEqual(["fold:6", "same", "same", "same", "del", "add", "same", "same", "same", "fold:7"]);
    const fold = folds(d.rows)[0];
    expect(fold.above).toBe(0);
    expect(fold.below).toBe(0);
    expect(hiddenRange(fold)).toEqual({ from: 1, to: 6 });
  });

  it("two changes closer than six lines merge into one hunk", () => {
    const text = Array.from({ length: 20 }, (_, i) => `line ${i + 1}`).join("\n");
    const next = text.replace("line 5", "five").replace("line 9", "nine");
    const d = buildDiff(text, next);
    // 1 unchanged line folds before, the two pairs with the 3 lines between them (inside the
    // context) are one hunk, 8 lines fold after.
    expect(kinds(d.rows)).toEqual(["fold:1", "same", "same", "same", "del", "add", "same", "same", "same", "del", "add", "same", "same", "same", "fold:8"]);
  });

  it("changes far apart stay in separate hunks", () => {
    const text = Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join("\n");
    const next = text.replace("line 5", "five").replace("line 25", "twenty-five");
    const d = buildDiff(text, next);
    expect(kinds(d.rows)).toEqual(["fold:1", "same", "same", "same", "del", "add", "same", "same", "same", "fold:13", "same", "same", "same", "del", "add", "same", "same", "same", "fold:2"]);
  });

  it("an added file folds nothing at the edges and says so; a removed file likewise", () => {
    const d = buildDiff("", "x\ny\nz");
    expect(kinds(d.rows)).toEqual(["add", "add", "add"]);
    expect(d.lines).toBe(3);
    expect(d.veryLarge).toBe(false);
    const r = buildDiff("x\ny\nz", "");
    expect(kinds(r.rows)).toEqual(["del", "del", "del"]);
    expect(r.lines).toBe(0);
  });

  it("identical files produce no changed line at all", () => {
    const d = buildDiff("a\nb\n", "a\nb\n");
    expect(d.rows.some((r) => r.type === "line" && r.line.kind !== "same")).toBe(false);
  });

  it("a very large input (over 5,000 lines) stays folded", () => {
    const text = Array.from({ length: 5_200 }, (_, i) => `line ${i + 1}`).join("\n");
    const next = text.replace("line 3000", "line three thousand");
    const d = buildDiff(text, next);
    expect(d.veryLarge).toBe(true);
    expect(d.lines).toBe(5_200);
    // Only the change and its context are lines; the rest is two folds.
    expect(d.rows.filter((r) => r.type === "fold").map((f) => (f as { count: number }).count)).toEqual([2_996, 2_197]);
    expect(d.rows.filter((r) => r.type === "line")).toHaveLength(2 * DIFF_CONTEXT + 2);
  });
});

describe("expanding a fold", () => {
  const text = Array.from({ length: 80 }, (_, i) => `line ${i + 1}`).join("\n");
  const next = text.replace("line 50", "fifty");
  const base = buildDiff(text, next);
  // The first fold holds the 46 unchanged lines before the change's context.
  const foldId = folds(base.rows)[0].id;
  expect(folds(base.rows)[0].count).toBe(46);

  it("Show 20 more above reveals exactly the lines it names", () => {
    const rows = expandAbove(base.rows, foldId);
    const fold = folds(rows)[0];
    expect(fold.above).toBe(EXPAND_STEP);
    expect(fold.below).toBe(0);
    expect(hiddenRange(fold)).toEqual({ from: 21, to: 46 });
    // The revealed lines are the fold's first 20, in file order.
    expect(fold.lines.slice(0, fold.above).map((l) => l.text)).toEqual(Array.from({ length: EXPAND_STEP }, (_, i) => `line ${i + 1}`));
  });

  it("Show 20 more below reveals exactly the lines it names", () => {
    const rows = expandBelow(base.rows, foldId);
    const fold = folds(rows)[0];
    expect(fold.below).toBe(EXPAND_STEP);
    expect(fold.above).toBe(0);
    expect(hiddenRange(fold)).toEqual({ from: 1, to: 26 });
    expect(fold.lines.slice(fold.count - fold.below).map((l) => l.text)).toEqual(Array.from({ length: EXPAND_STEP }, (_, i) => `line ${i + 27}`));
  });

  it("expanding by steps never crosses the other side, and the last step empties the fold", () => {
    let rows = base.rows;
    for (let k = 0; k < 10; k++) rows = expandAbove(rows, foldId, 5);
    // The first fold is gone; the second one (after the change) is untouched.
    expect(folds(rows).map((f) => f.id)).toEqual([1]);
    const all = lineText(rows);
    expect(all).toHaveLength(54);
    expect(all[45]).toBe("line 46");
    expect(all[48]).toBe("line 49");
    expect(all[49]).toBe("line 50");
    expect(all[50]).toBe("fifty");
    expect(all[51]).toBe("line 51");
  });

  it("Show all expands the whole run at once", () => {
    const rows = expandFold(base.rows, foldId);
    expect(folds(rows).map((f) => f.id)).toEqual([1]);
    expect(lineText(rows).slice(0, 19)).toEqual(Array.from({ length: 19 }, (_, i) => `line ${i + 1}`));
  });

  it("Show the whole file expands every fold", () => {
    const rows = expandAll(base.rows);
    expect(folds(rows)).toEqual([]);
    expect(rows).toHaveLength(81);
  });

  it("expanding an unknown or already expanded fold leaves the rows alone", () => {
    expect(expandAbove(base.rows, 99)).toBe(base.rows);
    const expanded = expandFold(base.rows, foldId);
    expect(expandFold(expanded, foldId)).toEqual(expanded);
  });
});

describe("wordSpans", () => {
  it("marks the words that differ and keeps the shared ones unchanged", () => {
    const r = wordSpans("return the first item", "return the last item");
    expect(r.old).toEqual([
      { text: "return the ", changed: false },
      { text: "first", changed: true },
      { text: " item", changed: false },
    ]);
    expect(r.new).toEqual([
      { text: "return the ", changed: false },
      { text: "last", changed: true },
      { text: " item", changed: false },
    ]);
  });

  it("punctuation belongs to the words around it", () => {
    const r = wordSpans("files.push(x);", "files.pop(x);");
    expect(r.old).toEqual([
      { text: "files.", changed: false },
      { text: "push", changed: true },
      { text: "(x);", changed: false },
    ]);
  });

  it("lines with nothing in common change as a whole", () => {
    const r = wordSpans("alpha beta", "gamma delta");
    // The space between the words is shared, so each word is its own changed span.
    expect(r.old).toEqual([
      { text: "alpha", changed: true },
      { text: " ", changed: false },
      { text: "beta", changed: true },
    ]);
    expect(r.new).toEqual([
      { text: "gamma", changed: true },
      { text: " ", changed: false },
      { text: "delta", changed: true },
    ]);
  });

  it("an empty side changes as a whole only when it has text", () => {
    expect(wordSpans("", "text").new).toEqual([{ text: "text", changed: true }]);
    expect(wordSpans("text", "").old).toEqual([{ text: "text", changed: true }]);
  });
});
