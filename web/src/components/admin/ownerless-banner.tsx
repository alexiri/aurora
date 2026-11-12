"use client";

import { Notice } from "@/components/ui/notice";
import { useAdminOverview } from "@/lib/api/org";

/**
 * (FIX-050, C-630) Admin warning banner for organizations without an active owner: the gateway
 * refuses every credential of such an organization, so its agents are offline until an owner is
 * restored. Renders nothing while the overview is loading or lists no ownerless organization
 * (the field is absent then).
 */
export function OwnerlessBanner() {
  const overview = useAdminOverview();
  const orgs = overview.data?.ownerless_orgs ?? [];
  if (overview.isPending || orgs.length === 0) return null;
  return (
    <Notice tone="pending" role="alert" data-testid="ownerless-banner">
      <strong>Organizations without an active owner.</strong>{" "}
      {orgs.map((o) => o.slug).join(", ")}{" "}
      {orgs.length === 1 ? "has" : "have"} no active owner, so the gateway refuses every credential
      of {orgs.length === 1 ? "its" : "their"} agents. Restore an owner (dashboard &rsaquo;{" "}
      {orgs.length === 1 ? orgs[0].slug : "the organization"} &rsaquo; Members) or run the repair
      UPDATE in docs/operator/install-helm.md &sect;Troubleshooting; the change takes effect within
      seconds.
    </Notice>
  );
}
