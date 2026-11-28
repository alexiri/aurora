"use client";

import type { ReactNode } from "react";

import { Altitude } from "@/components/ui/altitude";
import { useMe } from "@/lib/api/queries";
import { cn } from "@/lib/utils";

/**
 * (P5-REFINE-2, rec 13) The one fold for machinery: a collapsed "Technical details" with a mono body, for what only
 * someone setting things up needs (ids, hosts, methods, image names, raw phases). Members never need it open.
 *
 * `admin` renders it only for instance administrators (`GET /v1/me`). Without `admin` it is for Set up altitude and
 * above (`Altitude`, registry id `tech.details`): a person at Use doesn't see it, and "Show more controls" opens it.
 * A control that does nothing but open this fold needs the same `Altitude` around it.
 */
export function TechnicalDetails({
  admin = false,
  ...props
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
  return admin ? (
    <AdminOnly>
      <Fold {...props} />
    </AdminOnly>
  ) : (
    <Altitude id="tech.details" level="setup">
      <Fold {...props} />
    </Altitude>
  );
}

function AdminOnly({ children }: { children: ReactNode }) {
  const me = useMe();
  return me.data?.user.is_instance_admin === true ? <>{children}</> : null;
}

function Fold({
  children,
  title = "Technical details",
  className,
  summaryClassName,
  bodyClassName = "font-mono text-xs text-ink-2",
  "data-testid": testId,
}: {
  children: ReactNode;
  title?: string;
  className?: string;
  summaryClassName?: string;
  bodyClassName?: string;
  "data-testid"?: string;
}) {
  return (
    <details data-testid={testId} className={cn("text-sm", className)}>
      <summary className={cn("cursor-pointer text-muted", summaryClassName)}>{title}</summary>
      <div className={cn("mt-2", bodyClassName)}>{children}</div>
    </details>
  );
}
