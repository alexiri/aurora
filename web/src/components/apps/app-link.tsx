"use client";

import Link from "next/link";
import { useParams } from "next/navigation";
import type { ReactNode } from "react";

import { adminRoutes, orgRoutes, wsRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";

/**
 * (P5-UX-9, C-933) Where an app's name leads, at the viewer's level: a workspace's or an organization's setup page,
 * and in Administration (which has no setup page) the app's catalogue entry. The level is the one of the page in the
 * URL, so every tab that names an app uses the same rule.
 */
export function useAppHref(): (app: { app_id: string; package_id?: string | null }) => string | null {
  const { org, ws } = useParams<{ org?: string; ws?: string }>();
  return (app) => {
    if (org && ws) return wsRoutes(org, ws).app(app.app_id);
    if (org) return orgRoutes(org).app(app.app_id);
    return app.package_id ? adminRoutes.package(app.package_id) : null;
  };
}

/** The app's name as a link to its setup page (or its catalogue entry in Administration); plain text where there is nowhere to go. */
export function AppLink({
  app,
  children,
  className,
}: {
  app: { app_id: string; package_id?: string | null };
  children: ReactNode;
  className?: string;
}) {
  const href = useAppHref()(app);
  if (!href) return <>{children}</>;
  return (
    <Link href={href} className={cn("font-medium text-link underline", className)}>
      {children}
    </Link>
  );
}
