"use client";

import Link from "next/link";
import { useId, useState, type ReactNode } from "react";

import { Altitude } from "@/components/ui/altitude";
import { LockPill } from "@/components/ui/lock-pill";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { ItemList, ListRow } from "@/components/ui/section";
import type { WhoCanValue } from "@/lib/who-can";

/** A limit set by a level above (C-963): what it says in words, who set it, and who can change it. */
export interface LadderAbove {
  label: string;
  /** The value in words ("Requests without an account are blocked", "Websites on the allow-list: 4"). */
  value_display: string;
  source: "instance" | "org";
  who_can: WhoCanValue;
}

/** What the levels below tightened (C-963, C-964): a table of rows, each with a link to the level's own page. */
export interface LadderBelow {
  columns: string[];
  rows: { label: string; values: string[]; link: { href: string; label: string } }[];
  /** Shown when there is no row ("No workspace has tightened this."). */
  empty?: string;
}

const SOURCE_WORDS: Record<LadderAbove["source"], string> = { instance: "Set by Administration", org: "Set by your organization" };

/**
 * (P5-IA-2, C-963) The scope ladder, for any setting that levels tighten from above (`14 §Information architecture`):
 *
 * 1. **Above**: the limits the levels above set, one row each, with the source and `WhoCan` (who can change it).
 * 2. **Here**: this level's controls, with an optional `minimumNote` naming the limit ("Looser options are not available:
 *    Administration requires …"). `collapsed` (a workspace) puts them behind **Tighten…**, closed until opened.
 * 3. **Below** (optional): what the levels below tightened, as a table that stacks into rows at 390 px.
 *
 * A viewer who cannot change this level passes `readOnly`: the controls are replaced by the same ladder without the
 * disclosure, and the caller shows the `WhoCan` sentence in the place of its main action.
 */
export function ScopeLadder({
  above,
  here,
  minimumNote,
  collapsed = false,
  tightenLabel = "Tighten…",
  below,
  className,
}: {
  above: LadderAbove[];
  /** This level's controls; none for a viewer who cannot change this level (the ladder is then only what is above). */
  here?: ReactNode;
  minimumNote?: ReactNode;
  collapsed?: boolean;
  tightenLabel?: string;
  below?: LadderBelow;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const panelId = useId();
  return (
    <div className={className ? `space-y-4 ${className}` : "space-y-4"}>
      {above.length > 0 ? (
        <section aria-label="Limits from above" className="space-y-2">
          <h3 className="text-sm font-semibold text-ink">Limits from above</h3>
          <ItemList aria-label="Limits set above this level">
            {above.map((a) => (
              <ListRow key={a.label}>
                <div className="min-w-0 space-y-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-medium text-ink">{a.label}</span>
                    <LockPill owner={SOURCE_WORDS[a.source]} whoCan={a.who_can} />
                  </div>
                  <p className="text-sm text-ink-2">{a.value_display}</p>
                </div>
              </ListRow>
            ))}
          </ItemList>
        </section>
      ) : null}

      {here ? (
        <section aria-label="This level" className="space-y-2">
          {minimumNote ? <div className="text-sm text-ink-2">{minimumNote}</div> : null}
          {collapsed ? (
            // (P5-REFINE-3) Restricting further is Set up: a plain member reads the "Set for you by …" rows above only.
            <Altitude id="policy.restrict" level="setup">
              <Button type="button" variant="outline" size="sm" aria-expanded={open} aria-controls={panelId} onClick={() => setOpen((o) => !o)}>
                {open ? "Hide" : tightenLabel}
              </Button>
              {open ? (
                <div id={panelId} className="space-y-4">
                  {here}
                </div>
              ) : null}
            </Altitude>
          ) : (
            here
          )}
        </section>
      ) : null}

      {below ? (
        <section aria-label="Tightened below" className="space-y-2">
          <h3 className="text-sm font-semibold text-ink">Tightened below</h3>
          {below.rows.length === 0 ? (
            <p className="text-sm text-ink-2">{below.empty ?? "Nothing below has tightened this."}</p>
          ) : (
            <>
              <div className="hidden md:block">
                <DataTable caption="What the levels below tightened">
                  <thead>
                    <tr>
                      <Th />
                      {below.columns.map((c) => (
                        <Th key={c}>{c}</Th>
                      ))}
                      <Th />
                    </tr>
                  </thead>
                  <tbody>
                    {below.rows.map((r) => (
                      <tr key={r.label}>
                        <Td className="font-medium">{r.label}</Td>
                        {r.values.map((v, i) => (
                          <Td key={below.columns[i] ?? i}>{v}</Td>
                        ))}
                        <Td className="text-right">
                          <Link href={r.link.href} className="font-medium text-link underline">
                            {r.link.label}
                          </Link>
                        </Td>
                      </tr>
                    ))}
                  </tbody>
                </DataTable>
              </div>
              <div className="md:hidden">
                <ItemList aria-label="What the levels below tightened">
                  {below.rows.map((r) => (
                    <ListRow key={r.label}>
                      <div className="min-w-0 space-y-1">
                        <p className="font-medium text-ink">{r.label}</p>
                        <dl className="space-y-0.5 text-sm text-ink-2">
                          {r.values.map((v, i) => (
                            <div key={below.columns[i] ?? i} className="flex flex-wrap gap-x-2">
                              <dt className="text-muted">{below.columns[i]}</dt>
                              <dd>{v}</dd>
                            </div>
                          ))}
                        </dl>
                        <Link href={r.link.href} className="text-sm font-medium text-link underline">
                          {r.link.label}
                        </Link>
                      </div>
                    </ListRow>
                  ))}
                </ItemList>
              </div>
            </>
          )}
        </section>
      ) : null}
    </div>
  );
}
