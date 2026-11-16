"use client";

import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { useEffect, type ReactNode } from "react";

import { Loading } from "@/components/states";
import { legacyTarget } from "@/lib/legacy-routes";

/**
 * Pages that keep an old path (a `?tab=` decides where the old page went) wrap their content in this: an old
 * address is replaced, with its query and anchor, by the topic page and tab that took over (C-960); any other
 * address renders `children`.
 */
export function LegacyGate({ children }: { children: ReactNode }) {
  const pathname = usePathname();
  const search = useSearchParams().toString();
  const router = useRouter();
  const target = legacyTarget(pathname, search);
  useEffect(() => {
    // The anchor never reaches the server, so it is read here, where the browser has it.
    const to = legacyTarget(pathname, search, window.location.hash);
    if (to) router.replace(to);
  }, [pathname, search, router]);
  return target ? <Loading /> : <>{children}</>;
}
