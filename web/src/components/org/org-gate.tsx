"use client";

import type { ReactNode } from "react";

import { ErrorState, Loading } from "@/components/states";
import { useOrg, useResolve, type Org } from "@/lib/api/queries";
import { forbiddenError, orgRoleFor, whoCanFromContacts } from "@/lib/who-can";

/** Resolves the org slug, loads the org and renders children for allowed roles. */
export function OrgGate({
  slug,
  allow,
  children,
}: {
  slug: string;
  allow: ReadonlyArray<Org["my_role"]>;
  children: (org: Org) => ReactNode;
}) {
  const resolved = useResolve(slug);
  const org = useOrg(resolved.data?.org.id);
  if (resolved.isPending || (resolved.data && org.isPending)) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (org.isError) return <ErrorState error={org.error} onRetry={() => org.refetch()} />;
  if (!org.data) return null;
  // (C-915) A refusal names who can open the page, from the organization's contacts; never a bare `{ code }`.
  if (!allow.includes(org.data.my_role)) {
    const role = orgRoleFor(allow);
    return <ErrorState error={forbiddenError(whoCanFromContacts(role, org.data.contacts, "view_this"))} />;
  }
  return <>{children(org.data)}</>;
}
