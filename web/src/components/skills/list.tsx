"use client";

import { useId, useState } from "react";

import { Badge } from "@/components/ui/badge";
import { ChoiceChips } from "@/components/ui/choice-chips";
import { inputClass } from "@/components/ui/field";
import { SearchIcon } from "@/components/ui/icons";
import { Switch } from "@/components/ui/switch";
import { descriptionSentence } from "@/lib/skill-name";
import { cn } from "@/lib/utils";

import {
  combineWords,
  filterOptions,
  overrideSentence,
  pageSubject,
  replaceWords,
  sourceChip,
  usedByWords,
  type SkillFilter,
  type SkillPage,
  type SkillRow,
  type SourceTone,
} from "./model";

// A source is told by its words; the tint only groups them.
const TONE: Record<SourceTone, string> = {
  agent: "bg-accent-soft text-accent-ink",
  workspace: "bg-subtle text-ink-2",
  org: "bg-avatar-lavender text-avatar-lavender-ink",
  library: "bg-avatar-blue text-avatar-blue-ink",
  app: "bg-subtle text-ink-2",
};

export function SourceChip({ page, row }: { page: SkillPage; row: SkillRow }) {
  const { label, tone } = sourceChip(page, row);
  // `text-xs`, not the Badge's own size: the class merger reads `text-label` as a colour and drops it beside one.
  return (
    <Badge data-testid="source-chip" className={cn("max-w-full text-xs [overflow-wrap:anywhere]", TONE[tone])}>
      {label}
    </Badge>
  );
}

/**
 * A row's switch: "On"/"Off" beside it in the table and the panel. Its name is **Turn off for <subject>** with the
 * skill's name, so each switch of a list is told apart; a switch that cannot change says why.
 */
export function SkillSwitch({
  page,
  row,
  busy,
  onToggle,
  words = true,
}: {
  page: SkillPage;
  row: SkillRow;
  busy: boolean;
  onToggle: (row: SkillRow, on: boolean) => void;
  words?: boolean;
}) {
  const whyId = useId();
  // An app's skills come and go with the app: it reads On, and there is nothing to switch (D-455).
  if (row.control === "app") {
    return <span className="text-xs text-ink-2">{row.on ? "On" : "Off"}</span>;
  }
  return (
    <span className="inline-flex items-center gap-2" title={row.fixedWhy ?? undefined}>
      <Switch
        checked={row.on}
        disabled={busy || row.control === "fixed"}
        aria-label={`Turn off for ${pageSubject(page)}: ${row.name}`}
        aria-describedby={row.fixedWhy ? whyId : undefined}
        onCheckedChange={(on) => onToggle(row, on)}
      />
      {words ? (
        <span aria-hidden="true" className="w-6 text-xs text-ink-2">
          {row.on ? "On" : "Off"}
        </span>
      ) : null}
      {row.fixedWhy ? (
        <span id={whyId} className="sr-only">
          {row.fixedWhy}
        </span>
      ) : null}
    </span>
  );
}

export function SkillSearch({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const id = useId();
  return (
    <div className="relative">
      <label htmlFor={id} className="sr-only">
        Search skills
      </label>
      <SearchIcon className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted" />
      <input id={id} type="search" placeholder="Search skills" autoComplete="off" className={cn(inputClass, "min-h-11 pl-9")} value={value} onChange={(e) => onChange(e.target.value)} />
    </div>
  );
}

export function SkillChips({ page, rows, filter, onFilter }: { page: SkillPage; rows: SkillRow[]; filter: SkillFilter; onFilter: (f: SkillFilter) => void }) {
  return <ChoiceChips label="Show" value={filter} options={filterOptions(page, rows)} onChange={onFilter} />;
}

interface RowsProps {
  page: SkillPage;
  rows: SkillRow[];
  selected: string | null;
  /** The key of the row whose switch is being saved. */
  busyKey: string | null;
  onSelect: (key: string) => void;
  onToggle: (row: SkillRow, on: boolean) => void;
}

// Three columns only where the table itself is wide enough (`@container` + `@xl:`, 36 rem): between 768 and 1100 px the
// window is wide but the rail and the settings sub-nav leave about 300 px. Narrower, a row stacks.
const COLUMNS = "@xl:grid @xl:grid-cols-[minmax(0,1fr)_auto_7.5rem] @xl:items-center @xl:gap-4";

/** The whole table: **Skill · Source · For <subject>**; on a phone each row stacks. */
export function SkillTable({ page, rows, selected, busyKey, onSelect, onToggle, onCompare }: RowsProps & { onCompare: (row: SkillRow) => void }) {
  return (
    <div className="@container overflow-hidden rounded-card border border-line bg-surface">
      <div aria-hidden="true" className={`hidden border-b border-line bg-subtle px-4 py-2 text-label font-semibold uppercase tracking-label text-muted ${COLUMNS}`}>
        <span>Skill</span>
        <span className="text-right">Source</span>
        <span>For {pageSubject(page)}</span>
      </div>
      <ul aria-label="Skills" className="divide-y divide-line">
        {rows.map((row) => {
          const replaces = page.kind === "agent" ? replaceWords(page, row) : null;
          const usedBy = page.kind === "agent" ? null : row.skill.used_by_agents;
          return (
            <li key={row.key} data-testid={`skill-row-${row.skill.name}`} className={cn("relative flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3 hover:bg-subtle", COLUMNS, selected === row.key && "bg-subtle")}>
              <div className="min-w-0 basis-full">
                {/* The name's button covers the whole row; the switch and Compare sit above it. */}
                <button type="button" aria-current={selected === row.key ? "true" : undefined} className="block max-w-full text-left after:absolute after:inset-0" onClick={() => onSelect(row.key)}>
                  <span className={cn("block break-words text-sm font-semibold", row.on ? "text-ink" : "text-muted")}>{row.name}</span>
                </button>
                <p className="break-words text-xs text-ink-2">{descriptionSentence(row.skill.description)}</p>
                {typeof usedBy === "number" ? <p className="text-xs text-muted">{usedByWords(usedBy)}</p> : null}
                {row.fixedWhy ? <p className="text-xs text-muted">{row.fixedWhy}</p> : null}
                {replaces ? (
                  <p className="text-xs text-pending-ink">
                    {replaces} ·{" "}
                    <button type="button" className="relative z-10 text-link underline" aria-label={`Compare ${row.name}`} onClick={() => onCompare(row)}>
                      Compare
                    </button>
                  </p>
                ) : null}
              </div>
              <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5 @xl:justify-end">
                <SourceChip page={page} row={row} />
                {row.skill.source?.update_pending && !row.skill.source.detached ? <Badge className="text-xs">Update available</Badge> : null}
              </div>
              <div className="relative z-10 justify-self-start">
                <SkillSwitch page={page} row={row} busy={busyKey === row.key} onToggle={onToggle} />
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/** The list beside an open skill: the name, one line of the description and the switch. */
export function SkillCompactList({ page, rows, selected, busyKey, onSelect, onToggle }: RowsProps) {
  return (
    <ul aria-label="Skills" className="divide-y divide-line overflow-hidden rounded-card border border-line bg-surface">
      {rows.map((row) => (
        <li
          key={row.key}
          data-testid={`skill-row-${row.skill.name}`}
          className={cn("row-rail relative flex items-center gap-3 border-l-transparent py-2.5 pl-3 pr-3 hover:bg-subtle", selected === row.key && "border-l-accent bg-subtle")}
        >
          <div className="min-w-0 flex-1">
            <button type="button" aria-current={selected === row.key ? "true" : undefined} className="block max-w-full text-left after:absolute after:inset-0" onClick={() => onSelect(row.key)}>
              <span className={cn("block break-words text-sm font-semibold", row.on ? "text-ink" : "text-muted")}>{row.name}</span>
            </button>
            <p className="truncate text-xs text-muted">{descriptionSentence(row.skill.description)}</p>
          </div>
          <div className="relative z-10 shrink-0">
            <SkillSwitch page={page} row={row} busy={busyKey === row.key} onToggle={onToggle} words={false} />
          </div>
        </li>
      ))}
    </ul>
  );
}

/** Under the list: which version wins where two share a name, and **How skills combine**. */
export function SkillsFootnote({ page, rows }: { page: SkillPage; rows: SkillRow[] }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const sentence = overrideSentence(page, rows);
  return (
    <div className="space-y-1 text-sm text-ink-2" data-testid="skills-combine">
      <p>
        {sentence ? `${sentence} ` : null}
        <button type="button" className="min-h-6 text-link underline" aria-expanded={open} aria-controls={id} onClick={() => setOpen(!open)}>
          How skills combine
        </button>
      </p>
      <p id={id} hidden={!open} className="max-w-prose">
        {combineWords(page)}
      </p>
    </div>
  );
}
