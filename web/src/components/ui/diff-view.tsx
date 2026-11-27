"use client";

import { useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { Fold } from "@/components/ui/fold";
import { Notice } from "@/components/ui/notice";
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
  // The label without its "Diff of", to name the parts inside the region.
  const subject = label.replace(/^Diff of /, "");
  // Word-level highlight: in a block of removed lines followed by added ones, the lines pair up in order.
  const highlights = useMemo(() => {
    const map = new Map<DiffLine, WordSpan[]>();
    const lines = base.rows.flatMap((r) => (r.type === "fold" ? r.lines : [r.line]));
    let i = 0;
    while (i < lines.length) {
      const dels: DiffLine[] = [];
      while (lines[i]?.kind === "del") dels.push(lines[i++]!);
      const adds: DiffLine[] = [];
      while (lines[i]?.kind === "add") adds.push(lines[i++]!);
      for (let k = 0; k < Math.min(dels.length, adds.length); k++) {
        const spans = wordSpans(dels[k]!.text, adds[k]!.text);
        map.set(dels[k]!, spans.old);
        map.set(adds[k]!, spans.new);
      }
      if (dels.length === 0 && adds.length === 0) i++;
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
      {base.tooLarge ? (
        <div className="space-y-2">
          <Notice>This file is too large to compare line by line; the whole old and new text are below.</Notice>
          <WholeText summary="Old text" label={`Old text: ${subject}`} text={old} />
          <WholeText summary="New text" label={`New text: ${subject}`} text={next} />
        </div>
      ) : !changed ? (
        <p className="text-sm text-muted">{added || removed ? "The file is empty." : "Unchanged."}</p>
      ) : (
        <div tabIndex={0} role="region" aria-label={`Scrollable lines: ${subject}`} className="max-w-full overflow-x-auto rounded-control border border-line bg-canvas" data-testid="diff-box">
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

/** One side of a file too large to compare, in full, in a closed fold. */
function WholeText({ summary, label, text }: { summary: string; label: string; text: string }) {
  return (
    <Fold summary={summary} data-testid="diff-whole-text">
      <pre tabIndex={0} aria-label={label} className="mt-2 max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 font-mono text-xs text-ink [overflow-wrap:anywhere]">
        {text}
      </pre>
    </Fold>
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
        {/* The visible text must stay inside the accessible name (axe label-content-name-mismatch). */}
        <Button size="sm" variant="outline" className="mx-0.5" aria-label={`Show ${more} more line${more === 1 ? "" : "s"} above (lines ${from(fold.lines[fold.above])} to ${from(fold.lines[aboveTo - 1])})`} onClick={() => setRows(expandAbove(rows, fold.id))}>
          Show {more} more line{more === 1 ? "" : "s"} above
        </Button>
        <Button size="sm" variant="outline" className="mx-0.5" aria-label={`Show ${more} more line${more === 1 ? "" : "s"} below (lines ${from(fold.lines[belowFrom])} to ${from(fold.lines[fold.count - fold.below - 1])})`} onClick={() => setRows(expandBelow(rows, fold.id))}>
          Show {more} more line{more === 1 ? "" : "s"} below
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
