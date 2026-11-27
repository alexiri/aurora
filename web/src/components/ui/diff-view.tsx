"use client";

import { useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import {
  EXPAND_STEP,
  buildDiff,
  expandAbove,
  expandAll,
  expandBelow,
  expandFold,
  hiddenRange,
  wordSpans,
  type DiffLine,
  type DiffRow,
  type FoldRow,
  type WordSpan,
} from "@/lib/line-diff";

/**
 * The one diff primitive (FIX-250, replacing `Lines`): a GitLab-style unified diff of two texts —
 * every change with old and new line numbers and three lines of context, every longer run of
 * unchanged lines folded into one "N unchanged lines" row with **Show 20 more above / below** and
 * **Show all** buttons, and a **Show the whole file** switch in the header. A changed-line pair
 * highlights the words that differ; a file that was added or removed shows as such; files with no
 * change are not shown as diffs at all. Long lines scroll inside the box, never the page.
 */
export function DiffView({ old, next, label, fileName, status }: { old: string; next: string; label: string; fileName?: string; status?: "added" | "removed" | null }) {
  const base = useMemo(() => buildDiff(old, next), [old, next]);
  const [rows, setRows] = useState<DiffRow[]>(base.rows);
  const [whole, setWhole] = useState(false);
  // A new pair of texts resets the folding (the render-time reset pattern; `key` also works).
  const [texts, setTexts] = useState({ old, next });
  if (texts.old !== old || texts.next !== next) {
    setTexts({ old, next });
    setRows(base.rows);
    setWhole(false);
  }
  const added = status ? status === "added" : old === "" && next !== "";
  const removed = status ? status === "removed" : next === "" && old !== "";
  const changed = base.rows.some((r) => r.type === "line" && r.line.kind !== "same");
  // Word-level highlight: a removed line paired with the added line right after it.
  const highlights = useMemo(() => {
    const map = new Map<DiffLine, WordSpan[]>();
    const lines = base.rows.flatMap((r) => (r.type === "fold" ? r.lines : [r.line]));
    for (let i = 0; i + 1 < lines.length; i++) {
      if (lines[i].kind === "del" && lines[i + 1].kind === "add") {
        const spans = wordSpans(lines[i].text, lines[i + 1].text);
        map.set(lines[i], spans.old);
        map.set(lines[i + 1], spans.new);
      }
    }
    return map;
  }, [base]);

  return (
    <section role="region" aria-label={label} data-testid="diff-view" className="space-y-1">
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1">
        <p className="min-w-0 font-mono text-xs text-ink">
          {fileName ? <span className="font-semibold">{fileName}</span> : null}
          {added ? (
            <>
              {" "}
              <span className="text-muted">(added)</span>
            </>
          ) : null}
          {removed ? (
            <>
              {" "}
              <span className="text-muted">(removed)</span>
            </>
          ) : null}
        </p>
        {changed ? (
          <label className="flex items-center gap-2 text-sm text-ink-2">
            <Switch checked={whole} onCheckedChange={setWhole} />
            Show the whole file
          </label>
        ) : null}
      </div>
      {base.veryLarge ? (
        <p className="text-xs text-muted">This file has {base.lines.toLocaleString("en-US")} lines, so only the changes and their context are shown until you expand them.</p>
      ) : null}
      {!changed ? (
        <p className="text-sm text-muted">Unchanged.</p>
      ) : (
        <div tabIndex={0} className="max-w-full overflow-x-auto rounded-control border border-line bg-canvas" data-testid="diff-box">
          <table className="w-full border-collapse font-mono text-xs leading-5 text-ink">
            <tbody>
              {(whole ? expandAll(base.rows) : rows).flatMap((row) => {
                if (row.type === "line") return [<LineRow key={`l${row.line.new ?? `o${row.line.old}`}`} line={row.line} highlights={highlights} />];
                return [
                  ...row.lines.slice(0, row.above).map((line) => <LineRow key={`l${line.new ?? `o${line.old}`}`} line={line} highlights={highlights} />),
                  <FoldButtons key={`f${row.id}`} fold={row} rows={rows} setRows={setRows} />,
                  ...row.lines.slice(row.count - row.below).map((line) => <LineRow key={`l${line.new ?? `o${line.old}`}`} line={line} highlights={highlights} />),
                ];
              })}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

/** The fold row: the run it still hides, and the buttons that reveal it (keyboard operable). */
function FoldButtons({ fold, rows, setRows }: { fold: FoldRow; rows: DiffRow[]; setRows: (rows: DiffRow[]) => void }) {
  const hidden = hiddenRange(fold);
  if (!hidden) return null;
  const n = hidden.to - hidden.from + 1;
  const more = Math.min(EXPAND_STEP, n);
  const aboveTo = Math.min(fold.above + EXPAND_STEP, fold.count - fold.below);
  const belowFrom = Math.max(fold.count - fold.below - EXPAND_STEP, fold.above);
  const from = (line: DiffLine | undefined) => line?.new ?? line?.old ?? 0;
  return (
    <tr data-testid="diff-fold">
      <td colSpan={3} className="border-y border-line bg-subtle px-2 py-1 text-center font-sans text-xs text-ink-2">
        <span className="mr-2">
          {n} unchanged line{n === 1 ? "" : "s"}
        </span>
        <Button size="sm" variant="outline" className="mx-0.5" aria-label={`Show ${more} more lines above (lines ${from(fold.lines[fold.above])} to ${from(fold.lines[aboveTo - 1])})`} onClick={() => setRows(expandAbove(rows, fold.id))}>
          Show 20 more above
        </Button>
        <Button size="sm" variant="outline" className="mx-0.5" aria-label={`Show ${more} more lines below (lines ${from(fold.lines[belowFrom])} to ${from(fold.lines[fold.count - fold.below - 1])})`} onClick={() => setRows(expandBelow(rows, fold.id))}>
          Show 20 more below
        </Button>
        <Button size="sm" variant="outline" className="mx-0.5" aria-label={`Show all ${n} unchanged lines (lines ${hidden.from} to ${hidden.to})`} onClick={() => setRows(expandFold(rows, fold.id))}>
          Show all
        </Button>
      </td>
    </tr>
  );
}

/** One diff line: its old and new number, its sign, and its text with the changed words marked. */
function LineRow({ line, highlights }: { line: DiffLine; highlights: Map<DiffLine, WordSpan[]> }) {
  const spans = highlights.get(line);
  return (
    <tr data-diff={line.kind} className={line.kind === "add" ? "bg-accent-soft text-accent-ink" : line.kind === "del" ? "bg-danger-soft text-danger-ink" : undefined}>
      <td className="w-10 select-none border-r border-line px-1 text-right align-top text-muted tabular-nums">{line.old ?? ""}</td>
      <td className="w-10 select-none border-r border-line px-1 text-right align-top text-muted tabular-nums">{line.new ?? ""}</td>
      <td className="whitespace-pre px-2 align-top">
        {line.kind === "add" ? "+ " : line.kind === "del" ? "- " : "  "}
        {spans
          ? spans.map((s, i) => (s.changed ? <mark key={i} className={line.kind === "add" ? "bg-accent-ink/25 text-accent-ink" : "bg-danger-ink/25 text-danger-ink"}>{s.text}</mark> : <span key={i}>{s.text}</span>))
          : line.text}
      </td>
    </tr>
  );
}
