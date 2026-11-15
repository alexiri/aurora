"use client";

import { useInfiniteQuery } from "@tanstack/react-query";

import { api } from "./client";
import { unwrap } from "./unwrap";

export interface InstanceAuditFilter {
  scope: "instance" | "all";
  /** Organization id (implies `all`); empty = every organization. */
  org: string;
  action: string;
  criticalOnly: boolean;
  /** `YYYY-MM-DD`, empty = unbounded. */
  since: string;
  until: string;
}

/** Day start (UTC) of a `YYYY-MM-DD` input, or the day after for `until` so the day is inclusive. */
function day(v: string, plus = 0): string | undefined {
  if (!v) return undefined;
  const d = new Date(`${v}T00:00:00Z`);
  if (Number.isNaN(d.getTime())) return undefined;
  d.setUTCDate(d.getUTCDate() + plus);
  return d.toISOString();
}

/** Admin › Audit (FIX-125): instance-level rows, or every row with the organization. */
export function useInstanceAudit(f: InstanceAuditFilter) {
  return useInfiniteQuery({
    queryKey: ["admin", "audit", f] as const,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/v1/admin/audit", {
          params: {
            query: {
              limit: 50,
              cursor: pageParam,
              scope: f.scope,
              org: f.org || undefined,
              action: f.action || undefined,
              critical: f.criticalOnly || undefined,
              since: day(f.since),
              until: day(f.until, 1),
            },
          },
        }),
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}
