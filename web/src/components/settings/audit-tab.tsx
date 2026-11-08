"use client";

import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { inputClass } from "@/components/ui/field";
import { useAudit } from "@/lib/api/account";

export function AuditTab({ wsId }: { wsId: string }) {
  const [input, setInput] = useState("");
  const [action, setAction] = useState("");
  const audit = useAudit(wsId, action);
  const items = audit.data?.pages.flatMap((p) => p.items) ?? [];
  return (
    <div className="space-y-4">
      <form
        role="search"
        className="flex max-w-md gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          setAction(input.trim());
        }}
      >
        <input
          aria-label="Filter by action"
          placeholder="Action, e.g. workspace.update"
          className={inputClass}
          value={input}
          onChange={(e) => setInput(e.target.value)}
        />
        <Button type="submit" variant="outline">
          Filter
        </Button>
      </form>
      {audit.isPending ? <Loading /> : null}
      {audit.isError ? <ErrorState error={audit.error} onRetry={() => audit.refetch()} /> : null}
      {audit.data && items.length === 0 ? <EmptyState title="No audit entries" /> : null}
      {items.length > 0 ? (
        <DataTable caption="Workspace audit entries">
          <thead>
            <tr>
              <Th>When</Th>
              <Th>Actor</Th>
              <Th>Action</Th>
              <Th>Target</Th>
              <Th>Outcome</Th>
            </tr>
          </thead>
          <tbody>
            {items.map((e) => (
              <tr key={e.id}>
                <Td className="whitespace-nowrap text-ink-2">{fmtDate(e.created_at)}</Td>
                <Td>{e.actor_label}</Td>
                <Td className="font-mono text-xs">{e.action}</Td>
                <Td>{e.target_kind ? `${e.target_kind} ${e.target_id ?? ""}` : "—"}</Td>
                <Td>{e.outcome}</Td>
              </tr>
            ))}
          </tbody>
        </DataTable>
      ) : null}
      {audit.hasNextPage ? (
        <Button
          variant="outline"
          disabled={audit.isFetchingNextPage}
          onClick={() => void audit.fetchNextPage()}
        >
          Load more
        </Button>
      ) : null}
    </div>
  );
}
