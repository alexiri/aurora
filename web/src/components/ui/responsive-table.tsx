import type { ReactNode } from "react";

import { DataTable, Td, Th } from "@/components/ui/data-table";
import { cn } from "@/lib/utils";

export interface ResponsiveColumn<R> {
  key: string;
  /** The column header on a desktop, and the label before the value on a stacked row. */
  header: string;
  cell: (row: R) => ReactNode;
  /** `false`: the stacked row shows the value without the label before it (a chip, a role). */
  label?: boolean;
  /** Extra classes of the cell. */
  className?: string;
}

/**
 * (P5-REFINE-2, D-405; org-13, computer-5) A table that is a table on a desktop and **stacked rows** on a phone. One
 * DOM and a CSS breakpoint (no JS width listener, so the server markup matches): at or above `stackBelow` (900 px) a
 * normal `DataTable` with a column per `columns` entry; below it one card per row: the `title` cell on the first line,
 * `actions` at the right edge of that line, then the other cells as lines of "label value". Nothing hides behind an
 * inner scrollbar. `caption` is the visually hidden table name. More than six columns belong in the row's details.
 */
export function ResponsiveTable<R>({
  caption,
  title,
  columns,
  rows,
  rowKey,
  actions,
  actionsHeader = "Actions",
  stackBelow = 900,
  className,
}: {
  caption: string;
  title: { header: string; cell: (row: R) => ReactNode };
  columns: ResponsiveColumn<R>[];
  rows: R[];
  rowKey: (row: R) => string;
  actions?: (row: R) => ReactNode;
  actionsHeader?: string;
  stackBelow?: 900;
  className?: string;
}) {
  return (
    <DataTable caption={caption} stackBelow={stackBelow} className={className}>
      <thead>
        <tr>
          <Th>{title.header}</Th>
          {columns.map((c) => (
            <Th key={c.key}>{c.header}</Th>
          ))}
          {actions ? (
            <Th>
              <span className="sr-only">{actionsHeader}</span>
            </Th>
          ) : null}
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={rowKey(row)}>
            <Td cell="title">{title.cell(row)}</Td>
            {columns.map((c) => (
              <Td key={c.key} label={c.label === false ? undefined : c.header} className={cn(c.className)}>
                {c.cell(row)}
              </Td>
            ))}
            {actions ? (
              <Td cell="actions" className="whitespace-nowrap text-right">
                {actions(row)}
              </Td>
            ) : null}
          </tr>
        ))}
      </tbody>
    </DataTable>
  );
}
