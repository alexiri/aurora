"use client";

import { useId, useState, type ReactNode } from "react";

import { AUDIT_ACTION, auditWords } from "@/lib/labels";
import { Avatar } from "@/components/ui/avatar";
import { ResponsiveTable } from "@/components/ui/responsive-table";
import { cn } from "@/lib/utils";
import { When } from "@/components/ui/when";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { inputClass } from "@/components/ui/field";
import { TechnicalDetails } from "@/components/ui/technical-details";
import { LoadMore } from "@/components/ui/load-more";
import { PageHeader } from "@/components/ui/page-header";
import type { components } from "@/lib/api/schema";

import { AuditDetail } from "./audit-detail";

export type AuditEntry = components["schemas"]["AuditEntry"] & { org_slug?: string | null };

/** The slice of an infinite query the panel needs. */
export interface AuditFeed {
  data?: { pages: { items: AuditEntry[] }[] };
  isPending: boolean;
  isError: boolean;
  error: unknown;
  refetch: () => unknown;
  hasNextPage?: boolean;
  isFetchingNextPage: boolean;
  fetchNextPage: () => unknown;
}

export interface AuditFilters {
  action: string;
  criticalOnly: boolean;
}

/**
 * The audit log table with its action and security-critical filters, shared by the org and the instance audit
 * pages. `extraFilters` renders more controls in the filter bar; `showOrg` adds the organization column.
 */
export function AuditPanel({
  title,
  description,
  placeholder,
  feed,
  filters,
  onFilters,
  extraFilters,
  showOrg = false,
  headed = true,
  targetName,
}: {
  /**
   * Present: entries read as sentences ("Bob revoked the API key ci-script") with Critical / Important / Routine, stacked
   * under 900 px (workspace-3). It names the entry's target from what the page knows; `null` falls back to the entry's own name.
   */
  targetName?: (e: AuditEntry) => string | null;
  title: string;
  description: string;
  placeholder: string;
  feed: AuditFeed;
  filters: AuditFilters;
  onFilters: (f: AuditFilters) => void;
  extraFilters?: ReactNode;
  showOrg?: boolean;
  /** False where the page around it already has the heading (the workspace tab). */
  headed?: boolean;
}) {
  const [input, setInput] = useState(filters.action);
  const items = feed.data?.pages.flatMap((p) => p.items) ?? [];
  return (
    <div className="max-w-page space-y-4">
      {headed ? <PageHeader title={title} description={description} className="mb-2" /> : null}
      <form
        role="search"
        className="flex flex-wrap items-center gap-x-4 gap-y-2"
        onSubmit={(e) => {
          e.preventDefault();
          onFilters({ ...filters, action: input.trim() });
        }}>
        <input
          aria-label="Filter by action"
          placeholder={placeholder}
          className={`${inputClass} max-w-xs`}
          value={input}
          onChange={(e) => setInput(e.target.value)}
        />
        <Button type="submit" variant="outline">
          Filter
        </Button>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            className="size-4 accent-accent"
            checked={filters.criticalOnly}
            onChange={(e) => onFilters({ ...filters, criticalOnly: e.target.checked })}
          />
          Security-critical only
        </label>
        {extraFilters}
      </form>
      {feed.isPending ? <Loading /> : null}
      {feed.isError ? <ErrorState error={feed.error} onRetry={() => void feed.refetch()} /> : null}
      {feed.data && items.length === 0 ? <EmptyState title="No audit entries" /> : null}
      {items.length > 0 && targetName ? <SentenceList items={items} title={title} targetName={targetName} /> : null}
      {items.length > 0 && !targetName ? (
        <DataTable caption={title} stackBelow={900}>
          <thead>
            <tr>
              <Th>Action</Th>
              <Th>When</Th>
              <Th>Actor</Th>
              {showOrg ? <Th>Organization</Th> : null}
              <Th>Target</Th>
              <Th>Outcome</Th>
              <Th>
                <span className="sr-only">Details</span>
              </Th>
            </tr>
          </thead>
          <tbody>
            {items.map((e) => (
              <AuditRow key={e.id} e={e} showOrg={showOrg} columns={showOrg ? 7 : 6} />
            ))}
          </tbody>
        </DataTable>
      ) : null}
      <LoadMore query={feed} />
    </div>
  );
}

/** One row and, when opened, its detail row underneath (D-373). Opening refetches nothing: the entry already has it. */
function AuditRow({ e, showOrg, columns }: { e: AuditEntry; showOrg: boolean; columns: number }) {
  const [open, setOpen] = useState(false);
  const panelId = useId();
  return (
    <>
      <tr>
        <Td cell="title">
          {(AUDIT_ACTION as Record<string, { label: string } | undefined>)[e.action] ? (
            <span title={e.action}>{(AUDIT_ACTION as Record<string, { label: string }>)[e.action]!.label}</span>
          ) : (
            <span className="font-mono text-xs">{e.action}</span>
          )}
          {e.critical ? (
            <Badge tone="danger" className="ml-2">
              Critical
            </Badge>
          ) : null}
        </Td>
        <Td label="When" className="whitespace-nowrap text-ink-2">
          <When value={e.created_at} />
        </Td>
        <Td label="By">{e.actor_label}</Td>
        {showOrg ? <Td label="Organization" className="text-ink-2">{e.org_id ? (e.org_slug ?? e.org_id) : "Instance"}</Td> : null}
        <Td label="Target" className="break-all text-ink-2">{e.target_kind ? `${e.target_kind} ${e.target_id ?? ""}` : "—"}</Td>
        <Td label="Outcome">{e.outcome}</Td>
        <Td cell="actions" className="text-right">
          <Button variant="outline" size="sm" aria-expanded={open} aria-controls={panelId} onClick={() => setOpen(!open)}>
            Details<span className="sr-only"> of {e.action}</span>
          </Button>
        </Td>
      </tr>
      {open ? (
        <tr id={panelId}>
          <Td colSpan={columns} className="bg-canvas">
            {/* Limited to the visible width of the scrolling table region, so a long value wraps instead of widening it. */}
            <div className="sticky left-0 w-[min(100%,calc(100vw-4rem))]">
              <AuditDetail entry={e} />
            </div>
          </Td>
        </tr>
      ) : null}
    </>
  );
}

const WEIGHT_WORDS = { critical: "Critical", important: "Important", routine: "Routine" } as const;
const WEIGHT_CLASS = {
  critical: "bg-danger-soft text-danger-ink",
  important: "bg-pending-panel text-pending-ink",
  routine: "bg-subtle text-ink-2",
} as const;

/** (workspace-3) The audit as sentences on stacked rows; the raw event is under Details, in Technical details. */
function SentenceList({ items, title, targetName }: { items: AuditEntry[]; title: string; targetName: (e: AuditEntry) => string | null }) {
  const [open, setOpen] = useState<string | null>(null);
  return (
    <ResponsiveTable
      caption={title}
      rowKey={(e) => e.id}
      rows={items}
      title={{
        header: "What happened",
        cell: (e) => {
          const w = auditWords(e, targetName(e));
          return (
            <div className="flex min-w-0 items-start gap-3">
              <Avatar name={e.actor_label} size={32} />
              <div className="min-w-0">
                <p className="text-sm font-medium text-ink" data-testid="audit-sentence">{w.sentence}</p>
                <p className="text-xs text-muted">
                  <When value={e.created_at} /> · {e.actor_label}
                </p>
                {open === e.id ? (
                  <div className="mt-2 rounded-control bg-canvas p-3">
                    <TechnicalDetails title="Technical details" bodyClassName="mt-2 font-sans text-ink">
                      <p className="mb-2 font-mono text-xs text-ink-2">{e.action}</p>
                      <AuditDetail entry={e} />
                    </TechnicalDetails>
                  </div>
                ) : null}
              </div>
            </div>
          );
        },
      }}
      columns={[
        {
          key: "weight",
          header: "Importance",
          label: false,
          cell: (e) => {
            const w = auditWords(e, targetName(e)).weight;
            return <span className={cn("inline-flex rounded-full px-2 py-0.5 text-label font-medium leading-4", WEIGHT_CLASS[w])}>{WEIGHT_WORDS[w]}</span>;
          },
        },
      ]}
      actions={(e) => (
        <Button variant="ghost" size="sm" aria-expanded={open === e.id} onClick={() => setOpen(open === e.id ? null : e.id)}>
          Details
        </Button>
      )}
    />
  );
}
