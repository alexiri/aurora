"use client";

import { useId, useState, type ReactNode } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { inputClass } from "@/components/ui/field";
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
}: {
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
      {items.length > 0 ? (
        <DataTable caption={title}>
          <thead>
            <tr>
              <Th>When</Th>
              <Th>Actor</Th>
              <Th>Action</Th>
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
        <Td className="whitespace-nowrap text-ink-2">{fmtDate(e.created_at)}</Td>
        <Td>{e.actor_label}</Td>
        <Td>
          <span className="font-mono text-xs">{e.action}</span>
          {e.critical ? (
            <Badge tone="accent" className="ml-2">
              Critical
            </Badge>
          ) : null}
        </Td>
        {showOrg ? <Td className="text-ink-2">{e.org_id ? (e.org_slug ?? e.org_id) : "Instance"}</Td> : null}
        <Td className="text-ink-2">{e.target_kind ? `${e.target_kind} ${e.target_id ?? ""}` : "—"}</Td>
        <Td>{e.outcome}</Td>
        <Td className="text-right">
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
