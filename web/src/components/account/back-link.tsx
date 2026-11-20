"use client";

import Link from "next/link";
import { useSearchParams } from "next/navigation";

import { agentPathParts, safeFrom } from "@/lib/back-link";
import { useAgents, useResolve } from "@/lib/api/queries";

/**
 * (P5-REFINE-2, org-3) "← Back to <Agent name>" at the top of an Account page that was opened from an agent's page
 * (`?from=/o/…`). A `from` that is not a same-origin `/o/` path shows nothing.
 */
export function BackToAgent() {
  const from = safeFrom(useSearchParams().get("from"));
  const parts = from ? agentPathParts(from) : null;
  const resolved = useResolve(parts?.org, parts?.ws);
  const agents = useAgents(resolved.data?.workspace?.id);
  if (!from) return null;
  const agent = parts ? agents.data?.items.find((a) => a.handle === parts.handle) : undefined;
  return (
    <Link href={from} className="mb-4 inline-flex items-center gap-1 text-sm font-medium text-link underline">
      <span aria-hidden="true">←</span> Back to {agent?.name ?? (parts ? `@${parts.handle}` : "where you were")}
    </Link>
  );
}
