"use client";

import { useParams } from "next/navigation";

import { OrgGate } from "@/components/org/org-gate";

import { GovernanceView } from "./governance-view";

export function OrgGovernance() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin", "member"]}>
      {(org) => <GovernanceView orgId={org.id} orgSlug={slug} canEdit={org.my_role === "owner" || org.my_role === "admin"} />}
    </OrgGate>
  );
}
