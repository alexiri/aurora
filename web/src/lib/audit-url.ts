"use client";

import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { useCallback } from "react";

import type { InstanceAuditFilter } from "@/lib/api/admin-audit";

// D-373, C-1027: the audit filters live in the URL, so a shared link opens the same view. An invalid value is ignored.

const DAY = /^\d{4}-\d{2}-\d{2}$/;
const validDay = (v: string | null): string => (v && DAY.test(v) && !Number.isNaN(new Date(`${v}T00:00:00Z`).getTime()) ? v : "");

export interface BasicAuditFilter {
  action: string;
  criticalOnly: boolean;
}

export function basicFromParams(sp: URLSearchParams): BasicAuditFilter {
  return { action: sp.get("action") ?? "", criticalOnly: sp.get("critical") === "1" };
}

export function instanceFromParams(sp: URLSearchParams): InstanceAuditFilter {
  const org = sp.get("org") ?? "";
  const scope = sp.get("scope") === "all" || org ? "all" : "instance";
  return { ...basicFromParams(sp), scope, org, since: validDay(sp.get("since")), until: validDay(sp.get("until")) };
}

/** The filter as search parameters, leaving empty values out. */
function toParams(f: Partial<InstanceAuditFilter>): Record<string, string> {
  const out: Record<string, string> = {};
  if (f.action) out.action = f.action;
  if (f.scope === "all") out.scope = "all";
  if (f.org) out.org = f.org;
  if (f.criticalOnly) out.critical = "1";
  if (f.since) out.since = f.since;
  if (f.until) out.until = f.until;
  return out;
}

const OWN = ["action", "scope", "org", "critical", "since", "until"];

/** Writes the filter to the URL with `router.replace`, keeping every other parameter (`tab`). */
export function useWriteAuditParams() {
  const router = useRouter();
  const pathname = usePathname();
  const sp = useSearchParams();
  return useCallback(
    (f: Partial<InstanceAuditFilter>) => {
      const next = new URLSearchParams(sp.toString());
      for (const k of OWN) next.delete(k);
      for (const [k, v] of Object.entries(toParams(f))) next.set(k, v);
      const q = next.toString();
      router.replace(q ? `${pathname}?${q}` : pathname, { scroll: false });
    },
    [router, pathname, sp],
  );
}
