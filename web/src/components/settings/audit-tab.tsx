"use client";

import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
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
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <thead className="text-neutral-500">
              <tr>
                <th className="py-2 pr-4 font-medium">When</th>
                <th className="py-2 pr-4 font-medium">Actor</th>
                <th className="py-2 pr-4 font-medium">Action</th>
                <th className="py-2 pr-4 font-medium">Target</th>
                <th className="py-2 font-medium">Outcome</th>
              </tr>
            </thead>
            <tbody>
              {items.map((e) => (
                <tr key={e.id} className="border-t border-neutral-200">
                  <td className="py-2 pr-4">{fmtDate(e.created_at)}</td>
                  <td className="py-2 pr-4">{e.actor_label}</td>
                  <td className="py-2 pr-4 font-mono">{e.action}</td>
                  <td className="py-2 pr-4">
                    {e.target_kind ? `${e.target_kind} ${e.target_id ?? ""}` : "—"}
                  </td>
                  <td className="py-2">{e.outcome}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
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
