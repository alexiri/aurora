"use client";

import type { ReactNode } from "react";

import { useMe } from "@/lib/api/queries";
import { cn } from "@/lib/utils";

/**
 * (P5-REFINE-2, rec 13) The one fold for machinery: a collapsed "Technical details" with a mono body, for what only
 * someone setting things up needs (ids, hosts, methods, image names, raw phases). Members never need it open.
 *
 * `admin` renders it only for instance administrators (`GET /v1/me`). Without `admin` it is for Set up altitude and
 * above; the altitude check is P5-REFINE-3's `Altitude`.
 * TODO(P5-REFINE-3): until it merges this renders for everyone; brief 3 replaces the check by the altitude.
 */
export function TechnicalDetails({
  children,
  admin = false,
  title = "Technical details",
  className,
  summaryClassName,
  bodyClassName = "font-mono text-xs text-ink-2",
  "data-testid": testId,
}: {
  children: ReactNode;
  /** Instance administrators only. */
  admin?: boolean;
  title?: string;
  className?: string;
  summaryClassName?: string;
  /** The body's text style: mono by default; a body that holds its own structure (headings, tables) resets it. */
  bodyClassName?: string;
  "data-testid"?: string;
}) {
  const me = useMe();
  if (admin && me.data?.user.is_instance_admin !== true) return null;
  return (
    <details data-testid={testId} className={cn("text-sm", className)}>
      <summary className={cn("cursor-pointer text-muted", summaryClassName)}>{title}</summary>
      <div className={cn("mt-2", bodyClassName)}>{children}</div>
    </details>
  );
}
