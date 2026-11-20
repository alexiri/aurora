"use client";

import Link from "next/link";

import { formatDay } from "@/lib/format-date";

import { buttonVariants } from "@/components/ui/button";
import { useAdminAttention } from "@/lib/api/attention";
import { cn } from "@/lib/utils";

const fmtSince = (s: string) => formatDay(s);

/**
 * (FIX-167 Part B, D-370, C-1022) Administration › Overview opens with what needs the instance admin:
 * decisions first, then facts. Each item has its title, detail, since, and one button to the page that
 * resolves it. With nothing to show (or while loading, or on an error) there is no section.
 */
export function AttentionSection() {
  const q = useAdminAttention();
  const items = q.data?.items ?? [];
  if (items.length === 0) return null;
  return (
    <section aria-labelledby="attention-h" className="space-y-4 rounded-card border border-pending-line bg-pending-panel p-4" data-testid="admin-attention">
      <div>
        <h2 id="attention-h" className="font-display text-base font-semibold text-ink">
          Needs your attention
        </h2>
        <p className="text-sm text-ink-2">Decisions come first. Each item opens the page that resolves it.</p>
      </div>
      <ul className="divide-y divide-line">
        {items.map((i) => (
          <li key={`${i.kind}:${i.subject_id}`} data-kind={i.kind} className="flex flex-wrap items-center gap-x-4 gap-y-2 py-3 text-sm first:pt-0 last:pb-0">
            <div className="min-w-0 flex-1">
              <p className="font-semibold text-ink">{i.title}</p>
              <p className="text-ink-2">{i.detail}</p>
              <p className="text-xs text-muted">Since {fmtSince(i.since)}</p>
            </div>
            <Link
              href={i.href}
              aria-label={`${i.decision ? "Review" : "Open"}: ${i.title}`}
              className={cn(buttonVariants({ variant: i.decision ? "primary" : "outline", size: "sm" }), i.decision && "text-on-primary!")}
            >
              {i.decision ? "Review" : "Open"}
            </Link>
          </li>
        ))}
      </ul>
    </section>
  );
}
