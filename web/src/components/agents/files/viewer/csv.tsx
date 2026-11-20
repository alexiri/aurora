"use client";

import { Button, buttonVariants } from "@/components/ui/button";
import { DownloadIcon } from "@/components/ui/icons";

/** The table view reads at most this much of a CSV (computer-11). */
export const CSV_MAX_ROWS = 200;
export const CSV_MAX_COLS = 50;

/**
 * A small RFC 4180 reader: commas, quoted fields with `""` and embedded newlines, CRLF or LF. Stops after `maxRows`
 * records and keeps `maxCols` fields of each. Cells are plain strings: the table renders them as text, never as HTML.
 */
export function parseCsv(text: string, maxRows = CSV_MAX_ROWS, maxCols = CSV_MAX_COLS): { rows: string[][]; moreRows: boolean; moreCols: boolean } {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;
  let moreCols = false;
  let moreRows = false;
  const endField = () => {
    if (row.length < maxCols) row.push(field);
    else moreCols = true;
    field = "";
  };
  const endRow = (): boolean => {
    endField();
    rows.push(row);
    row = [];
    if (rows.length >= maxRows) {
      moreRows = true;
      return true;
    }
    return false;
  };
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i]!;
    if (quoted) {
      if (ch === '"') {
        if (text[i + 1] === '"') {
          field += '"';
          i += 1;
        } else quoted = false;
      } else field += ch;
    } else if (ch === '"' && field === "") quoted = true;
    else if (ch === ",") endField();
    else if (ch === "\n" || ch === "\r") {
      if (ch === "\r" && text[i + 1] === "\n") i += 1;
      if (endRow()) {
        // Anything left after the cap means there are more rows (a trailing newline alone does not).
        moreRows = text.slice(i + 1).trim() !== "";
        return { rows, moreRows, moreCols };
      }
    } else field += ch;
  }
  if (field !== "" || row.length > 0) {
    endRow();
    moreRows = false;
  }
  return { rows, moreRows, moreCols };
}

/** (P5-REFINE-15, computer-11) "fruit.csv · table view · read only": the first 200 rows and 50 columns, a header row, **Show as text**. */
export function CsvTable({ name, text, downloadHref, onShowText }: { name: string; text: string; downloadHref: string; onShowText: () => void }) {
  const { rows, moreRows, moreCols } = parseCsv(text);
  const [head, ...body] = rows;
  return (
    <div className="space-y-2" data-testid="csv-table">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-sm text-ink-2">
          {name} · table view · read only
        </p>
        <div className="flex flex-wrap gap-2">
          <Button type="button" variant="outline" size="sm" onClick={onShowText}>
            Show as text
          </Button>
          <a href={downloadHref} download className={buttonVariants({ variant: "outline", size: "sm" })}>
            <DownloadIcon /> Download
          </a>
        </div>
      </div>
      <div role="region" aria-label={`${name} as a table`} tabIndex={0} className="max-h-96 max-w-full overflow-auto rounded-card border border-line bg-surface">
        <table className="min-w-max text-left text-sm text-ink">
          <thead>
            <tr>
              {(head ?? []).map((c, i) => (
                <th key={i} scope="col" className="sticky top-0 whitespace-nowrap bg-canvas px-3 py-2 text-xs font-semibold text-muted">
                  {c}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {body.map((r, ri) => (
              <tr key={ri}>
                {r.map((c, ci) => (
                  <td key={ci} className="border-t border-line px-3 py-1.5 align-top">
                    {c}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {moreRows || moreCols ? (
        <p className="text-xs text-muted">
          Showing the first {CSV_MAX_ROWS} rows and {CSV_MAX_COLS} columns.
        </p>
      ) : null}
    </div>
  );
}
