"use client";

import Link from "next/link";

import { buttonVariants } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { useConnectorTypes } from "@/lib/api/connectors";
import { cn } from "@/lib/utils";

/**
 * (FIX-127, D-219) Admin › Setup: a signed connector type that the connector host announces and
 * nobody has added yet. It only points at Admin › Connectors; adding stays an explicit act there.
 * Renders nothing while loading, on error, with no host, or when everything is added.
 */
export function ConnectorAvailableCallout() {
  const q = useConnectorTypes();
  const open = q.data?.items.filter((t) => !t.added && t.signed !== false) ?? [];
  if (open.length === 0) return null;
  return (
    <div className="space-y-2" aria-label="Connectors available to add">
      {open.map((t) => (
        <Notice key={t.type_id} tone="pending" role="status" className="flex flex-wrap items-center gap-x-4 gap-y-2">
          <span className="min-w-0 flex-1 basis-60">
            <strong className="font-semibold">{t.name} is available — add it.</strong> Organizations can then connect
            their chat to agents. Adding is yours to approve; nothing is added automatically.
          </span>
          <Link href="/admin/connectors" className={cn(buttonVariants({ size: "sm" }), "text-on-primary!")}>
            Add {t.name}
          </Link>
        </Notice>
      ))}
    </div>
  );
}
